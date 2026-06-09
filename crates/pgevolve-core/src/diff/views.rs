//! View / MV diff and OR-REPLACE compatibility.
//!
//! ## Body change detection
//!
//! Two view bodies are considered identical when their `canonical_hash` bytes
//! are equal. This avoids fragile text comparison and is already the canonical
//! form produced by T4's AST canonicalization pass and by the catalog reader.
//!
//! ## OR-REPLACE compatibility
//!
//! Per Postgres's `CREATE OR REPLACE VIEW` rules the new definition must:
//! 1. Have at least as many columns as the existing view.
//! 2. Keep the same column names at the same positions.
//! 3. Keep the same types at the same positions.
//!
//! New columns may only be appended at the end.
//!
//! `or_replace_compatible` encodes exactly these rules.

use std::collections::BTreeSet;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::grant::GrantTarget;
use crate::ir::view::{MaterializedView, View, ViewColumn};

use super::change::{Change, MvChange, ViewChange};
use super::changeset::{ChangeSet, RevokeWithOwnerObservation, UnmanagedGrantObservation};
use super::destructiveness::Destructiveness;
use super::grants::diff_grants;
use super::owner_op::{AlterObjectOwner, OwnerObjectKind};

/// Per Postgres's `CREATE OR REPLACE VIEW` rules: the new column list must be
/// a non-shrinking superset of the existing list, with the same names **and
/// types** at the same indexes. New columns may be appended at the end.
///
/// `catalog` is the existing column list (live DB / target).
/// `source` is the desired column list (source SQL).
pub(crate) fn or_replace_compatible(catalog: &[ViewColumn], source: &[ViewColumn]) -> bool {
    if source.len() < catalog.len() {
        return false;
    }
    for (i, cat_col) in catalog.iter().enumerate() {
        let src_col = &source[i];
        if cat_col.name != src_col.name || cat_col.column_type != src_col.column_type {
            return false;
        }
    }
    true
}

/// Diff `target.views` (live DB) against `source.views` (desired).
///
/// Pairs views by [`QualifiedName`] and emits:
/// - [`ViewChange::Create`] for views present only in source.
/// - [`ViewChange::Drop`] for views present only in target.
/// - [`ViewChange::ReplaceBody`] when the canonical body hashes differ.
/// - [`ViewChange::SetReloption`] when `security_barrier` / `security_invoker`
///   differ (independent of body changes).
/// - [`ViewChange::SetComment`] / [`ViewChange::SetColumnComment`] for metadata
///   changes.
/// - [`Change::AlterObjectOwner`] / grant changes when applicable.
///
/// No change is emitted when target and source are byte-for-byte identical.
#[allow(clippy::too_many_lines)] // exhaustive per-view-property diff with create / replace / drop branches.
pub fn diff_views(
    target_views: &[View],
    source_views: &[View],
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    use std::collections::BTreeMap;

    let target_map: BTreeMap<&QualifiedName, &View> =
        target_views.iter().map(|v| (&v.qname, v)).collect();
    let source_map: BTreeMap<&QualifiedName, &View> =
        source_views.iter().map(|v| (&v.qname, v)).collect();

    // Views in source but not target → Create.
    for (qname, src) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::View(ViewChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Views in target but not source → Drop.
    for qname in target_map.keys() {
        if !source_map.contains_key(qname) {
            out.push(
                Change::View(ViewChange::Drop((*qname).clone())),
                Destructiveness::RequiresApproval {
                    reason: format!("drops view {qname}"),
                },
            );
        }
    }

    // Views present in both — check for changes.
    for (qname, src) in &source_map {
        let Some(tgt) = target_map.get(qname) else {
            continue;
        };

        // Body change: compare canonical hashes.
        if src.body_canonical.canonical_hash() != tgt.body_canonical.canonical_hash() {
            let compatible = or_replace_compatible(&tgt.columns, &src.columns);
            out.push(
                Change::View(ViewChange::ReplaceBody {
                    source: (*src).clone(),
                    catalog: (*tgt).clone(),
                    compatible,
                }),
                Destructiveness::Safe,
            );
        }

        // Reloption changes (independent of body).
        if src.security_barrier != tgt.security_barrier
            || src.security_invoker != tgt.security_invoker
        {
            out.push(
                Change::View(ViewChange::SetReloption {
                    qname: (*qname).clone(),
                    security_barrier: src.security_barrier,
                    security_invoker: src.security_invoker,
                }),
                Destructiveness::Safe,
            );
        }

        // Check option change (lenient — only managed when source declares it).
        if src.check_option != tgt.check_option {
            out.push(
                Change::AlterViewSetCheckOption {
                    qname: (*qname).clone(),
                    new_value: src.check_option,
                },
                Destructiveness::Safe,
            );
        }

        // View-level comment change.
        if src.comment != tgt.comment {
            out.push(
                Change::View(ViewChange::SetComment {
                    qname: (*qname).clone(),
                    comment: src.comment.clone(),
                }),
                Destructiveness::Safe,
            );
        }

        // Column comment changes: compare by name at matching positions.
        diff_view_column_comments(qname, &tgt.columns, &src.columns, out);

        // ---- owner diff ----
        if let Some(source_owner) = &src.owner
            && tgt.owner.as_ref() != Some(source_owner)
        {
            out.push(
                Change::AlterObjectOwner(AlterObjectOwner {
                    kind: OwnerObjectKind::View,
                    id: crate::diff::owner_op::OwnedObjectId::Qualified((*qname).clone()),
                    signature: String::new(),
                    from: tgt.owner.clone(),
                    to: source_owner.clone(),
                }),
                Destructiveness::Safe,
            );
        }

        // ---- grant diff ----
        {
            let object_label = format!("view {qname}");
            let (to_add, to_revoke, unmanaged) =
                diff_grants(&tgt.grants, &src.grants, managed_roles);
            // Emit REVOKEs before GRANTs (issue #33): revokes must precede
            // grants so that WGO-change pairs (same grantee+privilege, different
            // wgo) don't self-cancel.
            for g in to_revoke {
                if let Some(source_owner) = &src.owner {
                    out.revokes_with_owner.push(RevokeWithOwnerObservation {
                        object_label: object_label.clone(),
                        privilege_label: g.privilege.sql_keyword().into(),
                        grantee: g.grantee.clone(),
                        owner: source_owner.clone(),
                    });
                }
                let is_column_level = g.columns.is_some();
                if is_column_level {
                    out.push(
                        Change::RevokeColumnPrivilege {
                            qname: (*qname).clone(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                } else {
                    out.push(
                        Change::RevokeObjectPrivilege {
                            qname: (*qname).clone(),
                            kind: OwnerObjectKind::View,
                            signature: String::new(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                }
            }
            for g in to_add {
                let is_column_level = g.columns.is_some();
                if is_column_level {
                    out.push(
                        Change::GrantColumnPrivilege {
                            qname: (*qname).clone(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                } else {
                    out.push(
                        Change::GrantObjectPrivilege {
                            qname: (*qname).clone(),
                            kind: OwnerObjectKind::View,
                            signature: String::new(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                }
            }
            for g in unmanaged {
                if let GrantTarget::Role(role_name) = &g.grantee {
                    out.unmanaged_grants.push(UnmanagedGrantObservation {
                        object_label: object_label.clone(),
                        privilege_label: g.privilege.sql_keyword().into(),
                        role_name: role_name.clone(),
                    });
                }
            }
        }
    }
}

/// Diff `target.materialized_views` (live DB) against
/// `source.materialized_views` (desired).
///
/// Emits:
/// - [`MvChange::Create`] for MVs present only in source.
/// - [`MvChange::Drop`] for MVs present only in target (Safe: MVs are derived).
/// - [`MvChange::ReplaceBody`] when the canonical body hashes differ.
/// - [`MvChange::SetComment`] / [`MvChange::SetColumnComment`] for metadata.
/// - [`Change::AlterObjectOwner`] / grant changes when applicable.
#[allow(clippy::too_many_lines)] // exhaustive per-MV-property diff with create / replace / drop branches.
pub fn diff_materialized_views(
    target_mvs: &[MaterializedView],
    source_mvs: &[MaterializedView],
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    use std::collections::BTreeMap;

    let target_map: BTreeMap<&QualifiedName, &MaterializedView> =
        target_mvs.iter().map(|m| (&m.qname, m)).collect();
    let source_map: BTreeMap<&QualifiedName, &MaterializedView> =
        source_mvs.iter().map(|m| (&m.qname, m)).collect();

    // MVs in source but not target → Create.
    for (qname, src) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::Mv(MvChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // MVs in target but not source → Drop (Safe: MVs are derived data).
    for qname in target_map.keys() {
        if !source_map.contains_key(qname) {
            out.push(
                Change::Mv(MvChange::Drop((*qname).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // MVs present in both — check for changes.
    for (qname, src) in &source_map {
        let Some(tgt) = target_map.get(qname) else {
            continue;
        };

        // Body change: compare canonical hashes.
        if src.body_canonical.canonical_hash() != tgt.body_canonical.canonical_hash() {
            out.push(
                Change::Mv(MvChange::ReplaceBody {
                    source: (*src).clone(),
                    catalog: (*tgt).clone(),
                }),
                Destructiveness::Safe,
            );
        }

        // MV-level comment change.
        if src.comment != tgt.comment {
            out.push(
                Change::Mv(MvChange::SetComment {
                    qname: (*qname).clone(),
                    comment: src.comment.clone(),
                }),
                Destructiveness::Safe,
            );
        }

        // Column comment changes.
        diff_mv_column_comments(qname, &tgt.columns, &src.columns, out);

        // ---- storage reloptions diff ----
        let delta = crate::diff::reloptions::table_delta(&tgt.storage, &src.storage);
        if !delta.is_empty() {
            out.push(
                Change::SetMaterializedViewStorage {
                    qname: (*qname).clone(),
                    options: delta,
                },
                Destructiveness::Safe,
            );
        }

        // ---- owner diff ----
        if let Some(source_owner) = &src.owner
            && tgt.owner.as_ref() != Some(source_owner)
        {
            out.push(
                Change::AlterObjectOwner(AlterObjectOwner {
                    kind: OwnerObjectKind::MaterializedView,
                    id: crate::diff::owner_op::OwnedObjectId::Qualified((*qname).clone()),
                    signature: String::new(),
                    from: tgt.owner.clone(),
                    to: source_owner.clone(),
                }),
                Destructiveness::Safe,
            );
        }

        // ---- grant diff ----
        {
            let object_label = format!("materialized view {qname}");
            let (to_add, to_revoke, unmanaged) =
                diff_grants(&tgt.grants, &src.grants, managed_roles);
            // Emit REVOKEs before GRANTs (issue #33): revokes must precede
            // grants so that WGO-change pairs (same grantee+privilege, different
            // wgo) don't self-cancel.
            for g in to_revoke {
                if let Some(source_owner) = &src.owner {
                    out.revokes_with_owner.push(RevokeWithOwnerObservation {
                        object_label: object_label.clone(),
                        privilege_label: g.privilege.sql_keyword().into(),
                        grantee: g.grantee.clone(),
                        owner: source_owner.clone(),
                    });
                }
                let is_column_level = g.columns.is_some();
                if is_column_level {
                    out.push(
                        Change::RevokeColumnPrivilege {
                            qname: (*qname).clone(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                } else {
                    out.push(
                        Change::RevokeObjectPrivilege {
                            qname: (*qname).clone(),
                            kind: OwnerObjectKind::MaterializedView,
                            signature: String::new(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                }
            }
            for g in to_add {
                let is_column_level = g.columns.is_some();
                if is_column_level {
                    out.push(
                        Change::GrantColumnPrivilege {
                            qname: (*qname).clone(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                } else {
                    out.push(
                        Change::GrantObjectPrivilege {
                            qname: (*qname).clone(),
                            kind: OwnerObjectKind::MaterializedView,
                            signature: String::new(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                }
            }
            for g in unmanaged {
                if let GrantTarget::Role(role_name) = &g.grantee {
                    out.unmanaged_grants.push(UnmanagedGrantObservation {
                        object_label: object_label.clone(),
                        privilege_label: g.privilege.sql_keyword().into(),
                        role_name: role_name.clone(),
                    });
                }
            }
        }
    }
}

/// Emit [`ViewChange::SetColumnComment`] for each column where the comment
/// differs between target and source.
fn diff_view_column_comments(
    qname: &QualifiedName,
    target_cols: &[ViewColumn],
    source_cols: &[ViewColumn],
    out: &mut ChangeSet,
) {
    use std::collections::BTreeMap;

    let tgt_map: BTreeMap<_, _> = target_cols.iter().map(|c| (&c.name, c)).collect();
    let src_map: BTreeMap<_, _> = source_cols.iter().map(|c| (&c.name, c)).collect();

    for (name, src_col) in &src_map {
        let tgt_comment = tgt_map.get(name).and_then(|c| c.comment.as_deref());
        let src_comment = src_col.comment.as_deref();
        if src_comment != tgt_comment {
            out.push(
                Change::View(ViewChange::SetColumnComment {
                    qname: qname.clone(),
                    column: (*name).clone(),
                    comment: src_col.comment.clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }
}

/// Emit [`MvChange::SetColumnComment`] for each column where the comment
/// differs between target and source.
fn diff_mv_column_comments(
    qname: &QualifiedName,
    target_cols: &[ViewColumn],
    source_cols: &[ViewColumn],
    out: &mut ChangeSet,
) {
    use std::collections::BTreeMap;

    let tgt_map: BTreeMap<_, _> = target_cols.iter().map(|c| (&c.name, c)).collect();
    let src_map: BTreeMap<_, _> = source_cols.iter().map(|c| (&c.name, c)).collect();

    for (name, src_col) in &src_map {
        let tgt_comment = tgt_map.get(name).and_then(|c| c.comment.as_deref());
        let src_comment = src_col.comment.as_deref();
        if src_comment != tgt_comment {
            out.push(
                Change::Mv(MvChange::SetColumnComment {
                    qname: qname.clone(),
                    column: (*name).clone(),
                    comment: src_col.comment.clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::identifier::Identifier;
    use crate::ir::column_type::ColumnType;
    use crate::ir::view::{MaterializedView, View, ViewColumn};
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn body(sql: &str) -> NormalizedBody {
        NormalizedBody::from_sql(sql).unwrap()
    }

    fn col(name: &str, ty: ColumnType) -> ViewColumn {
        ViewColumn {
            name: id(name),
            column_type: Some(ty),
            comment: None,
        }
    }

    fn simple_view(qname: QualifiedName, body_sql: &str, cols: Vec<ViewColumn>) -> View {
        View {
            qname,
            columns: cols,
            body_canonical: body(body_sql),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        }
    }

    fn simple_mv(qname: QualifiedName, body_sql: &str, cols: Vec<ViewColumn>) -> MaterializedView {
        MaterializedView {
            qname,
            columns: cols,
            body_canonical: body(body_sql),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        }
    }

    // ------------------------------------------------------------------ //
    // OR-REPLACE compatibility predicate tests
    // ------------------------------------------------------------------ //

    #[test]
    fn identical_column_lists_are_compatible() {
        let cols = vec![col("id", ColumnType::BigInt), col("name", ColumnType::Text)];
        assert!(or_replace_compatible(&cols, &cols));
    }

    #[test]
    fn appending_columns_is_compatible() {
        let catalog = vec![col("id", ColumnType::BigInt)];
        let source = vec![col("id", ColumnType::BigInt), col("name", ColumnType::Text)];
        assert!(or_replace_compatible(&catalog, &source));
    }

    #[test]
    fn renaming_a_column_is_incompatible() {
        let catalog = vec![col("id", ColumnType::BigInt)];
        let source = vec![col("user_id", ColumnType::BigInt)];
        assert!(!or_replace_compatible(&catalog, &source));
    }

    #[test]
    fn dropping_a_column_is_incompatible() {
        let catalog = vec![col("id", ColumnType::BigInt), col("name", ColumnType::Text)];
        let source = vec![col("id", ColumnType::BigInt)];
        assert!(!or_replace_compatible(&catalog, &source));
    }

    #[test]
    fn reordering_is_incompatible() {
        let catalog = vec![col("id", ColumnType::BigInt), col("name", ColumnType::Text)];
        let source = vec![col("name", ColumnType::Text), col("id", ColumnType::BigInt)];
        assert!(!or_replace_compatible(&catalog, &source));
    }

    #[test]
    fn type_change_is_incompatible() {
        let catalog = vec![col("id", ColumnType::Integer)];
        let source = vec![col("id", ColumnType::BigInt)];
        assert!(!or_replace_compatible(&catalog, &source));
    }

    // ------------------------------------------------------------------ //
    // diff_views tests
    // ------------------------------------------------------------------ //

    #[test]
    fn view_only_in_source_is_create() {
        let src = vec![simple_view(qn("app", "v"), "SELECT 1", vec![])];
        let mut out = ChangeSet::new();
        diff_views(&[], &src, &mut out, &BTreeSet::new());
        let changes = &out.entries;
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0].change,
            Change::View(ViewChange::Create(_))
        ));
    }

    #[test]
    fn view_only_in_target_is_drop() {
        let tgt = vec![simple_view(qn("app", "v"), "SELECT 1", vec![])];
        let mut out = ChangeSet::new();
        diff_views(&tgt, &[], &mut out, &BTreeSet::new());
        let changes = &out.entries;
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0].change,
            Change::View(ViewChange::Drop(_))
        ));
        assert!(changes[0].destructiveness.requires_approval());
    }

    #[test]
    fn view_body_change_with_compatible_columns_is_replace_body_compatible_true() {
        let cols = vec![col("id", ColumnType::BigInt)];
        let tgt = vec![simple_view(qn("app", "v"), "SELECT 1 AS id", cols.clone())];
        let src = vec![simple_view(qn("app", "v"), "SELECT 2 AS id", cols)];
        let mut out = ChangeSet::new();
        diff_views(&tgt, &src, &mut out, &BTreeSet::new());
        let changes = &out.entries;
        assert_eq!(changes.len(), 1);
        match &changes[0].change {
            Change::View(ViewChange::ReplaceBody { compatible, .. }) => {
                assert!(*compatible, "expected compatible=true");
            }
            other => panic!("unexpected change: {other:?}"),
        }
    }

    #[test]
    fn view_body_change_with_incompatible_columns_is_replace_body_compatible_false() {
        let tgt_cols = vec![col("id", ColumnType::Integer)];
        let src_cols = vec![col("id", ColumnType::BigInt)];
        let tgt = vec![simple_view(qn("app", "v"), "SELECT 1::int AS id", tgt_cols)];
        let src = vec![simple_view(
            qn("app", "v"),
            "SELECT 2::bigint AS id",
            src_cols,
        )];
        let mut out = ChangeSet::new();
        diff_views(&tgt, &src, &mut out, &BTreeSet::new());
        let changes = &out.entries;
        assert_eq!(changes.len(), 1);
        match &changes[0].change {
            Change::View(ViewChange::ReplaceBody { compatible, .. }) => {
                assert!(!*compatible, "expected compatible=false");
            }
            other => panic!("unexpected change: {other:?}"),
        }
    }

    #[test]
    fn view_security_barrier_change_emits_set_reloption() {
        let body_sql = "SELECT 1";
        let tgt = vec![{
            let mut v = simple_view(qn("app", "v"), body_sql, vec![]);
            v.security_barrier = Some(false);
            v
        }];
        let src = vec![{
            let mut v = simple_view(qn("app", "v"), body_sql, vec![]);
            v.security_barrier = Some(true);
            v
        }];
        let mut out = ChangeSet::new();
        diff_views(&tgt, &src, &mut out, &BTreeSet::new());
        let changes = &out.entries;
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0].change,
            Change::View(ViewChange::SetReloption { .. })
        ));
    }

    #[test]
    fn identical_views_emit_no_changes() {
        let v = simple_view(qn("app", "v"), "SELECT 1", vec![]);
        let mut out = ChangeSet::new();
        diff_views(
            std::slice::from_ref(&v),
            std::slice::from_ref(&v),
            &mut out,
            &BTreeSet::new(),
        );
        assert!(out.is_empty());
    }

    // ------------------------------------------------------------------ //
    // diff_materialized_views tests
    // ------------------------------------------------------------------ //

    #[test]
    fn mv_only_in_source_is_create() {
        let src = vec![simple_mv(qn("app", "mv"), "SELECT 1", vec![])];
        let mut out = ChangeSet::new();
        diff_materialized_views(&[], &src, &mut out, &BTreeSet::new());
        let changes = &out.entries;
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0].change,
            Change::Mv(MvChange::Create(_))
        ));
    }

    #[test]
    fn mv_drop_does_not_set_destructive() {
        let tgt = vec![simple_mv(qn("app", "mv"), "SELECT 1", vec![])];
        let mut out = ChangeSet::new();
        diff_materialized_views(&tgt, &[], &mut out, &BTreeSet::new());
        let changes = &out.entries;
        assert_eq!(changes.len(), 1);
        assert!(matches!(&changes[0].change, Change::Mv(MvChange::Drop(_))));
        // MVs are derived data — drop is Safe.
        assert!(!changes[0].destructiveness.requires_approval());
    }

    #[test]
    fn identical_mvs_emit_no_changes() {
        let mv = simple_mv(qn("app", "mv"), "SELECT 1", vec![]);
        let mut out = ChangeSet::new();
        diff_materialized_views(
            std::slice::from_ref(&mv),
            std::slice::from_ref(&mv),
            &mut out,
            &BTreeSet::new(),
        );
        assert!(out.is_empty());
    }
}
