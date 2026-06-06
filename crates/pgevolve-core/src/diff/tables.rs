//! Table-level diffing.
//!
//! Pairs tables by [`QualifiedName`]. Existence differences emit
//! [`Change::CreateTable`] / [`Change::DropTable`]; pairs that are present in
//! both catalogs dispatch to [`super::columns::diff_columns`] and
//! [`super::constraints::diff_constraints`] and emit a single
//! [`Change::AlterTable`] containing every per-table operation.

use std::collections::{BTreeMap, BTreeSet};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::grant::GrantTarget;
use crate::ir::table::Table;

use super::change::{Change, TableChange};
use super::changeset::{ChangeSet, RevokeWithOwnerObservation, UnmanagedGrantObservation};
use super::columns::diff_columns;
use super::constraints::diff_constraints;
use super::destructiveness::Destructiveness;
use super::grants::diff_grants;
use super::owner_op::{AlterObjectOwner, OwnerObjectKind};
use super::table_op::TableOpEntry;

/// Diff tables in `target` against `source`, appending entries to `out`.
#[allow(clippy::too_many_lines)] // exhaustive per-table-property diff; extraction would fragment a single conceptual pass.
pub fn diff_tables(
    target: &Catalog,
    source: &Catalog,
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    let target_map: BTreeMap<&QualifiedName, &Table> =
        target.tables.iter().map(|t| (&t.qname, t)).collect();
    let source_map: BTreeMap<&QualifiedName, &Table> =
        source.tables.iter().map(|t| (&t.qname, t)).collect();

    for (qname, source_table) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::CreateTable((*source_table).clone()),
                Destructiveness::Safe,
            );
            // Synthesize an empty target so the attribute helper can diff
            // source attributes against "nothing" and emit the appropriate
            // follow-up Changes (owner, grants, policies, storage).
            let empty_target = Table {
                qname: source_table.qname.clone(),
                columns: vec![],
                constraints: vec![],
                partition_by: None,
                partition_of: None,
                comment: None,
                owner: None,
                grants: vec![],
                rls_enabled: false,
                rls_forced: false,
                policies: vec![],
                storage: crate::ir::reloptions::TableStorageOptions::default(),
                access_method: None,
            };
            emit_table_attribute_changes(&empty_target, source_table, managed_roles, out);
        }
    }

    for (qname, target_table) in &target_map {
        match source_map.get(qname) {
            None => {
                out.push(
                    Change::DropTable {
                        qname: (*qname).clone(),
                        row_count_estimate: None,
                    },
                    Destructiveness::RequiresApprovalAndDataLossWarning {
                        reason: format!("drops table {qname}"),
                    },
                );
            }
            Some(source_table) => {
                let mut ops: Vec<TableOpEntry> = Vec::new();

                // Skip column and constraint diffs when either side is a
                // partition child (`partition_of.is_some()`).
                //
                // A partition child's column list is always inherited from the
                // parent; the canonical source form (`PARTITION OF …`) never
                // includes explicit columns. Diffing them would produce spurious
                // ADD/DROP COLUMN steps. Three sub-cases:
                //
                //   1. partition_of is changing (None → Some / Some → None):
                //      ATTACH / DETACH handles column inheritance atomically.
                //      - ATTACH: child columns must already match parent.
                //      - DETACH: inherited columns become explicit automatically.
                //
                //   2. partition_of is stable (both Some): e.g. Form 3 parses
                //      a standalone CREATE TABLE + ALTER TABLE ATTACH, giving the
                //      child an explicit column list; Form 2 gives an empty list.
                //      Skip to avoid spurious DROP COLUMN steps.
                //
                //   3. partition_of is None in both: ordinary table → run diff.
                let either_is_partition =
                    source_table.partition_of.is_some() || target_table.partition_of.is_some();
                if !either_is_partition {
                    diff_columns(target_table, source_table, &mut ops);
                    diff_constraints(target_table, source_table, &mut ops);
                }

                if target_table.comment != source_table.comment {
                    ops.push(TableOpEntry {
                        op: super::table_op::TableOp::SetTableComment {
                            comment: source_table.comment.clone(),
                        },
                        destructiveness: Destructiveness::Safe,
                    });
                }

                if !ops.is_empty() {
                    out.push(
                        Change::AlterTable {
                            qname: (*qname).clone(),
                            ops,
                        },
                        Destructiveness::Safe,
                    );
                }

                // ---- partition_by diff (parent partitioning configuration) ----
                // Changing PARTITION BY cannot be done in-place; surface as an
                // UnsupportedDiff so the ordering phase aborts the plan.
                match (&source_table.partition_by, &target_table.partition_by) {
                    (None, None) => {}
                    (Some(s), Some(t)) if s == t => {}
                    (Some(_), Some(_)) => {
                        out.push(
                            Change::UnsupportedDiff {
                                reason: format!(
                                    "cannot change PARTITION BY clause on {qname} in-place; \
                                     manual migration required"
                                ),
                            },
                            Destructiveness::Safe,
                        );
                    }
                    (None, Some(_)) => {
                        out.push(
                            Change::UnsupportedDiff {
                                reason: format!(
                                    "cannot remove PARTITION BY from {qname} in-place; \
                                     manual migration required"
                                ),
                            },
                            Destructiveness::Safe,
                        );
                    }
                    (Some(_), None) => {
                        out.push(
                            Change::UnsupportedDiff {
                                reason: format!(
                                    "cannot add PARTITION BY to {qname} in-place; \
                                     manual migration required"
                                ),
                            },
                            Destructiveness::Safe,
                        );
                    }
                }

                // ---- partition_of diff (child membership in partitioned parent) ----
                match (&source_table.partition_of, &target_table.partition_of) {
                    (None, None) => {}
                    (Some(s), Some(t)) if s == t => {}
                    (Some(s), None) => {
                        // Source declares partition membership; catalog does not → attach.
                        out.push(
                            Change::Table(TableChange::AttachPartition {
                                parent: s.parent.clone(),
                                child: (*qname).clone(),
                                bounds: s.bounds.clone(),
                            }),
                            Destructiveness::Safe,
                        );
                    }
                    (None, Some(t)) => {
                        // Catalog has partition membership; source dropped it → detach.
                        out.push(
                            Change::Table(TableChange::DetachPartition {
                                parent: t.parent.clone(),
                                child: (*qname).clone(),
                            }),
                            Destructiveness::Safe,
                        );
                    }
                    (Some(s), Some(t)) if s.parent != t.parent => {
                        // Re-parented: detach from old parent, attach to new.
                        out.push(
                            Change::Table(TableChange::DetachPartition {
                                parent: t.parent.clone(),
                                child: (*qname).clone(),
                            }),
                            Destructiveness::Safe,
                        );
                        out.push(
                            Change::Table(TableChange::AttachPartition {
                                parent: s.parent.clone(),
                                child: (*qname).clone(),
                                bounds: s.bounds.clone(),
                            }),
                            Destructiveness::Safe,
                        );
                    }
                    (Some(s), Some(_)) => {
                        // Same parent, bounds differ: detach + re-attach.
                        out.push(
                            Change::Table(TableChange::DetachPartition {
                                parent: s.parent.clone(),
                                child: (*qname).clone(),
                            }),
                            Destructiveness::Safe,
                        );
                        out.push(
                            Change::Table(TableChange::AttachPartition {
                                parent: s.parent.clone(),
                                child: (*qname).clone(),
                                bounds: s.bounds.clone(),
                            }),
                            Destructiveness::Safe,
                        );
                    }
                }

                // ---- owner / grants / policies / storage diffs ----
                emit_table_attribute_changes(target_table, source_table, managed_roles, out);
            }
        }
    }
}

/// Emit per-attribute diff changes (owner, grants, policies, storage) for
/// one table pair.
///
/// Called from two sites:
/// - "both catalogs have the table" branch — with the real target table.
/// - "new table" branch — with a synthesized empty target so the diff against
///   "nothing" produces one `Change` per non-default attribute the source has.
///
/// Intentionally excludes: column/constraint diffs (handled by the ALTER TABLE
/// ops block), `partition_by`/`partition_of` (structural, inline-with-create),
/// and comment (rides in the ALTER TABLE ops block above).
/// Return `grants` with `dropped_columns` removed from every column-level
/// grant's column list. A column-level grant whose entire list is dropped is
/// omitted (the column drop alone revokes it); object-level grants
/// (`columns: None`) and grants on surviving columns pass through unchanged.
fn strip_dropped_columns_from_grants(
    grants: &[crate::ir::grant::Grant],
    dropped_columns: &BTreeSet<&Identifier>,
) -> Vec<crate::ir::grant::Grant> {
    if dropped_columns.is_empty() {
        return grants.to_vec();
    }
    grants
        .iter()
        .filter_map(|g| {
            g.columns.as_ref().map_or_else(
                || Some(g.clone()),
                |cols| {
                    let kept: Vec<Identifier> = cols
                        .iter()
                        .filter(|c| !dropped_columns.contains(c))
                        .cloned()
                        .collect();
                    if kept.is_empty() {
                        None
                    } else {
                        Some(crate::ir::grant::Grant {
                            columns: Some(kept),
                            ..g.clone()
                        })
                    }
                },
            )
        })
        .collect()
}

/// Emit the GRANT/REVOKE changes for one table pair (object- and column-level),
/// plus unmanaged-grant and revoke-with-owner observations.
fn emit_table_grant_changes(
    target_table: &Table,
    source_table: &Table,
    managed_roles: &BTreeSet<Identifier>,
    out: &mut ChangeSet,
) {
    let qname = &source_table.qname;
    let object_label = format!("table {qname}");
    // Columns present in the target (live) table but absent from the source are
    // about to be dropped by an `ALTER TABLE ... DROP COLUMN`. PG cascade-revokes
    // their column-level ACLs as part of that drop, so they must not generate
    // `RevokeColumnPrivilege` steps — an explicit `REVOKE ... (dropped_col)`
    // fails with `42703` once the column is gone. Strip dropped columns from the
    // target grants before diffing so only surviving-column changes are emitted.
    let dropped_columns: BTreeSet<&Identifier> = {
        let source_cols: BTreeSet<&Identifier> =
            source_table.columns.iter().map(|c| &c.name).collect();
        target_table
            .columns
            .iter()
            .map(|c| &c.name)
            .filter(|n| !source_cols.contains(*n))
            .collect()
    };
    let adjusted_target = strip_dropped_columns_from_grants(&target_table.grants, &dropped_columns);
    let (to_add, to_revoke, unmanaged) =
        diff_grants(&adjusted_target, &source_table.grants, managed_roles);
    // Emit REVOKEs before GRANTs (issue #33): revokes must precede grants so
    // that WGO-change pairs (same grantee+privilege, different wgo) don't
    // self-cancel (GRANT followed by REVOKE would drop the privilege entirely).
    for g in to_revoke {
        if let Some(source_owner) = &source_table.owner {
            out.revokes_with_owner.push(RevokeWithOwnerObservation {
                object_label: object_label.clone(),
                privilege_label: g.privilege.sql_keyword().into(),
                grantee: g.grantee.clone(),
                owner: source_owner.clone(),
            });
        }
        if g.columns.is_some() {
            out.push(
                Change::RevokeColumnPrivilege {
                    qname: qname.clone(),
                    grant: g,
                },
                Destructiveness::Safe,
            );
        } else {
            out.push(
                Change::RevokeObjectPrivilege {
                    qname: qname.clone(),
                    kind: OwnerObjectKind::Table,
                    signature: String::new(),
                    grant: g,
                },
                Destructiveness::Safe,
            );
        }
    }
    for g in to_add {
        if g.columns.is_some() {
            out.push(
                Change::GrantColumnPrivilege {
                    qname: qname.clone(),
                    grant: g,
                },
                Destructiveness::Safe,
            );
        } else {
            out.push(
                Change::GrantObjectPrivilege {
                    qname: qname.clone(),
                    kind: OwnerObjectKind::Table,
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

fn emit_table_attribute_changes(
    target_table: &Table,
    source_table: &Table,
    managed_roles: &BTreeSet<Identifier>,
    out: &mut ChangeSet,
) {
    let qname = &source_table.qname;

    // ---- owner diff ----
    if let Some(source_owner) = &source_table.owner
        && target_table.owner.as_ref() != Some(source_owner)
    {
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Table,
                id: crate::diff::owner_op::OwnedObjectId::Qualified(qname.clone()),
                signature: String::new(),
                from: target_table.owner.clone(),
                to: source_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // ---- grant diff ----
    emit_table_grant_changes(target_table, source_table, managed_roles, out);

    // ---- policy diff (RLS toggles + per-policy changes) ----
    let mut policy_changes: Vec<Change> = Vec::new();
    super::policies::diff_policies(target_table, source_table, &mut policy_changes);
    for c in policy_changes {
        out.push(c, Destructiveness::Safe);
    }

    // ---- storage reloptions diff ----
    let delta = crate::diff::reloptions::table_delta(&target_table.storage, &source_table.storage);
    if !delta.is_empty() {
        out.push(
            Change::SetTableStorage {
                qname: qname.clone(),
                options: delta,
            },
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::grant::{Grant, GrantTarget, Privilege};
    use crate::ir::partition::{
        BoundDatum, PartitionBounds, PartitionBy, PartitionColumn, PartitionColumnKind,
        PartitionOf, PartitionStrategy,
    };
    use crate::ir::policy::{Policy, PolicyCommand};
    use crate::ir::reloptions::TableStorageOptions;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn col(name: &str, ty: ColumnType, nullable: bool) -> Column {
        Column {
            name: id(name),
            ty,
            nullable,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn users() -> Table {
        Table {
            qname: qn("users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        }
    }

    /// Regression for the soak `[42703] column "X" of relation "Y" does not
    /// exist` failure. When a column carrying a *multi-column* grant is dropped
    /// — e.g. live `GRANT SELECT (id, price)` and the source drops `price`,
    /// keeping `GRANT SELECT (id)` — the grant diff must not emit a
    /// `RevokeColumnPrivilege` whose column list names the dropped column. PG
    /// cascade-revokes the column ACL as part of `ALTER TABLE ... DROP COLUMN`,
    /// so an explicit `REVOKE SELECT (id, price)` fails with 42703 once `price`
    /// is gone. Dropped columns must be stripped from the target grants before
    /// diffing; here that leaves `SELECT (id)` on both sides, so no grant change
    /// is emitted at all (the column drop alone converges the grant).
    #[test]
    fn dropped_column_does_not_emit_column_grant_revoke() {
        let writers = id("writers");
        let managed_roles: BTreeSet<Identifier> = std::iter::once(writers.clone()).collect();

        let col_grant = |cols: &[&str]| Grant {
            grantee: GrantTarget::Role(writers.clone()),
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: Some(cols.iter().map(|c| id(c)).collect()),
        };

        let mut target = Catalog::empty();
        target.tables.push(Table {
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("price", ColumnType::BigInt, true),
            ],
            grants: vec![col_grant(&["id", "price"])],
            ..users()
        });

        let mut source = Catalog::empty();
        source.tables.push(Table {
            columns: vec![col("id", ColumnType::BigInt, false)],
            grants: vec![col_grant(&["id"])],
            ..users()
        });

        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs, &managed_roles);

        for entry in &cs.entries {
            if let Change::RevokeColumnPrivilege { grant, .. } = &entry.change {
                let cols = grant.columns.as_ref().expect("column grant");
                assert!(
                    !cols.iter().any(|c| c.as_str() == "price"),
                    "must not revoke the dropped column 'price': {grant:?}"
                );
            }
            assert!(
                !matches!(&entry.change, Change::GrantColumnPrivilege { .. }),
                "no column grant needed — surviving column 'id' already has it: {:?}",
                entry.change
            );
        }
    }

    #[test]
    fn add_table_emits_create_safe() {
        let target = Catalog::empty();
        let mut source = Catalog::empty();
        source.tables.push(users());
        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs, &BTreeSet::new());
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        assert!(matches!(entry.change, Change::CreateTable(_)));
        assert_eq!(entry.destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn new_table_with_attrs_emits_create_plus_attribute_changes() {
        // A brand-new table (not in target) that carries owner, grants, a
        // policy, rls_enabled, rls_forced, and non-default storage should emit
        // one CreateTable *plus* one change per non-default attribute.
        let target = Catalog::empty();
        let mut source = Catalog::empty();

        let app_role = id("app");
        let reader_role = id("reader");
        let managed_roles: BTreeSet<Identifier> = [app_role.clone(), reader_role.clone()]
            .into_iter()
            .collect();

        let table = Table {
            qname: qn("users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: Some(app_role),
            grants: vec![Grant {
                grantee: GrantTarget::Role(reader_role),
                privilege: Privilege::Select,
                with_grant_option: false,
                columns: None,
            }],
            rls_enabled: true,
            rls_forced: true,
            policies: vec![Policy {
                name: id("tenant_isolation"),
                permissive: true,
                command: PolicyCommand::All,
                roles: vec![GrantTarget::Public],
                using: None,
                with_check: None,
            }],
            storage: TableStorageOptions {
                fillfactor: Some(70),
                ..Default::default()
            },
            access_method: None,
        };
        source.tables.push(table);

        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs, &managed_roles);

        let changes: Vec<&Change> = cs.entries.iter().map(|e| &e.change).collect();

        // Must have a CreateTable.
        assert!(
            changes.iter().any(|c| matches!(c, Change::CreateTable(_))),
            "missing CreateTable; got: {changes:?}"
        );
        // Must have an AlterObjectOwner.
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::AlterObjectOwner(_))),
            "missing AlterObjectOwner; got: {changes:?}"
        );
        // Must have a GrantObjectPrivilege.
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::GrantObjectPrivilege { .. })),
            "missing GrantObjectPrivilege; got: {changes:?}"
        );
        // Must have a CreatePolicy.
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::CreatePolicy { .. })),
            "missing CreatePolicy; got: {changes:?}"
        );
        // Must have SetTableRowSecurity (rls_enabled = true).
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::SetTableRowSecurity { enable: true, .. })),
            "missing SetTableRowSecurity; got: {changes:?}"
        );
        // Must have SetTableForceRowSecurity (rls_forced = true).
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::SetTableForceRowSecurity { force: true, .. })),
            "missing SetTableForceRowSecurity; got: {changes:?}"
        );
        // Must have SetTableStorage.
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::SetTableStorage { .. })),
            "missing SetTableStorage; got: {changes:?}"
        );
    }

    #[test]
    fn drop_table_emits_data_loss_warning() {
        let mut target = Catalog::empty();
        target.tables.push(users());
        let source = Catalog::empty();
        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs, &BTreeSet::new());
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        match &entry.change {
            Change::DropTable {
                qname,
                row_count_estimate,
            } => {
                assert_eq!(qname, &qn("users"));
                assert!(row_count_estimate.is_none());
            }
            other => panic!("expected DropTable, got {other:?}"),
        }
        assert!(entry.destructiveness.data_loss_risk());
        assert!(
            entry
                .destructiveness
                .reason()
                .unwrap()
                .contains("app.users")
        );
    }

    #[test]
    fn equal_tables_emit_nothing() {
        let mut target = Catalog::empty();
        target.tables.push(users());
        let mut source = Catalog::empty();
        source.tables.push(users());
        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs, &BTreeSet::new());
        assert!(cs.is_empty());
    }

    #[test]
    fn comment_only_change_emits_alter_with_set_comment() {
        let mut target = Catalog::empty();
        target.tables.push(users());
        let mut source = Catalog::empty();
        source.tables.push(Table {
            comment: Some("the users table".into()),
            ..users()
        });
        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs, &BTreeSet::new());
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        match &entry.change {
            Change::AlterTable { qname, ops } => {
                assert_eq!(qname, &qn("users"));
                assert_eq!(ops.len(), 1);
                assert!(matches!(
                    ops[0].op,
                    super::super::table_op::TableOp::SetTableComment { .. }
                ));
            }
            other => panic!("expected AlterTable, got {other:?}"),
        }
        assert_eq!(entry.destructiveness, Destructiveness::Safe);
    }

    // ---- partition test helpers ----

    fn qn2(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// A plain (non-partitioned) table with the given schema/name.
    fn sample_table_with_qname(schema: &str, name: &str) -> Table {
        Table {
            qname: qn2(schema, name),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        }
    }

    /// Construct a `PartitionOf` with `DEFAULT` bounds.
    fn po_default(schema: &str, parent: &str) -> PartitionOf {
        PartitionOf {
            parent: qn2(schema, parent),
            bounds: PartitionBounds::Default,
        }
    }

    /// Construct a `PartitionOf` with a single-column RANGE FROM literal TO MAXVALUE.
    fn po_range(schema: &str, parent: &str, from_lit: &str) -> PartitionOf {
        use crate::ir::default_expr::NormalizedExpr;
        PartitionOf {
            parent: qn2(schema, parent),
            bounds: PartitionBounds::Range {
                from: vec![BoundDatum::Literal(NormalizedExpr::from_text(from_lit))],
                to: vec![BoundDatum::MaxValue],
            },
        }
    }

    /// Construct a `PartitionBy LIST` on a single column.
    fn pb_list(col_name: &str) -> PartitionBy {
        PartitionBy {
            strategy: PartitionStrategy::List,
            columns: vec![PartitionColumn {
                kind: PartitionColumnKind::Column(id(col_name)),
                collation: None,
                opclass: None,
            }],
        }
    }

    /// Construct a `PartitionBy RANGE` on a single column.
    fn pb_range(col_name: &str) -> PartitionBy {
        PartitionBy {
            strategy: PartitionStrategy::Range,
            columns: vec![PartitionColumn {
                kind: PartitionColumnKind::Column(id(col_name)),
                collation: None,
                opclass: None,
            }],
        }
    }

    /// Diff `source_table` against `target_table` (as the only table in each
    /// catalog) and return the collected changes.
    fn run_diff(source: &Table, target: &Table) -> Vec<Change> {
        let mut src_catalog = Catalog::empty();
        src_catalog.tables.push(source.clone());
        let mut tgt_catalog = Catalog::empty();
        tgt_catalog.tables.push(target.clone());
        let mut cs = ChangeSet::new();
        diff_tables(&tgt_catalog, &src_catalog, &mut cs, &BTreeSet::new());
        cs.entries.into_iter().map(|e| e.change).collect()
    }

    /// Like `run_diff` but returns `Err` if any `Change::UnsupportedDiff` is
    /// emitted, or `Ok(changes)` otherwise.
    fn try_diff(source: &Table, target: &Table) -> Result<Vec<Change>, String> {
        let changes = run_diff(source, target);
        for c in &changes {
            if let Change::UnsupportedDiff { reason } = c {
                return Err(reason.clone());
            }
        }
        Ok(changes)
    }

    // ---- partition tests ----

    #[test]
    fn detects_attach_partition_when_source_declares_it() {
        // source says partition; catalog says standalone → AttachPartition
        let mut src = sample_table_with_qname("app", "orders_2024");
        src.partition_of = Some(po_default("app", "orders"));
        let target = sample_table_with_qname("app", "orders_2024");
        let changes = run_diff(&src, &target);
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::Table(TableChange::AttachPartition { .. }))),
            "got: {changes:?}"
        );
    }

    #[test]
    fn detects_detach_partition_when_source_drops_declaration() {
        let src = sample_table_with_qname("app", "orders_2024");
        let mut target = sample_table_with_qname("app", "orders_2024");
        target.partition_of = Some(po_default("app", "orders"));
        let changes = run_diff(&src, &target);
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::Table(TableChange::DetachPartition { .. }))),
            "got: {changes:?}"
        );
    }

    #[test]
    fn bounds_change_emits_detach_then_attach() {
        let mut src = sample_table_with_qname("app", "orders_2024");
        src.partition_of = Some(po_range("app", "orders", "10"));
        let mut target = sample_table_with_qname("app", "orders_2024");
        target.partition_of = Some(po_range("app", "orders", "20"));
        let changes = run_diff(&src, &target);
        let positions: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                Change::Table(TableChange::DetachPartition { .. }) => Some("detach"),
                Change::Table(TableChange::AttachPartition { .. }) => Some("attach"),
                _ => None,
            })
            .collect();
        assert_eq!(positions, vec!["detach", "attach"]);
    }

    #[test]
    fn parent_partition_by_change_errors() {
        let mut src = sample_table_with_qname("app", "orders");
        src.partition_by = Some(pb_list("region"));
        let mut target = sample_table_with_qname("app", "orders");
        target.partition_by = Some(pb_range("placed"));
        let err = try_diff(&src, &target).unwrap_err();
        assert!(err.contains("PARTITION BY"), "got: {err}");
    }
}
