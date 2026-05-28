//! Diff user-defined types (enums, domains, composites).
//!
//! [`diff_user_types`] compares two slices of [`UserType`] — one from the live
//! catalog (`catalog`) and one from the declared source (`source`) — and
//! populates a [`ChangeSet`] with the minimal sequence of
//! [`UserTypeChange`] variants required to
//! converge the catalog toward the source.

use std::collections::{BTreeMap, BTreeSet};

use crate::diff::change::{Change, UserTypeChange};
use crate::diff::changeset::{ChangeSet, RevokeWithOwnerObservation, UnmanagedGrantObservation};
use crate::diff::destructiveness::Destructiveness;
use crate::diff::grants::diff_grants;
use crate::diff::owner_op::{AlterObjectOwner, OwnerObjectKind};
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::grant::GrantTarget;
use crate::ir::user_type::{CompositeAttribute, EnumValue, UserType, UserTypeKind};

/// Compute `UserType`-level changes needed to converge `catalog` toward `source`.
///
/// `catalog` is the live database snapshot; `source` is the desired state.
pub fn diff_user_types(
    catalog: &[UserType],
    source: &[UserType],
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    let cat: BTreeMap<_, _> = catalog.iter().map(|t| (t.qname.clone(), t)).collect();
    let src: BTreeMap<_, _> = source.iter().map(|t| (t.qname.clone(), t)).collect();

    let all_keys: BTreeSet<_> = cat.keys().chain(src.keys()).cloned().collect();

    for key in all_keys {
        match (cat.get(&key), src.get(&key)) {
            (None, Some(s)) => out.push(
                Change::UserType(UserTypeChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            (Some(_c), None) => out.push(
                Change::UserType(UserTypeChange::Drop(key.clone())),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!("drops user-defined type {key}"),
                },
            ),
            (Some(c), Some(s)) => {
                diff_same_qname(c, s, out);
                diff_type_owner_grants(c, s, out, managed_roles);
            }
            (None, None) => unreachable!(),
        }
    }
}

/// Diff owner and grants for a type pair.
fn diff_type_owner_grants(
    catalog: &UserType,
    source: &UserType,
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
    // Owner diff.
    if let Some(source_owner) = &source.owner
        && catalog.owner.as_ref() != Some(source_owner)
    {
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::UserType,
                id: crate::diff::owner_op::OwnedObjectId::Qualified(source.qname.clone()),
                signature: String::new(),
                from: catalog.owner.clone(),
                to: source_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Grant diff.
    let object_label = format!("type {}", source.qname);
    let (to_add, to_revoke, unmanaged) =
        diff_grants(&catalog.grants, &source.grants, managed_roles);
    for g in to_add {
        out.push(
            Change::GrantObjectPrivilege {
                qname: source.qname.clone(),
                kind: OwnerObjectKind::UserType,
                signature: String::new(),
                grant: g,
            },
            Destructiveness::Safe,
        );
    }
    for g in to_revoke {
        if let Some(source_owner) = &source.owner {
            out.revokes_with_owner.push(RevokeWithOwnerObservation {
                object_label: object_label.clone(),
                privilege_label: g.privilege.sql_keyword().into(),
                grantee: g.grantee.clone(),
                owner: source_owner.clone(),
            });
        }
        out.push(
            Change::RevokeObjectPrivilege {
                qname: source.qname.clone(),
                kind: OwnerObjectKind::UserType,
                signature: String::new(),
                grant: g,
            },
            Destructiveness::Safe,
        );
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

/// Diff two types with the same qualified name.
fn diff_same_qname(catalog: &UserType, source: &UserType, out: &mut ChangeSet) {
    // Detect kind mismatch FIRST. When a cascade replaces the type, the
    // recreated CREATE statement already carries the new comment, so emitting
    // a separate SetComment would either be redundant or — worse — schedule
    // a COMMENT ON TYPE after a DROP TYPE, which fails at execution.
    let kinds_match = matches!(
        (&catalog.kind, &source.kind),
        (UserTypeKind::Enum { .. }, UserTypeKind::Enum { .. })
            | (UserTypeKind::Domain { .. }, UserTypeKind::Domain { .. })
            | (
                UserTypeKind::Composite { .. },
                UserTypeKind::Composite { .. }
            )
    );

    // Comment change is always safe — but only emit when we're NOT replacing
    // the whole type (kind mismatch / domain base change / enum or composite
    // reorder all carry the new comment through the CREATE TYPE step).
    if kinds_match && catalog.comment != source.comment {
        out.push(
            Change::UserType(UserTypeChange::SetComment {
                qname: catalog.qname.clone(),
                comment: source.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    match (&catalog.kind, &source.kind) {
        (UserTypeKind::Enum { values: cat_vals }, UserTypeKind::Enum { values: src_vals }) => {
            diff_enum(&catalog.qname, cat_vals, src_vals, catalog, source, out);
        }
        (UserTypeKind::Domain { .. }, UserTypeKind::Domain { .. }) => {
            diff_domain(catalog, source, out);
        }
        (
            UserTypeKind::Composite {
                attributes: cat_attrs,
            },
            UserTypeKind::Composite {
                attributes: src_attrs,
            },
        ) => {
            diff_composite(&catalog.qname, cat_attrs, src_attrs, catalog, source, out);
        }
        // Kind mismatch — must replace with cascade (PG can't change type kind in-place).
        _ => {
            out.push(
                Change::UserType(UserTypeChange::ReplaceWithCascade {
                    source: source.clone(),
                    catalog: catalog.clone(),
                }),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!(
                        "type {} changed kind (requires DROP TYPE … CASCADE + CREATE TYPE)",
                        catalog.qname
                    ),
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Enum diffing
// ---------------------------------------------------------------------------

/// Returns `true` when the catalog→source enum transition can be expressed as
/// a series of `ALTER TYPE … ADD VALUE` and `ALTER TYPE … RENAME VALUE`
/// operations without requiring a full DROP + CREATE cascade.
///
/// The check passes iff:
/// - Every label present in both old and new lists appears at the same relative
///   position among shared labels (no reordering of preserved labels).
/// - Any label present in catalog but absent from source is matched 1:1 by a
///   new source label at the same list index (rename heuristic).
pub(crate) fn enum_can_alter_in_place(
    catalog_vals: &[EnumValue],
    source_vals: &[EnumValue],
) -> bool {
    let cat_names: Vec<&str> = catalog_vals.iter().map(|v| v.name.as_str()).collect();
    let src_names: Vec<&str> = source_vals.iter().map(|v| v.name.as_str()).collect();

    let cat_set: BTreeSet<&str> = cat_names.iter().copied().collect();
    let src_set: BTreeSet<&str> = src_names.iter().copied().collect();

    let removed: BTreeSet<&str> = cat_set.difference(&src_set).copied().collect();
    let added: BTreeSet<&str> = src_set.difference(&cat_set).copied().collect();

    // If any labels are removed but the count of added labels doesn't match,
    // we can't pair them as renames.
    if removed.len() != added.len() && !removed.is_empty() {
        return false;
    }

    // If same length and removed == added (each removed pairs with an add),
    // check that all paired positions hold (rename case) and preserved labels
    // maintain relative order.
    if !removed.is_empty() {
        // Rename heuristic: same list length, position-paired exclusive names.
        if cat_names.len() != src_names.len() {
            return false;
        }
        // Every removed label must be paired with an added label at the same index.
        for (i, cat_name) in cat_names.iter().enumerate() {
            if removed.contains(cat_name) {
                // The counterpart at position i in source must be an added label.
                if !added.contains(src_names[i]) {
                    return false;
                }
            }
        }
        // Preserved labels must appear in the same relative order.
        let preserved_in_cat: Vec<&str> = cat_names
            .iter()
            .copied()
            .filter(|n| !removed.contains(n))
            .collect();
        let preserved_in_src: Vec<&str> = src_names
            .iter()
            .copied()
            .filter(|n| !added.contains(n))
            .collect();
        if preserved_in_cat != preserved_in_src {
            return false;
        }
        return true;
    }

    // No removals: only additions are allowed. Check that preserved labels
    // appear in the same relative order in the source list.
    let preserved_in_src: Vec<&str> = src_names
        .iter()
        .copied()
        .filter(|n| cat_set.contains(n))
        .collect();

    // The preserved subsequence in source must match the catalog order exactly.
    preserved_in_src == cat_names
}

/// Diff two enum value lists and emit the appropriate changes.
fn diff_enum(
    qname: &QualifiedName,
    catalog_vals: &[EnumValue],
    source_vals: &[EnumValue],
    catalog_type: &UserType,
    source_type: &UserType,
    out: &mut ChangeSet,
) {
    // No change needed.
    if catalog_vals == source_vals {
        return;
    }

    if !enum_can_alter_in_place(catalog_vals, source_vals) {
        out.push(
            Change::UserType(UserTypeChange::ReplaceWithCascade {
                source: source_type.clone(),
                catalog: catalog_type.clone(),
            }),
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: format!(
                    "enum {qname} values changed in a way that requires DROP TYPE … CASCADE \
                     (value removed or order changed)"
                ),
            },
        );
        return;
    }

    let cat_names: Vec<&str> = catalog_vals.iter().map(|v| v.name.as_str()).collect();
    let src_names: Vec<&str> = source_vals.iter().map(|v| v.name.as_str()).collect();

    let cat_set: BTreeSet<&str> = cat_names.iter().copied().collect();
    let src_set: BTreeSet<&str> = src_names.iter().copied().collect();

    let removed: BTreeSet<&str> = cat_set.difference(&src_set).copied().collect();
    let added: BTreeSet<&str> = src_set.difference(&cat_set).copied().collect();

    // Rename detection: same-length lists, position-paired exclusive names.
    if !removed.is_empty() && !added.is_empty() && cat_names.len() == src_names.len() {
        for (i, cat_name) in cat_names.iter().enumerate() {
            if removed.contains(cat_name) {
                let src_name = src_names[i];
                if added.contains(src_name) {
                    out.push(
                        Change::UserType(UserTypeChange::EnumRenameValue {
                            qname: qname.clone(),
                            from: (*cat_name).to_string(),
                            to: src_name.to_string(),
                        }),
                        Destructiveness::Safe,
                    );
                }
            }
        }
        return;
    }

    // Only additions remain. For each new value, compute BEFORE/AFTER positioning.
    for (i, src_val) in source_vals.iter().enumerate() {
        if cat_set.contains(src_val.name.as_str()) {
            // Already in catalog — skip.
            continue;
        }

        // Determine positioning: find the nearest existing neighbor.
        // We prefer AFTER (the label before the new one in src that exists in catalog).
        // If none, use BEFORE (the first catalog label that comes after this position).
        let after: Option<String> = src_names[..i]
            .iter()
            .rev()
            .find(|n| cat_set.contains(*n))
            .map(|n| (*n).to_string());

        let before: Option<String> = if after.is_none() {
            src_names[i + 1..]
                .iter()
                .find(|n| cat_set.contains(*n))
                .map(|n| (*n).to_string())
        } else {
            None
        };

        out.push(
            Change::UserType(UserTypeChange::EnumAddValue {
                qname: qname.clone(),
                value: src_val.name.clone(),
                before,
                after,
            }),
            Destructiveness::Safe,
        );
    }
}

// ---------------------------------------------------------------------------
// Domain diffing
// ---------------------------------------------------------------------------

/// Diff two domain types and emit per-property changes.
#[allow(clippy::too_many_lines)] // exhaustive domain-property diff (base, nullability, default, checks, collation).
fn diff_domain(catalog: &UserType, source: &UserType, out: &mut ChangeSet) {
    let (cat_base, cat_nullable, cat_default, cat_checks, cat_collation) = match &catalog.kind {
        UserTypeKind::Domain {
            base,
            nullable,
            default,
            check_constraints,
            collation,
        } => (base, *nullable, default, check_constraints, collation),
        _ => unreachable!(),
    };
    let (src_base, src_nullable, src_default, src_checks, src_collation) = match &source.kind {
        UserTypeKind::Domain {
            base,
            nullable,
            default,
            check_constraints,
            collation,
        } => (base, *nullable, default, check_constraints, collation),
        _ => unreachable!(),
    };

    let qname = &catalog.qname;

    // Base type OR collation change requires full replacement — PG does not
    // support ALTER DOMAIN … SET DATA TYPE or ALTER DOMAIN … COLLATE.
    if cat_base != src_base || cat_collation != src_collation {
        out.push(
            Change::UserType(UserTypeChange::ReplaceWithCascade {
                source: source.clone(),
                catalog: catalog.clone(),
            }),
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: if cat_base == src_base {
                    format!(
                        "domain {qname} collation changed (requires DROP DOMAIN … CASCADE + CREATE DOMAIN)"
                    )
                } else {
                    format!(
                        "domain {qname} base type changed from {cat_base:?} to {src_base:?} \
                         (requires DROP DOMAIN … CASCADE + CREATE DOMAIN)"
                    )
                },
            },
        );
        return;
    }

    // NOT NULL toggle.
    if cat_nullable != src_nullable {
        out.push(
            Change::UserType(UserTypeChange::DomainSetNotNull {
                qname: qname.clone(),
                not_null: !src_nullable,
            }),
            Destructiveness::Safe,
        );
    }

    // DEFAULT change.
    let cat_default_expr = cat_default.as_ref().map(|d| &d.canonical_text);
    let src_default_expr = src_default.as_ref().map(|d| &d.canonical_text);
    if cat_default_expr != src_default_expr {
        out.push(
            Change::UserType(UserTypeChange::DomainSetDefault {
                qname: qname.clone(),
                default: src_default.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // CHECK constraints: pair by name.
    let cat_checks_map: BTreeMap<&Identifier, _> =
        cat_checks.iter().map(|c| (&c.name, c)).collect();
    let src_checks_map: BTreeMap<&Identifier, _> =
        src_checks.iter().map(|c| (&c.name, c)).collect();

    // Drop constraints removed or changed.
    for (name, cat_check) in &cat_checks_map {
        match src_checks_map.get(name) {
            None => {
                // Dropped.
                out.push(
                    Change::UserType(UserTypeChange::DomainDropCheck {
                        qname: qname.clone(),
                        name: (*name).clone(),
                    }),
                    Destructiveness::Safe, // loosening a constraint is safe
                );
            }
            Some(src_check) => {
                if cat_check.expression != src_check.expression {
                    // Expression changed: drop old + add new.
                    out.push(
                        Change::UserType(UserTypeChange::DomainDropCheck {
                            qname: qname.clone(),
                            name: (*name).clone(),
                        }),
                        Destructiveness::Safe,
                    );
                    out.push(
                        Change::UserType(UserTypeChange::DomainAddCheck {
                            qname: qname.clone(),
                            constraint: (*src_check).clone(),
                        }),
                        Destructiveness::RequiresApproval {
                            reason: format!(
                                "adding CHECK constraint {} to domain {qname} validates all existing values using this domain",
                                src_check.name.as_str(),
                            ),
                        },
                    );
                }
            }
        }
    }

    // Add new constraints. Adding a CHECK to a domain validates all existing
    // values that use the domain — table scan, may fail on bad rows. Treat as
    // approval-required.
    for (name, src_check) in &src_checks_map {
        if !cat_checks_map.contains_key(name) {
            out.push(
                Change::UserType(UserTypeChange::DomainAddCheck {
                    qname: qname.clone(),
                    constraint: (*src_check).clone(),
                }),
                Destructiveness::RequiresApproval {
                    reason: format!(
                        "adding CHECK constraint {} to domain {qname} validates all existing values using this domain",
                        src_check.name.as_str(),
                    ),
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Composite diffing
// ---------------------------------------------------------------------------

/// Returns `true` when the catalog→source composite transition can be expressed
/// as a sequence of `ALTER TYPE … ADD ATTRIBUTE` / `DROP ATTRIBUTE` /
/// `ALTER ATTRIBUTE … SET DATA TYPE` without requiring a cascade replacement.
///
/// The check fails if any attribute preserved in both sides has a different
/// relative order compared with the catalog.
pub(crate) fn composite_can_alter_in_place(
    catalog_attrs: &[CompositeAttribute],
    source_attrs: &[CompositeAttribute],
) -> bool {
    let src_names: BTreeSet<&Identifier> = source_attrs.iter().map(|a| &a.name).collect();

    // Preserved attributes: those present in both lists.
    let preserved_in_cat: Vec<&Identifier> = catalog_attrs
        .iter()
        .map(|a| &a.name)
        .filter(|n| src_names.contains(n))
        .collect();

    let cat_names: BTreeSet<&Identifier> = catalog_attrs.iter().map(|a| &a.name).collect();

    let preserved_in_src: Vec<&Identifier> = source_attrs
        .iter()
        .map(|a| &a.name)
        .filter(|n| cat_names.contains(n))
        .collect();

    // Preserved attributes must appear in the same relative order.
    preserved_in_cat == preserved_in_src
}

/// Diff two composite attribute lists and emit per-attribute changes.
fn diff_composite(
    qname: &QualifiedName,
    catalog_attrs: &[CompositeAttribute],
    source_attrs: &[CompositeAttribute],
    catalog_type: &UserType,
    source_type: &UserType,
    out: &mut ChangeSet,
) {
    if catalog_attrs == source_attrs {
        return;
    }

    if !composite_can_alter_in_place(catalog_attrs, source_attrs) {
        out.push(
            Change::UserType(UserTypeChange::ReplaceWithCascade {
                source: source_type.clone(),
                catalog: catalog_type.clone(),
            }),
            Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: format!(
                    "composite type {qname} attribute order changed (requires DROP TYPE … CASCADE \
                     + CREATE TYPE)"
                ),
            },
        );
        return;
    }

    let cat_map: BTreeMap<&Identifier, &CompositeAttribute> =
        catalog_attrs.iter().map(|a| (&a.name, a)).collect();
    let src_map: BTreeMap<&Identifier, &CompositeAttribute> =
        source_attrs.iter().map(|a| (&a.name, a)).collect();

    // 1. Drops first (attributes removed or absent from source).
    for cat_attr in catalog_attrs {
        if !src_map.contains_key(&cat_attr.name) {
            out.push(
                Change::UserType(UserTypeChange::CompositeDropAttribute {
                    qname: qname.clone(),
                    name: cat_attr.name.clone(),
                }),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!(
                        "drops attribute {} from composite type {qname}",
                        cat_attr.name
                    ),
                },
            );
        }
    }

    // 2. Type changes for preserved attributes.
    for src_attr in source_attrs {
        if let Some(cat_attr) = cat_map.get(&src_attr.name) {
            if cat_attr.ty != src_attr.ty {
                out.push(
                    Change::UserType(UserTypeChange::CompositeAlterAttributeType {
                        qname: qname.clone(),
                        attribute: src_attr.name.clone(),
                        new_type: src_attr.ty.clone(),
                    }),
                    Destructiveness::RequiresApproval {
                        reason: format!(
                            "changes type of attribute {} in composite type {qname} \
                             (may require table rewrite)",
                            src_attr.name
                        ),
                    },
                );
            }
            // Collation changes are treated as a type-alter (same destructiveness level).
            // If only collation changed but type didn't, we still emit CompositeAlterAttributeType
            // because PG's ALTER TYPE … ALTER ATTRIBUTE … SET DATA TYPE … COLLATE … covers it.
            // However, to keep changes minimal, only emit if something actually changed.
            // (ty check above already covers the primary case; collation-only changes
            //  need the same ALTER but with the existing type.)
            else if cat_attr.collation != src_attr.collation {
                out.push(
                    Change::UserType(UserTypeChange::CompositeAlterAttributeType {
                        qname: qname.clone(),
                        attribute: src_attr.name.clone(),
                        new_type: src_attr.ty.clone(),
                    }),
                    Destructiveness::RequiresApproval {
                        reason: format!(
                            "changes collation of attribute {} in composite type {qname}",
                            src_attr.name
                        ),
                    },
                );
            }
        }
    }

    // 3. Additions (attributes new in source).
    for src_attr in source_attrs {
        if !cat_map.contains_key(&src_attr.name) {
            out.push(
                Change::UserType(UserTypeChange::CompositeAddAttribute {
                    qname: qname.clone(),
                    attribute: src_attr.clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cloned_ref_to_slice_refs,
    clippy::redundant_clone
)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::diff::change::UserTypeChange;
    use crate::ir::column_type::ColumnType;
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::user_type::{
        CompositeAttribute, DomainCheck, EnumValue, UserType, UserTypeKind,
    };

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn ev(name: &str, order: f32) -> EnumValue {
        EnumValue {
            name: name.to_string(),
            sort_order: order,
        }
    }

    fn make_enum(qname: QualifiedName, values: &[&str]) -> UserType {
        UserType {
            qname,
            kind: UserTypeKind::Enum {
                values: values
                    .iter()
                    .enumerate()
                    .map(|(i, &s)| ev(s, (i + 1) as f32))
                    .collect(),
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_composite(qname: QualifiedName, attrs: Vec<CompositeAttribute>) -> UserType {
        UserType {
            qname,
            kind: UserTypeKind::Composite { attributes: attrs },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn attr(name: &str, ty: ColumnType) -> CompositeAttribute {
        CompositeAttribute {
            name: id(name),
            ty,
            collation: None,
        }
    }

    fn make_domain(qname: QualifiedName, nullable: bool) -> UserType {
        UserType {
            qname,
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn run(catalog: &[UserType], source: &[UserType]) -> Vec<Change> {
        let mut out = ChangeSet::new();
        diff_user_types(catalog, source, &mut out, &BTreeSet::new());
        out.entries.into_iter().map(|e| e.change).collect()
    }

    fn run_with_destructiveness(
        catalog: &[UserType],
        source: &[UserType],
    ) -> Vec<(Change, Destructiveness)> {
        let mut out = ChangeSet::new();
        diff_user_types(catalog, source, &mut out, &BTreeSet::new());
        out.entries
            .into_iter()
            .map(|e| (e.change, e.destructiveness))
            .collect()
    }

    // ---- Create / Drop ----

    #[test]
    fn type_only_in_source_is_create() {
        let src = make_enum(qn("app", "status"), &["a", "b"]);
        let changes = run(&[], &[src.clone()]);
        assert_eq!(changes.len(), 1);
        assert!(
            matches!(&changes[0], Change::UserType(UserTypeChange::Create(t)) if t.qname == src.qname)
        );
    }

    #[test]
    fn type_only_in_catalog_is_drop() {
        let cat = make_enum(qn("app", "status"), &["a", "b"]);
        let changes = run(&[cat.clone()], &[]);
        assert_eq!(changes.len(), 1);
        assert!(
            matches!(&changes[0], Change::UserType(UserTypeChange::Drop(q)) if *q == cat.qname)
        );
    }

    #[test]
    fn drop_is_data_loss_warning() {
        let cat = make_enum(qn("app", "status"), &["a"]);
        let pairs = run_with_destructiveness(&[cat], &[]);
        assert!(pairs[0].1.data_loss_risk());
    }

    // ---- Enum: add value at end ----

    #[test]
    fn enum_add_value_at_end() {
        let cat = make_enum(qn("app", "s"), &["a", "b"]);
        let src = make_enum(qn("app", "s"), &["a", "b", "c"]);
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::UserType(UserTypeChange::EnumAddValue {
                value,
                before,
                after,
                ..
            }) => {
                assert_eq!(value, "c");
                assert_eq!(after.as_deref(), Some("b"));
                assert!(before.is_none());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ---- Enum: add value before existing ----

    #[test]
    fn enum_add_value_before_existing() {
        let cat = make_enum(qn("app", "s"), &["b", "c"]);
        let src = make_enum(qn("app", "s"), &["a", "b", "c"]);
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::UserType(UserTypeChange::EnumAddValue {
                value,
                before,
                after,
                ..
            }) => {
                assert_eq!(value, "a");
                assert_eq!(before.as_deref(), Some("b"));
                assert!(after.is_none());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ---- Enum: drop value triggers cascade ----

    #[test]
    fn enum_drop_value_triggers_cascade() {
        let cat = make_enum(qn("app", "s"), &["a", "b", "c"]);
        let src = make_enum(qn("app", "s"), &["a", "c"]);
        let changes = run(&[cat.clone()], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::ReplaceWithCascade { .. })
        ));
    }

    // ---- Enum: reorder triggers cascade ----

    #[test]
    fn enum_reorder_triggers_cascade() {
        let cat = make_enum(qn("app", "s"), &["a", "b", "c"]);
        let src = make_enum(qn("app", "s"), &["c", "b", "a"]);
        let changes = run(&[cat.clone()], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::ReplaceWithCascade { .. })
        ));
    }

    // ---- Enum: rename detected ----

    #[test]
    fn enum_rename_value_detected() {
        let cat = make_enum(qn("app", "s"), &["a", "b", "c"]);
        let src = make_enum(qn("app", "s"), &["a", "x", "c"]);
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::UserType(UserTypeChange::EnumRenameValue { from, to, .. }) => {
                assert_eq!(from, "b");
                assert_eq!(to, "x");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ---- Composite: add attribute ----

    #[test]
    fn composite_add_attribute() {
        let cat = make_composite(qn("app", "addr"), vec![attr("a", ColumnType::Integer)]);
        let src = make_composite(
            qn("app", "addr"),
            vec![attr("a", ColumnType::Integer), attr("b", ColumnType::Text)],
        );
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::CompositeAddAttribute { .. })
        ));
    }

    // ---- Composite: reorder triggers cascade ----

    #[test]
    fn composite_reorder_triggers_cascade() {
        let cat = make_composite(
            qn("app", "addr"),
            vec![attr("a", ColumnType::Integer), attr("b", ColumnType::Text)],
        );
        let src = make_composite(
            qn("app", "addr"),
            vec![attr("b", ColumnType::Text), attr("a", ColumnType::Integer)],
        );
        let changes = run(&[cat.clone()], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::ReplaceWithCascade { .. })
        ));
    }

    // ---- Kind mismatch triggers cascade ----

    #[test]
    fn kind_mismatch_triggers_cascade() {
        let cat = make_enum(qn("app", "t"), &["a", "b"]);
        let src = make_composite(qn("app", "t"), vec![attr("x", ColumnType::Integer)]);
        let changes = run(&[cat.clone()], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::ReplaceWithCascade { .. })
        ));
    }

    // ---- Domain: set not null ----

    #[test]
    fn domain_set_not_null() {
        let cat = make_domain(qn("app", "d"), true); // nullable=true
        let src = make_domain(qn("app", "d"), false); // nullable=false → NOT NULL
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::UserType(UserTypeChange::DomainSetNotNull { not_null, .. }) => {
                assert!(*not_null, "expected not_null=true");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ---- Domain: drop not null ----

    #[test]
    fn domain_drop_not_null() {
        let cat = make_domain(qn("app", "d"), false); // NOT NULL
        let src = make_domain(qn("app", "d"), true); // nullable → DROP NOT NULL
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::UserType(UserTypeChange::DomainSetNotNull { not_null, .. }) => {
                assert!(!*not_null, "expected not_null=false");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ---- Domain: add check ----

    #[test]
    fn domain_add_check() {
        let qname = qn("app", "d");
        let cat = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let check = DomainCheck {
            name: id("positive"),
            expression: NormalizedExpr::from_text("VALUE > 0"),
        };
        let src = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![check.clone()],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::DomainAddCheck { .. })
        ));
    }

    // ---- Domain: drop check ----

    #[test]
    fn domain_drop_check() {
        let qname = qn("app", "d");
        let check = DomainCheck {
            name: id("positive"),
            expression: NormalizedExpr::from_text("VALUE > 0"),
        };
        let cat = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![check.clone()],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let src = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::DomainDropCheck { .. })
        ));
    }

    // ---- Domain: CHECK expression change ----

    #[test]
    fn domain_replace_check_expression_emits_drop_then_add() {
        let qname = qn("app", "d");
        let old_check = DomainCheck {
            name: id("positive"),
            expression: NormalizedExpr::from_text("VALUE > 0"),
        };
        let new_check = DomainCheck {
            name: id("positive"),
            expression: NormalizedExpr::from_text("VALUE > 10"),
        };
        let cat = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![old_check],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let src = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![new_check],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 2, "expected drop then add, got {changes:?}");
        assert!(
            matches!(
                &changes[0],
                Change::UserType(UserTypeChange::DomainDropCheck { .. })
            ),
            "first change must be drop, got {:?}",
            changes[0],
        );
        assert!(
            matches!(
                &changes[1],
                Change::UserType(UserTypeChange::DomainAddCheck { .. })
            ),
            "second change must be add, got {:?}",
            changes[1],
        );
    }

    // ---- Domain: set default ----

    #[test]
    fn domain_set_default() {
        let qname = qn("app", "d");
        let cat = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let src = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: Some(NormalizedExpr::from_text("0")),
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::DomainSetDefault { .. })
        ));
    }

    // ---- Domain: base type change triggers cascade ----

    #[test]
    fn domain_base_type_change_triggers_cascade() {
        let qname = qn("app", "d");
        let cat = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let src = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::BigInt,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::ReplaceWithCascade { .. })
        ));
    }

    // ---- Comment change ----

    #[test]
    fn comment_change_emits_set_comment() {
        let mut cat = make_enum(qn("app", "s"), &["a"]);
        cat.comment = Some("old comment".into());
        let mut src = make_enum(qn("app", "s"), &["a"]);
        src.comment = Some("new comment".into());
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::SetComment { .. })
        ));
    }

    // ---- Identical types emit no changes ----

    #[test]
    fn identical_enum_emits_no_changes() {
        let t = make_enum(qn("app", "s"), &["a", "b", "c"]);
        let changes = run(&[t.clone()], &[t]);
        assert!(changes.is_empty());
    }

    #[test]
    fn identical_composite_emits_no_changes() {
        let t = make_composite(
            qn("app", "addr"),
            vec![attr("a", ColumnType::Integer), attr("b", ColumnType::Text)],
        );
        let changes = run(&[t.clone()], &[t]);
        assert!(changes.is_empty());
    }

    // ---- Composite: drop attribute ----

    #[test]
    fn composite_drop_attribute() {
        let cat = make_composite(
            qn("app", "addr"),
            vec![attr("a", ColumnType::Integer), attr("b", ColumnType::Text)],
        );
        let src = make_composite(qn("app", "addr"), vec![attr("a", ColumnType::Integer)]);
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::CompositeDropAttribute { .. })
        ));
    }

    // ---- Composite: type change ----

    #[test]
    fn composite_attribute_type_change() {
        let cat = make_composite(qn("app", "addr"), vec![attr("a", ColumnType::Integer)]);
        let src = make_composite(qn("app", "addr"), vec![attr("a", ColumnType::BigInt)]);
        let changes = run(&[cat], &[src]);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            Change::UserType(UserTypeChange::CompositeAlterAttributeType { .. })
        ));
    }

    // ---- Destructiveness classification spot-checks ----

    #[test]
    fn composite_drop_attr_is_data_loss() {
        let cat = make_composite(
            qn("app", "addr"),
            vec![attr("a", ColumnType::Integer), attr("b", ColumnType::Text)],
        );
        let src = make_composite(qn("app", "addr"), vec![attr("a", ColumnType::Integer)]);
        let pairs = run_with_destructiveness(&[cat], &[src]);
        assert!(pairs[0].1.data_loss_risk());
    }

    #[test]
    fn composite_type_change_requires_approval() {
        let cat = make_composite(qn("app", "addr"), vec![attr("a", ColumnType::Integer)]);
        let src = make_composite(qn("app", "addr"), vec![attr("a", ColumnType::BigInt)]);
        let pairs = run_with_destructiveness(&[cat], &[src]);
        assert!(pairs[0].1.requires_approval());
        assert!(!pairs[0].1.data_loss_risk());
    }

    #[test]
    fn domain_drop_check_is_safe() {
        let qname = qn("app", "d");
        let check = DomainCheck {
            name: id("positive"),
            expression: NormalizedExpr::from_text("VALUE > 0"),
        };
        let cat = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![check],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let src = UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        };
        let pairs = run_with_destructiveness(&[cat], &[src]);
        assert!(!pairs[0].1.requires_approval());
    }
}
