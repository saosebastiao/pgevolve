//! Table-level diffing.
//!
//! Pairs tables by [`QualifiedName`]. Existence differences emit
//! [`Change::CreateTable`] / [`Change::DropTable`]; pairs that are present in
//! both catalogs dispatch to [`super::columns::diff_columns`] and
//! [`super::constraints::diff_constraints`] and emit a single
//! [`Change::AlterTable`] containing every per-table operation.

use std::collections::BTreeMap;

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::table::Table;

use super::change::Change;
use super::changeset::ChangeSet;
use super::columns::diff_columns;
use super::constraints::diff_constraints;
use super::destructiveness::Destructiveness;
use super::table_op::TableOpEntry;

/// Diff tables in `target` against `source`, appending entries to `out`.
pub fn diff_tables(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
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
                diff_columns(target_table, source_table, &mut ops);
                diff_constraints(target_table, source_table, &mut ops);

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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;

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
            comment: None,
        }
    }

    fn users() -> Table {
        Table {
            qname: qn("users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![],
            comment: None,
        }
    }

    #[test]
    fn add_table_emits_create_safe() {
        let target = Catalog::empty();
        let mut source = Catalog::empty();
        source.tables.push(users());
        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs);
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
        diff_tables(&target, &source, &mut cs);
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
        assert!(entry
            .destructiveness
            .reason()
            .unwrap()
            .contains("app.users"));
    }

    #[test]
    fn equal_tables_emit_nothing() {
        let mut target = Catalog::empty();
        target.tables.push(users());
        let mut source = Catalog::empty();
        source.tables.push(users());
        let mut cs = ChangeSet::new();
        diff_tables(&target, &source, &mut cs);
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
        diff_tables(&target, &source, &mut cs);
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
}
