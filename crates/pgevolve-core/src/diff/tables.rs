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
#[allow(clippy::too_many_lines)]
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

                // ---- owner diff ----
                if let Some(source_owner) = &source_table.owner
                    && target_table.owner.as_ref() != Some(source_owner)
                {
                    let from = target_table.owner.clone().unwrap_or_else(|| {
                        Identifier::from_unquoted("__unknown_owner__").expect("literal is valid")
                    });
                    out.push(
                        Change::AlterObjectOwner(AlterObjectOwner {
                            kind: OwnerObjectKind::Table,
                            qname: (*qname).clone(),
                            signature: String::new(),
                            from,
                            to: source_owner.clone(),
                        }),
                        Destructiveness::Safe,
                    );
                }

                // ---- grant diff ----
                {
                    let object_label = format!("table {qname}");
                    let (to_add, to_revoke, unmanaged) =
                        diff_grants(&target_table.grants, &source_table.grants, managed_roles);
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
                                    kind: OwnerObjectKind::Table,
                                    signature: String::new(),
                                    grant: g,
                                },
                                Destructiveness::Safe,
                            );
                        }
                    }
                    for g in to_revoke {
                        if let Some(source_owner) = &source_table.owner {
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::partition::{
        BoundDatum, PartitionBounds, PartitionBy, PartitionColumn, PartitionColumnKind,
        PartitionOf, PartitionStrategy,
    };

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
