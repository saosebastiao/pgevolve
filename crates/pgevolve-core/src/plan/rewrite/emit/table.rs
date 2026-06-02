//! Dispatchers for table changes: `CreateTable`, `DropTable`,
//! `AlterTable`, plus the per-`TableOp` emitter.

use crate::diff::table_op::{TableOp, TableOpEntry};
use crate::identifier::QualifiedName;
use crate::ir::column::Column;
use crate::ir::table::Table;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::{
    check_not_valid_validate, fk_not_valid_validate, set_not_null_check_pattern, sql,
};

use super::super::destructive_reason;

/// Emit a `SET STORAGE` step for a column that has an explicit storage
/// strategy.  `column_def` deliberately omits inline `STORAGE` because
/// that syntax is PG 16+ only; we always follow up with this separate
/// `ALTER TABLE … SET STORAGE` statement which is supported on all PG
/// versions we target (14–18).
fn push_set_storage_if_needed(qname: &QualifiedName, col: &Column, out: &mut Vec<RawStep>) {
    if let Some(storage) = col.storage {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnStorage,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_storage(qname, &col.name, storage),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

pub fn create(
    t: Table,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    let qname = t.qname.clone();
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateTable,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: sql::create_table(&t),
        transactional: TransactionConstraint::InTransaction,
    });
    // Inline STORAGE in CREATE TABLE is PG 16+ syntax; emit separate
    // ALTER TABLE … SET STORAGE steps (supported on all PG versions) for
    // every column that carries an explicit storage strategy.
    for col in &t.columns {
        push_set_storage_if_needed(&qname, col, out);
    }
    if let Some(c) = &t.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterTableSetComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::comment_on_table(&qname, Some(c)),
            transactional: TransactionConstraint::InTransaction,
        });
    }
    for col in &t.columns {
        if let Some(comment) = &col.comment {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::SetColumnComment,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::comment_on_column(&qname, &col.name, Some(comment)),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
    for c in &t.constraints {
        if let Some(cm) = &c.comment {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::SetConstraintComment,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::comment_on_constraint(&qname, &c.qname.name, Some(cm)),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

pub fn drop_(
    qname: QualifiedName,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::DropTable,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![qname.clone()],
        sql: sql::drop_table(&qname),
        transactional: TransactionConstraint::InTransaction,
    });
}

pub fn alter(
    qname: QualifiedName,
    ops: Vec<TableOpEntry>,
    ctx: &super::super::Ctx<'_>,
    out: &mut Vec<RawStep>,
) {
    for op_entry in ops {
        op(&qname, op_entry, ctx, out);
    }
}

#[allow(clippy::too_many_lines)] // one arm per `TableOpEntry` variant emitting its SQL; extraction would scatter the templates.
pub fn op(
    qname: &QualifiedName,
    entry: TableOpEntry,
    ctx: &super::super::Ctx<'_>,
    out: &mut Vec<RawStep>,
) {
    let destructive = entry.destructiveness.requires_approval();
    let destructive_reason = destructive_reason(&entry.destructiveness);
    match entry.op {
        TableOp::AddColumn(c) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AddColumn,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::alter_table_add_column(qname, &c),
                transactional: TransactionConstraint::InTransaction,
            });
            // Inline STORAGE in ALTER TABLE ADD COLUMN is PG 16+ syntax; emit
            // a separate SET STORAGE step for all supported PG versions (14+).
            push_set_storage_if_needed(qname, &c, out);
        }
        TableOp::DropColumn { name, .. } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::DropColumn,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_drop_column(qname, &name),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::AlterColumnType {
            name,
            from: _,
            to,
            using,
        } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterColumnType,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_type(qname, &name, &to, using.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnNullable { name, nullable } => {
            // Only `SET NOT NULL` is eligible for the CHECK pattern; flipping a
            // column back to nullable is always a single cheap step.
            if !nullable
                && set_not_null_check_pattern::should_rewrite(qname, &name, ctx.target, ctx.policy)
            {
                for step in set_not_null_check_pattern::rewrite_steps(
                    qname,
                    &name,
                    destructive,
                    destructive_reason,
                ) {
                    out.push(step);
                }
            } else {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::SetColumnNullable,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::alter_column_set_nullable(qname, &name, nullable),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        TableOp::SetColumnDefault { name, default } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnDefault,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_default(qname, &name, default.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnIdentity { name, identity } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnIdentity,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_identity(qname, &name, identity.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnGenerated { name, generated } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnGenerated,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_generated(qname, &name, generated.as_ref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnComment { name, comment } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::comment_on_column(qname, &name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnStorage { name, to, .. } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnStorage,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_storage(qname, &name, to),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetColumnCompression { name, compression } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnCompression,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_column_set_compression(qname, &name, compression),
            transactional: TransactionConstraint::InTransaction,
        }),

        TableOp::AddConstraint(c) => {
            if fk_not_valid_validate::should_rewrite(qname, &c, ctx.target, ctx.policy) {
                let [a, b] = fk_not_valid_validate::rewrite_steps(
                    qname,
                    &c,
                    destructive,
                    destructive_reason,
                );
                out.push(a);
                out.push(b);
            } else if check_not_valid_validate::should_rewrite(qname, &c, ctx.target, ctx.policy) {
                let [a, b] = check_not_valid_validate::rewrite_steps(
                    qname,
                    &c,
                    destructive,
                    destructive_reason,
                );
                out.push(a);
                out.push(b);
            } else {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::AddConstraint,
                    destructive,
                    destructive_reason,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::alter_table_add_constraint(qname, &c),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        TableOp::DropConstraint { name } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::DropConstraint,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::alter_table_drop_constraint(qname, &name),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetConstraintComment { name, comment } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetConstraintComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::comment_on_constraint(qname, &name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }),
        TableOp::SetTableComment { comment } => out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterTableSetComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::comment_on_table(qname, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }),
    }
}
