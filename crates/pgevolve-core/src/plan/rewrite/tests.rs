//! End-to-end tests for the rewrite pass. Exercises the public
//! `rewrite()` / `rewrite_with_source()` entry points via the
//! `OrderedChangeSet → Vec<RawStep>` contract.

use super::*;
use crate::diff::change::Change;
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::diff::sequence_op::{SequenceOp, SequenceOpEntry};
use crate::diff::table_op::{TableOp, TableOpEntry};
use crate::identifier::Identifier;
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
use crate::plan::ordered::{DeferredFkAdd, OrderedChangeSet};
use crate::plan::ordering::order;
use crate::plan::policy::PlannerPolicy;
use crate::plan::raw_step::{StepKind, TransactionConstraint};

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
        storage: None,
        compression: None,
        comment: None,
    }
}

fn pk(name: &str, cols: &[&str]) -> Constraint {
    Constraint {
        qname: qn("app", name),
        kind: ConstraintKind::PrimaryKey {
            columns: cols.iter().map(|c| id(c)).collect(),
            include: vec![],
        },
        deferrable: Deferrable::NotDeferrable,
        comment: None,
    }
}

fn make_index(name: &str, table: QualifiedName, unique: bool) -> Index {
    Index {
        qname: qn("app", name),
        on: IndexParent::Table(table),
        method: IndexMethod::BTree,
        columns: vec![IndexColumn {
            expr: IndexColumnExpr::Column(id("id")),
            collation: None,
            opclass: None,
            sort_order: SortOrder::Asc,
            nulls_order: NullsOrder::NullsLast,
        }],
        include: vec![],
        unique,
        nulls_not_distinct: false,
        predicate: None,
        tablespace: None,
        comment: None,
        storage: crate::ir::reloptions::IndexStorageOptions::default(),
    }
}

fn rewrite_with_default(target: &Catalog, source: &Catalog, changes: ChangeSet) -> Vec<RawStep> {
    let policy = PlannerPolicy::default();
    let ordered = order(target, source, changes, &policy).unwrap();
    rewrite(ordered, target, &policy)
}

fn rewrite_changeset_only(changes: ChangeSet) -> Vec<RawStep> {
    rewrite(
        OrderedChangeSet {
            creates_and_adds: changes.entries,
            modifies: vec![],
            drops: vec![],
            deferred_fks: vec![],
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    )
}

#[test]
fn empty_ordered_set_yields_no_steps() {
    let policy = PlannerPolicy::default();
    let steps = rewrite(OrderedChangeSet::default(), &Catalog::empty(), &policy);
    assert!(steps.is_empty());
}

#[test]
fn create_schema_emits_single_step() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::CreateSchema(Schema::new(id("app"))),
        Destructiveness::Safe,
    );
    let mut source = Catalog::empty();
    source.schemas.push(Schema::new(id("app")));
    let steps = rewrite_with_default(&Catalog::empty(), &source, cs);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::CreateSchema);
    assert_eq!(steps[0].sql, "CREATE SCHEMA app;");
    assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
    assert!(!steps[0].destructive);
}

#[test]
fn create_schema_with_comment_emits_two_steps() {
    let s = Schema {
        name: id("app"),
        comment: Some("the app".into()),
        owner: None,
        grants: vec![],
    };
    let mut cs = ChangeSet::new();
    cs.push(Change::CreateSchema(s), Destructiveness::Safe);
    let steps = rewrite_changeset_only(cs);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].kind, StepKind::CreateSchema);
    assert_eq!(steps[1].kind, StepKind::AlterSchemaComment);
    assert_eq!(steps[1].sql, "COMMENT ON SCHEMA app IS 'the app';");
}

#[test]
fn drop_schema_marks_destructive() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::DropSchema(id("legacy")),
        Destructiveness::RequiresApproval {
            reason: "drop schema".into(),
        },
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::DropSchema);
    assert!(steps[0].destructive);
    assert_eq!(steps[0].sql, "DROP SCHEMA legacy;");
}

#[test]
fn alter_schema_emits_comment_step() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterSchema {
            name: id("app"),
            comment: Some("v2".into()),
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AlterSchemaComment);
    assert_eq!(steps[0].sql, "COMMENT ON SCHEMA app IS 'v2';");
}

#[test]
fn create_table_emits_full_create_with_columns_and_pk() {
    let t = Table {
        qname: qn("app", "users"),
        columns: vec![
            col("id", ColumnType::BigInt, false),
            col("email", ColumnType::Text, true),
        ],
        constraints: vec![pk("users_pkey", &["id"])],
        partition_by: None,
        partition_of: None,
        comment: None,
        owner: None,
        grants: vec![],
        rls_enabled: false,
        rls_forced: false,
        policies: vec![],
        storage: crate::ir::reloptions::TableStorageOptions::default(),
    };
    let mut cs = ChangeSet::new();
    cs.push(Change::CreateTable(t), Destructiveness::Safe);
    let steps = rewrite_changeset_only(cs);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::CreateTable);
    assert!(steps[0].sql.starts_with("CREATE TABLE app.users ("));
    assert!(steps[0].sql.contains("id bigint NOT NULL"));
    assert!(steps[0].sql.contains("email text"));
    assert!(
        steps[0]
            .sql
            .contains("CONSTRAINT users_pkey PRIMARY KEY (id)")
    );
}

#[test]
fn drop_table_marks_destructive() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::DropTable {
            qname: qn("app", "old"),
            row_count_estimate: Some(100),
        },
        Destructiveness::RequiresApprovalAndDataLossWarning {
            reason: "drops table".into(),
        },
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::DropTable);
    assert!(steps[0].destructive);
    assert_eq!(steps[0].sql, "DROP TABLE app.old;");
}

#[test]
fn create_index_emits_basic_create() {
    // Index on a fresh table → not eligible for concurrent rewrite.
    let mut cs = ChangeSet::new();
    cs.push(
        Change::CreateIndex(make_index("users_idx", qn("app", "users"), false)),
        Destructiveness::Safe,
    );
    let steps = rewrite_changeset_only(cs);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::CreateIndex);
    assert!(
        steps[0]
            .sql
            .starts_with("CREATE INDEX users_idx ON app.users USING btree (id)")
    );
    assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
}

#[test]
fn drop_index_emits_plain_drop_in_default_path() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::DropIndex(qn("app", "users_idx")),
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::DropIndex);
    assert_eq!(steps[0].sql, "DROP INDEX app.users_idx;");
}

#[test]
fn replace_index_emits_drop_then_create() {
    let from = make_index("users_idx", qn("app", "users"), false);
    let to = make_index("users_idx", qn("app", "users"), true);
    let mut cs = ChangeSet::new();
    cs.push(Change::ReplaceIndex { from, to }, Destructiveness::Safe);
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].kind, StepKind::DropIndex);
    assert_eq!(steps[1].kind, StepKind::CreateIndex);
    assert!(steps[1].sql.contains("UNIQUE INDEX"));
}

#[test]
fn create_sequence_emits_full_create() {
    let s = Sequence {
        qname: qn("app", "id_seq"),
        data_type: ColumnType::BigInt,
        start: 1,
        increment: 1,
        min_value: None,
        max_value: None,
        cache: 1,
        cycle: false,
        owned_by: None,
        comment: None,
        owner: None,
        grants: vec![],
    };
    let mut cs = ChangeSet::new();
    cs.push(Change::CreateSequence(s), Destructiveness::Safe);
    let steps = rewrite_changeset_only(cs);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::CreateSequence);
    assert!(
        steps[0]
            .sql
            .starts_with("CREATE SEQUENCE app.id_seq AS bigint")
    );
    assert!(steps[0].sql.contains("INCREMENT BY 1"));
    assert!(steps[0].sql.contains("START WITH 1"));
    assert!(steps[0].sql.contains("NO CYCLE"));
}

#[test]
fn alter_table_add_column_emits_step() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddColumn(col("email", ColumnType::Text, true)),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddColumn);
    assert_eq!(steps[0].sql, "ALTER TABLE app.users ADD COLUMN email text;");
}

#[test]
fn alter_table_drop_column_marks_destructive() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::DropColumn {
                    name: id("email"),
                    is_populated: true,
                },
                destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: "drop col".into(),
                },
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert!(steps[0].destructive);
    assert_eq!(steps[0].sql, "ALTER TABLE app.users DROP COLUMN email;");
}

#[test]
fn alter_column_type_emits_using_clause_when_present() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AlterColumnType {
                    name: id("count"),
                    from: ColumnType::Integer,
                    to: ColumnType::BigInt,
                    using: Some(crate::ir::default_expr::NormalizedExpr::from_text(
                        "count::bigint",
                    )),
                },
                destructiveness: Destructiveness::RequiresApproval {
                    reason: "type change".into(),
                },
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(
        steps[0].sql,
        "ALTER TABLE app.users ALTER COLUMN count TYPE bigint USING count::bigint;",
    );
}

#[test]
fn set_column_nullable_distinguishes_directions() {
    for (nullable, expected) in [
        (true, "ALTER TABLE app.users ALTER COLUMN c DROP NOT NULL;"),
        (false, "ALTER TABLE app.users ALTER COLUMN c SET NOT NULL;"),
    ] {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "users"),
                ops: vec![TableOpEntry {
                    op: TableOp::SetColumnNullable {
                        name: id("c"),
                        nullable,
                    },
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        let steps = rewrite(
            OrderedChangeSet {
                modifies: cs.entries,
                ..Default::default()
            },
            &Catalog::empty(),
            &PlannerPolicy::default(),
        );
        assert_eq!(steps[0].sql, expected);
    }
}

#[test]
fn add_constraint_emits_single_step_in_default_path() {
    // Non-FK, non-CHECK constraint → no rewrite ever applies, even with
    // online policy. (Unique constraints stay simple.)
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(Constraint {
                    qname: qn("app", "users_email_uq"),
                    kind: ConstraintKind::Unique {
                        columns: vec![id("email")],
                        include: vec![],
                        nulls_distinct: true,
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                }),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddConstraint);
    assert!(steps[0].sql.contains("UNIQUE"));
}

#[test]
fn drop_constraint_emits_step() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::DropConstraint {
                    name: id("users_email_uq"),
                },
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::DropConstraint);
    assert_eq!(
        steps[0].sql,
        "ALTER TABLE app.users DROP CONSTRAINT users_email_uq;",
    );
}

#[test]
fn alter_sequence_set_increment_emits_step() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterSequence {
            qname: qn("app", "s1"),
            ops: vec![SequenceOpEntry {
                op: SequenceOp::SetIncrement(2),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::AlterSequence);
    assert_eq!(steps[0].sql, "ALTER SEQUENCE app.s1 INCREMENT BY 2;");
}

#[test]
fn alter_sequence_set_owned_by_renders_qualified_owner() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterSequence {
            qname: qn("app", "s1"),
            ops: vec![SequenceOpEntry {
                op: SequenceOp::SetOwnedBy(Some(crate::ir::sequence::SequenceOwner {
                    table: qn("app", "users"),
                    column: id("id"),
                })),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].sql, "ALTER SEQUENCE app.s1 OWNED BY app.users.id;");
}

#[test]
fn drop_sequence_marks_destructive() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::DropSequence(qn("app", "s1")),
        Destructiveness::RequiresApproval {
            reason: "drop seq".into(),
        },
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert!(steps[0].destructive);
    assert_eq!(steps[0].sql, "DROP SEQUENCE app.s1;");
}

#[test]
fn deferred_fk_emits_alter_table_add_constraint() {
    let fk = DeferredFkAdd {
        table: qn("app", "a"),
        constraint: Constraint {
            qname: qn("app", "a_b_fk"),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: vec![id("ref_id")],
                referenced_table: qn("app", "b"),
                referenced_columns: vec![id("id")],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::NoAction,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        },
    };
    let steps = rewrite(
        OrderedChangeSet {
            deferred_fks: vec![fk],
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddConstraint);
    assert!(
        steps[0]
            .sql
            .contains("ADD CONSTRAINT a_b_fk FOREIGN KEY (ref_id) REFERENCES app.b (id)")
    );
}

// ---- concurrent-index rewrite (Task 6.4) ----

#[test]
fn create_index_on_existing_table_rewrites_to_concurrent() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
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
    });

    let idx = make_index("users_idx", qn("app", "users"), false);
    let mut cs = ChangeSet::new();
    cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

    let steps = rewrite(
        OrderedChangeSet {
            creates_and_adds: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::CreateIndexConcurrent);
    assert_eq!(
        steps[0].transactional,
        TransactionConstraint::OutsideTransaction,
    );
    assert!(steps[0].sql.contains("CONCURRENTLY"));
}

#[test]
fn create_index_on_new_table_stays_inline() {
    // Empty target ⇒ users is being created in this plan ⇒ no concurrent rewrite.
    let idx = make_index("users_idx", qn("app", "users"), false);
    let mut cs = ChangeSet::new();
    cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

    let steps = rewrite(
        OrderedChangeSet {
            creates_and_adds: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::CreateIndex);
    assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
    assert!(!steps[0].sql.contains("CONCURRENTLY"));
}

#[test]
fn unique_create_index_does_not_rewrite_to_concurrent() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
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
    });

    let idx = make_index("users_email_idx", qn("app", "users"), true);
    let mut cs = ChangeSet::new();
    cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

    let steps = rewrite(
        OrderedChangeSet {
            creates_and_adds: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::CreateIndex);
    assert!(steps[0].sql.contains("UNIQUE INDEX"));
    assert!(!steps[0].sql.contains("CONCURRENTLY"));
}

#[test]
fn atomic_policy_disables_concurrent_index_rewrite() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
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
    });

    let idx = make_index("users_idx", qn("app", "users"), false);
    let mut cs = ChangeSet::new();
    cs.push(Change::CreateIndex(idx), Destructiveness::Safe);

    let policy = PlannerPolicy {
        strategy: crate::plan::policy::Strategy::Atomic,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(
        OrderedChangeSet {
            creates_and_adds: cs.entries,
            ..Default::default()
        },
        &target,
        &policy,
    );
    assert_eq!(steps[0].kind, StepKind::CreateIndex);
    assert!(!steps[0].sql.contains("CONCURRENTLY"));
}

#[test]
fn drop_index_on_existing_non_unique_rewrites_to_concurrent() {
    let mut target = Catalog::empty();
    target
        .indexes
        .push(make_index("users_idx", qn("app", "users"), false));

    let mut cs = ChangeSet::new();
    cs.push(
        Change::DropIndex(qn("app", "users_idx")),
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::DropIndexConcurrent);
    assert_eq!(
        steps[0].transactional,
        TransactionConstraint::OutsideTransaction
    );
}

#[test]
fn drop_unique_index_stays_inline() {
    let mut target = Catalog::empty();
    target
        .indexes
        .push(make_index("users_idx", qn("app", "users"), true));

    let mut cs = ChangeSet::new();
    cs.push(
        Change::DropIndex(qn("app", "users_idx")),
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::DropIndex);
    assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
}

#[test]
fn drop_index_unknown_in_target_stays_inline() {
    // If the index isn't in the target catalog, we can't tell whether
    // it's unique. Default to the safe inline form.
    let mut cs = ChangeSet::new();
    cs.push(
        Change::DropIndex(qn("app", "users_idx")),
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps[0].kind, StepKind::DropIndex);
}

// ---- FK NOT VALID + VALIDATE rewrite (Task 6.5) ----

fn fk(name: &str, ref_table: QualifiedName) -> Constraint {
    Constraint {
        qname: qn("app", name),
        kind: ConstraintKind::ForeignKey(ForeignKey {
            columns: vec![id("ref_id")],
            referenced_table: ref_table,
            referenced_columns: vec![id("id")],
            on_update: ReferentialAction::NoAction,
            on_delete: ReferentialAction::NoAction,
            match_type: FkMatchType::Simple,
        }),
        deferrable: Deferrable::NotDeferrable,
        comment: None,
    }
}

#[test]
fn add_fk_on_existing_table_emits_two_steps() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![
            col("id", ColumnType::BigInt, false),
            col("ref_id", ColumnType::BigInt, false),
        ],
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
    });
    target.tables.push(Table {
        qname: qn("app", "orgs"),
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
    });

    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(fk("users_orgs_fk", qn("app", "orgs"))),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].kind, StepKind::AddConstraintNotValid);
    assert!(steps[0].sql.contains("NOT VALID"));
    assert_eq!(steps[1].kind, StepKind::ValidateConstraint);
    assert_eq!(
        steps[1].sql,
        "ALTER TABLE app.users VALIDATE CONSTRAINT users_orgs_fk;",
    );
}

#[test]
fn add_fk_on_new_table_via_alter_stays_inline_when_target_missing() {
    // Target is empty, so users does not yet exist ⇒ no rewrite.
    // (In practice an FK on a brand-new table would ride inside the
    // CREATE TABLE — we exercise the alter-path edge case here.)
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(fk("users_orgs_fk", qn("app", "orgs"))),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddConstraint);
}

#[test]
fn add_fk_with_atomic_policy_stays_inline() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![
            col("id", ColumnType::BigInt, false),
            col("ref_id", ColumnType::BigInt, false),
        ],
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
    });

    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(fk("users_orgs_fk", qn("app", "orgs"))),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let policy = PlannerPolicy {
        strategy: crate::plan::policy::Strategy::Atomic,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &policy,
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddConstraint);
}

#[test]
fn add_unique_constraint_on_existing_table_does_not_trigger_fk_rewrite() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![
            col("id", ColumnType::BigInt, false),
            col("email", ColumnType::Text, true),
        ],
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
    });

    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(Constraint {
                    qname: qn("app", "users_email_uq"),
                    kind: ConstraintKind::Unique {
                        columns: vec![id("email")],
                        include: vec![],
                        nulls_distinct: true,
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                }),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddConstraint);
}

// ---- CHECK NOT VALID + VALIDATE rewrite (Task 6.6) ----

fn check(name: &str, expr: &str) -> Constraint {
    Constraint {
        qname: qn("app", name),
        kind: ConstraintKind::Check {
            expression: crate::ir::default_expr::NormalizedExpr::from_text(expr),
            no_inherit: false,
        },
        deferrable: Deferrable::NotDeferrable,
        comment: None,
    }
}

#[test]
fn add_check_on_existing_table_emits_two_steps() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![col("age", ColumnType::Integer, true)],
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
    });

    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(check("users_age_chk", "age >= 0")),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].kind, StepKind::AddConstraintNotValid);
    assert!(steps[0].sql.contains("CHECK (age >= 0)"));
    assert!(steps[0].sql.contains("NOT VALID"));
    assert_eq!(steps[1].kind, StepKind::ValidateConstraint);
}

#[test]
fn add_check_on_new_table_via_alter_stays_inline_when_target_missing() {
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(check("users_age_chk", "age >= 0")),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddConstraint);
}

#[test]
fn add_check_with_atomic_policy_stays_inline() {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![col("age", ColumnType::Integer, true)],
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
    });

    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::AddConstraint(check("users_age_chk", "age >= 0")),
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let policy = PlannerPolicy {
        strategy: crate::plan::policy::Strategy::Atomic,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &policy,
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AddConstraint);
}

// ---- SET NOT NULL via CHECK pattern (Task 6.7) ----

fn target_with_users_and_email() -> Catalog {
    let mut target = Catalog::empty();
    target.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![
            col("id", ColumnType::BigInt, false),
            col("email", ColumnType::Text, true),
        ],
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
    });
    target
}

#[test]
fn set_not_null_on_existing_column_emits_four_steps() {
    let target = target_with_users_and_email();
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::SetColumnNullable {
                    name: id("email"),
                    nullable: false,
                },
                destructiveness: Destructiveness::RequiresApproval {
                    reason: "set not null".into(),
                },
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 4);
    assert_eq!(steps[0].kind, StepKind::AddCheckForNotNull);
    assert!(steps[0].sql.contains("__pgevolve_chk_email"));
    assert!(steps[0].sql.contains("CHECK (email IS NOT NULL)"));
    assert!(steps[0].sql.contains("NOT VALID"));
    assert_eq!(steps[1].kind, StepKind::ValidateConstraint);
    assert!(steps[1].sql.contains("__pgevolve_chk_email"));
    assert_eq!(steps[2].kind, StepKind::SetColumnNullable);
    assert_eq!(
        steps[2].sql,
        "ALTER TABLE app.users ALTER COLUMN email SET NOT NULL;",
    );
    assert_eq!(steps[3].kind, StepKind::DropConstraint);
    assert!(steps[3].sql.contains("__pgevolve_chk_email"));
}

#[test]
fn set_not_null_on_unknown_column_stays_single_step() {
    // email isn't in the (empty) target ⇒ this is a new column path; the
    // existing AddColumn would carry NOT NULL inline, but if the differ
    // happens to emit a bare SetColumnNullable it should remain one step.
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::SetColumnNullable {
                    name: id("email"),
                    nullable: false,
                },
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::SetColumnNullable);
}

#[test]
fn set_not_null_with_atomic_policy_stays_single_step() {
    let target = target_with_users_and_email();
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::SetColumnNullable {
                    name: id("email"),
                    nullable: false,
                },
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let policy = PlannerPolicy {
        strategy: crate::plan::policy::Strategy::Atomic,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &policy,
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::SetColumnNullable);
}

#[test]
fn drop_not_null_is_always_single_step() {
    // Going from NOT NULL to nullable never needs the CHECK pattern.
    let target = target_with_users_and_email();
    let mut cs = ChangeSet::new();
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![TableOpEntry {
                op: TableOp::SetColumnNullable {
                    name: id("email"),
                    nullable: true,
                },
                destructiveness: Destructiveness::Safe,
            }],
        },
        Destructiveness::Safe,
    );
    let steps = rewrite(
        OrderedChangeSet {
            modifies: cs.entries,
            ..Default::default()
        },
        &target,
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::SetColumnNullable);
    assert!(steps[0].sql.contains("DROP NOT NULL"));
}

#[test]
fn rewrite_preserves_bucket_order_creates_modifies_drops() {
    let mut source = Catalog::empty();
    source.schemas.push(Schema::new(id("app")));
    source.tables.push(Table {
        qname: qn("app", "users"),
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
    });

    let mut cs = ChangeSet::new();
    cs.push(
        Change::CreateSchema(Schema::new(id("app"))),
        Destructiveness::Safe,
    );
    cs.push(
        Change::CreateTable(source.tables[0].clone()),
        Destructiveness::Safe,
    );
    cs.push(
        Change::AlterTable {
            qname: qn("app", "users"),
            ops: vec![],
        },
        Destructiveness::Safe,
    );
    cs.push(
        Change::DropSchema(id("legacy")),
        Destructiveness::RequiresApproval { reason: "x".into() },
    );

    let mut target = Catalog::empty();
    target.schemas.push(Schema::new(id("legacy")));
    let steps = rewrite_with_default(&target, &source, cs);
    let kinds: Vec<StepKind> = steps.iter().map(|s| s.kind).collect();
    // Creates first (schema, table), then modifies (alter table — produces no
    // child ops for empty `ops`), then drops (drop schema).
    assert_eq!(
        kinds,
        vec![
            StepKind::CreateSchema,
            StepKind::CreateTable,
            StepKind::DropSchema
        ]
    );
}

#[test]
fn emits_attach_partition_step() {
    use crate::diff::change::TableChange;
    use crate::ir::partition::PartitionBounds;

    let mut cs = ChangeSet::new();
    cs.push(
        Change::Table(TableChange::AttachPartition {
            parent: qn("app", "orders"),
            child: qn("app", "orders_2024"),
            bounds: PartitionBounds::Default,
        }),
        Destructiveness::Safe,
    );
    let steps = rewrite_changeset_only(cs);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::AttachPartition);
    assert!(
        steps[0].sql.contains("ATTACH PARTITION"),
        "expected ATTACH PARTITION in sql, got: {}",
        steps[0].sql
    );
    assert!(!steps[0].destructive);
    assert_eq!(steps[0].targets, vec![qn("app", "orders_2024")]);
    assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
}

#[test]
fn emits_detach_partition_step() {
    use crate::diff::change::TableChange;

    let mut cs = ChangeSet::new();
    cs.push(
        Change::Table(TableChange::DetachPartition {
            parent: qn("app", "orders"),
            child: qn("app", "orders_2024"),
        }),
        Destructiveness::Safe,
    );
    let steps = rewrite_changeset_only(cs);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::DetachPartition);
    assert!(
        steps[0].sql.contains("DETACH PARTITION"),
        "expected DETACH PARTITION in sql, got: {}",
        steps[0].sql
    );
    assert!(!steps[0].destructive);
    assert_eq!(steps[0].targets, vec![qn("app", "orders_2024")]);
    assert_eq!(steps[0].transactional, TransactionConstraint::InTransaction);
}

// ---- CREATE SUBSCRIPTION transaction-constraint (issue #11) ----

fn minimal_subscription(create_slot: Option<bool>) -> crate::ir::subscription::Subscription {
    use crate::ir::subscription::{Subscription, SubscriptionOptions};
    Subscription {
        name: id("mysub"),
        connection: "host=db.example.com dbname=app".to_string(),
        publications: vec![id("mypub")],
        options: SubscriptionOptions {
            create_slot,
            ..SubscriptionOptions::default()
        },
        owner: None,
        comment: None,
    }
}

fn create_subscription_changeset(sub: crate::ir::subscription::Subscription) -> Vec<RawStep> {
    let mut cs = crate::diff::changeset::ChangeSet::new();
    cs.push(
        Change::Subscription(crate::diff::change::SubscriptionChange::Create(sub)),
        crate::diff::destructiveness::Destructiveness::Safe,
    );
    rewrite_changeset_only(cs)
}

/// `create_slot = None` means PG default (true) — must run outside transaction.
#[test]
fn create_subscription_create_slot_none_is_outside_transaction() {
    let sub = minimal_subscription(None);
    let steps = create_subscription_changeset(sub);
    assert_eq!(steps[0].kind, StepKind::CreateSubscription);
    assert_eq!(
        steps[0].transactional,
        TransactionConstraint::OutsideTransaction,
        "create_slot=None means PG default true → must run outside transaction"
    );
}

/// `create_slot = Some(true)` — explicitly requesting slot creation — must run
/// outside transaction.
#[test]
fn create_subscription_create_slot_true_is_outside_transaction() {
    let sub = minimal_subscription(Some(true));
    let steps = create_subscription_changeset(sub);
    assert_eq!(steps[0].kind, StepKind::CreateSubscription);
    assert_eq!(
        steps[0].transactional,
        TransactionConstraint::OutsideTransaction,
        "create_slot=Some(true) → must run outside transaction"
    );
}

/// `create_slot = Some(false)` — no slot creation — may run inside a
/// transaction.
#[test]
fn create_subscription_create_slot_false_is_in_transaction() {
    let sub = minimal_subscription(Some(false));
    let steps = create_subscription_changeset(sub);
    assert_eq!(steps[0].kind, StepKind::CreateSubscription);
    assert_eq!(
        steps[0].transactional,
        TransactionConstraint::InTransaction,
        "create_slot=Some(false) → safe to run inside transaction"
    );
}

// ---- DROP SUBSCRIPTION transaction-constraint (issue #26) ----

/// `DROP SUBSCRIPTION` must always run outside a transaction block.
///
/// PG error 25001: when the subscription has an attached replication slot
/// the server rejects DROP SUBSCRIPTION inside a transaction. The IR cannot
/// determine at diff time whether the live subscription has a slot, so the
/// conservative path is always `OutsideTransaction`.
///
/// Companion of `4d8ec92` (#11 — CREATE SUBSCRIPTION).
#[test]
fn drop_subscription_is_always_outside_transaction() {
    let mut cs = crate::diff::changeset::ChangeSet::new();
    cs.push(
        Change::Subscription(crate::diff::change::SubscriptionChange::Drop { name: id("mysub") }),
        crate::diff::destructiveness::Destructiveness::RequiresApproval {
            reason: "drop subscription".into(),
        },
    );
    let steps = rewrite(
        OrderedChangeSet {
            drops: cs.entries,
            ..Default::default()
        },
        &Catalog::empty(),
        &PlannerPolicy::default(),
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, StepKind::DropSubscription);
    assert_eq!(
        steps[0].transactional,
        TransactionConstraint::OutsideTransaction,
        "DROP SUBSCRIPTION must always run outside transaction (PG error 25001 when slot is attached)"
    );
    assert_eq!(steps[0].sql, "DROP SUBSCRIPTION mysub;");
}
