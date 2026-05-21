//! Pure-function diff over the IR.
//!
//! `diff(target, source)` returns a [`ChangeSet`] describing every structural
//! difference between two [`Catalog`](crate::ir::catalog::Catalog) snapshots.
//! No SQL is generated here — that is the job of phases 5+ (planner, rewrites).
//!
//! ## Direction
//!
//! `diff(target, source)` describes the changes needed to take the *target*
//! catalog and turn it into the *source* catalog. In pgevolve terminology, the
//! "target" is the live database and the "source" is the desired state declared
//! in `*.sql` files; running the resulting plan converges target to source.

pub mod change;
pub mod changeset;
pub mod columns;
pub mod constraints;
pub mod destructiveness;
pub mod extensions;
pub mod indexes;
pub mod routines;
pub mod schemas;
pub mod sequence_op;
pub mod sequences;
pub mod table_op;
pub mod tables;
pub mod triggers;
pub mod types;
pub mod views;

pub use change::{
    Change, ChangeEntry, ExtensionChange, FunctionChange, MvChange, ProcedureChange, TableChange,
    TriggerChange, UserTypeChange, ViewChange,
};
pub use changeset::ChangeSet;
pub use destructiveness::Destructiveness;
pub use routines::{diff_functions, diff_procedures};
pub use sequence_op::{SequenceOp, SequenceOpEntry};
pub use table_op::{TableOp, TableOpEntry};

use crate::catalog::DriftReport;
use crate::ir::catalog::Catalog;

/// Compute the changes needed to converge `target` toward `source`.
///
/// `drift` captures any NOT VALID constraints or INVALID indexes found in the
/// live database (from [`crate::catalog::read_catalog`]). Pass
/// `&DriftReport::default()` when diffing two source-side catalogs (no live
/// database involved).
///
/// The returned [`ChangeSet`] is unordered; ordering / dependency analysis is
/// the planner's responsibility (phase 5).
pub fn diff(target: &Catalog, source: &Catalog, drift: &DriftReport) -> ChangeSet {
    let mut out = ChangeSet::new();

    // Emit recovery changes for any drift in the live database before the
    // structural diff, so the planner can schedule them appropriately.
    for (table_qname, constraint_name) in &drift.pending_validation {
        out.push(
            Change::ValidateConstraint {
                table: table_qname.clone(),
                constraint: constraint_name.clone(),
            },
            Destructiveness::Safe,
        );
    }
    for qname in &drift.invalid_indexes {
        out.push(
            Change::RecreateIndex {
                qname: qname.clone(),
            },
            Destructiveness::Safe,
        );
    }

    schemas::diff_schemas(target, source, &mut out);
    extensions::diff_extensions(&target.extensions, &source.extensions, &mut out);
    tables::diff_tables(target, source, &mut out);
    indexes::diff_indexes(target, source, &mut out);
    sequences::diff_sequences(target, source, &mut out);
    views::diff_views(&target.views, &source.views, &mut out);
    views::diff_materialized_views(
        &target.materialized_views,
        &source.materialized_views,
        &mut out,
    );
    types::diff_user_types(&target.types, &source.types, &mut out);
    routines::diff_functions(&target.functions, &source.functions, &mut out);
    routines::diff_procedures(&target.procedures, &source.procedures, &mut out);
    triggers::diff_triggers(&target.triggers, &source.triggers, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::DriftReport;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::ir::schema::Schema;
    use crate::ir::sequence::Sequence;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
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

    fn catalog_empty() -> Catalog {
        Catalog::empty()
    }

    fn catalog_with_one_table() -> Catalog {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![Constraint {
                qname: qn("app", "users_pkey"),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![id("id")],
                    include: vec![],
                },
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
                        partition_by: None,
            partition_of: None,
comment: Some("user accounts".into()),
        });
        c
    }

    fn catalog_with_indexes_and_fks() -> Catalog {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![col("id", ColumnType::BigInt, false)],
            constraints: vec![Constraint {
                qname: qn("app", "orgs_pkey"),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![id("id")],
                    include: vec![],
                },
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
                        partition_by: None,
            partition_of: None,
comment: None,
        });
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("org_id", ColumnType::BigInt, false),
                col("email", ColumnType::Varchar { len: Some(255) }, true),
            ],
            constraints: vec![
                Constraint {
                    qname: qn("app", "users_pkey"),
                    kind: ConstraintKind::PrimaryKey {
                        columns: vec![id("id")],
                        include: vec![],
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
                Constraint {
                    qname: qn("app", "users_org_fkey"),
                    kind: ConstraintKind::ForeignKey(ForeignKey {
                        columns: vec![id("org_id")],
                        referenced_table: qn("app", "orgs"),
                        referenced_columns: vec![id("id")],
                        on_update: ReferentialAction::NoAction,
                        on_delete: ReferentialAction::Cascade,
                        match_type: FkMatchType::Simple,
                    }),
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
            ],
                        partition_by: None,
            partition_of: None,
comment: None,
        });
        c.indexes.push(Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("email")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        });
        c.sequences.push(Sequence {
            qname: qn("app", "global_counter"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        });
        c
    }

    #[test]
    fn diff_against_empty_self_is_empty() {
        let c = Catalog::empty();
        assert!(diff(&c, &c, &DriftReport::default()).is_empty());
    }

    #[test]
    fn diff_against_single_table_self_is_empty() {
        assert!(
            diff(
                &catalog_with_one_table(),
                &catalog_with_one_table(),
                &DriftReport::default()
            )
            .is_empty()
        );
    }

    /// Property-style: `diff(c, c)` is empty for every hand-built catalog.
    /// Replace with a real `proptest` once the testkit `IRGenerator` lands in phase 11.
    #[test]
    fn diff_against_self_is_empty() {
        let catalogs = vec![
            catalog_empty(),
            catalog_with_one_table(),
            catalog_with_indexes_and_fks(),
        ];
        for c in &catalogs {
            assert!(
                diff(c, c, &DriftReport::default()).is_empty(),
                "diff(c, c) was not empty for catalog: {c:?}"
            );
        }
    }
}
