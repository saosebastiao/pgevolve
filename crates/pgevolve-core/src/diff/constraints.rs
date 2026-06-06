//! Constraint-level diffing inside an `AlterTable`.
//!
//! Pairs constraints by [`QualifiedName`]. v0.1 emits *only* `AddConstraint`,
//! `DropConstraint`, and `SetConstraintComment`: any change to the definition
//! of an existing constraint is expressed as a drop-then-add pair, since
//! Postgres does not support most `ALTER ... CONSTRAINT` operations.
//!
//! ## Destructiveness
//!
//! - `AddConstraint(...)`: `Safe`. The rewrite pass converts FK adds to
//!   `NOT VALID + VALIDATE` for online behavior.
//! - `DropConstraint`: conservatively `RequiresApproval` for every kind. We may
//!   relax CHECK / UNIQUE drops to `Safe` in a future pass once we have
//!   experience with users.
//! - `SetConstraintComment`: `Safe`.

use std::collections::BTreeMap;

use crate::identifier::QualifiedName;
use crate::ir::constraint::{Constraint, ConstraintKind};
use crate::ir::table::Table;

use super::destructiveness::Destructiveness;
use super::table_op::{TableOp, TableOpEntry};

/// Diff constraints in `target` against `source`, appending entries to `out`.
pub fn diff_constraints(target: &Table, source: &Table, out: &mut Vec<TableOpEntry>) {
    let target_map: BTreeMap<&QualifiedName, &Constraint> =
        target.constraints.iter().map(|c| (&c.qname, c)).collect();
    let source_map: BTreeMap<&QualifiedName, &Constraint> =
        source.constraints.iter().map(|c| (&c.qname, c)).collect();

    // Adds: present in source but not in target.
    for (qname, source_constraint) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(add_constraint_entry((*source_constraint).clone()));
        }
    }

    for (qname, target_constraint) in &target_map {
        match source_map.get(qname) {
            None => {
                // Pure drop.
                out.push(drop_constraint_entry(target_constraint));
            }
            Some(source_constraint) => {
                if definition_changed(target_constraint, source_constraint) {
                    // Express definition changes as drop + add.
                    out.push(drop_constraint_entry(target_constraint));
                    out.push(add_constraint_entry((*source_constraint).clone()));
                } else if target_constraint.comment != source_constraint.comment {
                    out.push(TableOpEntry {
                        op: TableOp::SetConstraintComment {
                            name: qname.name.clone(),
                            comment: source_constraint.comment.clone(),
                        },
                        destructiveness: Destructiveness::Safe,
                    });
                }
            }
        }
    }
}

const fn add_constraint_entry(c: Constraint) -> TableOpEntry {
    TableOpEntry {
        op: TableOp::AddConstraint(c),
        destructiveness: Destructiveness::Safe,
    }
}

fn drop_constraint_entry(c: &Constraint) -> TableOpEntry {
    TableOpEntry {
        op: TableOp::DropConstraint {
            name: c.qname.name.clone(),
        },
        destructiveness: Destructiveness::RequiresApproval {
            reason: format!("drops {} constraint {}", kind_label(&c.kind), c.qname.name),
        },
    }
}

const fn kind_label(k: &ConstraintKind) -> &'static str {
    match k {
        ConstraintKind::PrimaryKey { .. } => "primary key",
        ConstraintKind::Unique { .. } => "unique",
        ConstraintKind::ForeignKey(_) => "foreign key",
        ConstraintKind::Check { .. } => "check",
    }
}

/// True iff anything other than `comment` changed.
fn definition_changed(a: &Constraint, b: &Constraint) -> bool {
    a.kind != b.kind || a.deferrable != b.deferrable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{Deferrable, FkMatchType, ForeignKey, ReferentialAction};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn col(name: &str) -> Column {
        Column {
            name: id(name),
            ty: ColumnType::BigInt,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn tbl_with(constraints: Vec<Constraint>) -> Table {
        Table {
            qname: qn("users"),
            columns: vec![col("id"), col("org_id")],
            constraints,
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

    fn pk(name: &str, cols: &[&str]) -> Constraint {
        Constraint {
            qname: qn(name),
            kind: ConstraintKind::PrimaryKey {
                columns: cols.iter().map(|c| id(c)).collect(),
                include: vec![],
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn fk(name: &str, on_delete: ReferentialAction) -> Constraint {
        Constraint {
            qname: qn(name),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: vec![id("org_id")],
                referenced_table: qn("orgs"),
                referenced_columns: vec![id("id")],
                on_update: ReferentialAction::NoAction,
                on_delete,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn run(target: &Table, source: &Table) -> Vec<TableOpEntry> {
        let mut out = Vec::new();
        diff_constraints(target, source, &mut out);
        out
    }

    #[test]
    fn add_pk_is_safe() {
        let target = tbl_with(vec![]);
        let source = tbl_with(vec![pk("users_pkey", &["id"])]);
        let ops = run(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op, TableOp::AddConstraint(_)));
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn add_fk_is_safe() {
        let target = tbl_with(vec![]);
        let source = tbl_with(vec![fk("users_org_fkey", ReferentialAction::NoAction)]);
        let ops = run(&target, &source);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn drop_pk_requires_approval() {
        let target = tbl_with(vec![pk("users_pkey", &["id"])]);
        let source = tbl_with(vec![]);
        let ops = run(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op, TableOp::DropConstraint { .. }));
        assert!(ops[0].destructiveness.requires_approval());
        assert!(
            ops[0]
                .destructiveness
                .reason()
                .unwrap()
                .contains("primary key")
        );
    }

    #[test]
    fn drop_fk_requires_approval() {
        let target = tbl_with(vec![fk("users_org_fkey", ReferentialAction::NoAction)]);
        let source = tbl_with(vec![]);
        let ops = run(&target, &source);
        assert_eq!(ops.len(), 1);
        assert!(ops[0].destructiveness.requires_approval());
        assert!(
            ops[0]
                .destructiveness
                .reason()
                .unwrap()
                .contains("foreign key")
        );
    }

    #[test]
    fn pk_column_change_emits_drop_then_add() {
        let target = tbl_with(vec![pk("users_pkey", &["id"])]);
        let source = tbl_with(vec![pk("users_pkey", &["id", "org_id"])]);
        let ops = run(&target, &source);
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0].op, TableOp::DropConstraint { .. }));
        assert!(matches!(ops[1].op, TableOp::AddConstraint(_)));
    }

    #[test]
    fn fk_on_delete_change_emits_drop_then_add() {
        let target = tbl_with(vec![fk("fk1", ReferentialAction::NoAction)]);
        let source = tbl_with(vec![fk("fk1", ReferentialAction::Cascade)]);
        let ops = run(&target, &source);
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0].op, TableOp::DropConstraint { .. }));
        assert!(matches!(ops[1].op, TableOp::AddConstraint(_)));
    }

    #[test]
    fn comment_only_change_emits_set_constraint_comment() {
        let mut a = pk("users_pkey", &["id"]);
        a.comment = Some("v1".into());
        let mut b = pk("users_pkey", &["id"]);
        b.comment = Some("v2".into());
        let ops = run(&tbl_with(vec![a]), &tbl_with(vec![b]));
        assert_eq!(ops.len(), 1);
        match &ops[0].op {
            TableOp::SetConstraintComment { name, comment } => {
                assert_eq!(name, &id("users_pkey"));
                assert_eq!(comment.as_deref(), Some("v2"));
            }
            other => panic!("got {other:?}"),
        }
        assert_eq!(ops[0].destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn deferrable_change_emits_drop_then_add() {
        let a = pk("users_pkey", &["id"]);
        let b = Constraint {
            deferrable: Deferrable::Deferrable {
                initially_deferred: true,
            },
            ..a.clone()
        };
        let ops = run(&tbl_with(vec![a]), &tbl_with(vec![b]));
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn equal_constraints_emit_nothing() {
        let a = pk("users_pkey", &["id"]);
        let b = pk("users_pkey", &["id"]);
        let ops = run(&tbl_with(vec![a]), &tbl_with(vec![b]));
        assert!(ops.is_empty());
    }
}
