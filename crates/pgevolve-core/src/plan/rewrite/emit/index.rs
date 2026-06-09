//! Dispatchers for index changes: Create, Drop, Replace, Recreate.
//!
//! `Replace` and `Recreate` cover online-rewrite paths (gated on
//! `PlannerPolicy::create_index_concurrent`) and drift recovery for
//! INVALID indexes, respectively.

use crate::identifier::QualifiedName;
use crate::ir::index::Index;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::{concurrent_index, sql};

pub fn create(
    idx: Index,
    ctx: &super::super::Ctx<'_>,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    let qname = idx.qname.clone();
    let step = if concurrent_index::should_rewrite_create(&idx, ctx.target, ctx.policy) {
        concurrent_index::create_step(&idx, destructive, destructive_reason)
    } else {
        RawStep {
            step_no: 0,
            kind: StepKind::CreateIndex,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone(), idx.on.qname().clone()],
            sql: sql::create_index(&idx, false),
            transactional: TransactionConstraint::InTransaction,
        }
    };
    out.push(step);
    if let Some(c) = &idx.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::SetColumnComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: format!(
                "COMMENT ON INDEX {} IS '{}';",
                qname.render_sql(),
                sql::escape_sql_literal_body(c),
            ),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

pub fn drop_(
    qname: QualifiedName,
    ctx: &super::super::Ctx<'_>,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    let step = if concurrent_index::should_rewrite_drop(&qname, ctx.target, ctx.policy) {
        concurrent_index::drop_step(&qname, destructive, destructive_reason)
    } else {
        RawStep {
            step_no: 0,
            kind: StepKind::DropIndex,
            destructive,
            destructive_reason,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::drop_index(&qname, false),
            transactional: TransactionConstraint::InTransaction,
        }
    };
    out.push(step);
}

pub fn replace(
    from: Index,
    to: Index,
    ctx: &super::super::Ctx<'_>,
    _destructive: bool,
    _destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Drop the old index, then create the new one. Each side runs
    // through the same concurrent-rewrite check as a top-level
    // Create/Drop so policy applies uniformly.
    let drop_step = if concurrent_index::should_rewrite_drop(&from.qname, ctx.target, ctx.policy) {
        concurrent_index::drop_step(&from.qname, false, None)
    } else {
        RawStep {
            step_no: 0,
            kind: StepKind::DropIndex,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![from.qname.clone()],
            sql: sql::drop_index(&from.qname, false),
            transactional: TransactionConstraint::InTransaction,
        }
    };
    let create_step = if concurrent_index::should_rewrite_create(&to, ctx.target, ctx.policy) {
        concurrent_index::create_step(&to, false, None)
    } else {
        RawStep {
            step_no: 0,
            kind: StepKind::CreateIndex,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![to.qname.clone(), to.on.qname().clone()],
            sql: sql::create_index(&to, false),
            transactional: TransactionConstraint::InTransaction,
        }
    };
    out.push(drop_step);
    out.push(create_step);
}

pub fn recreate(
    qname: QualifiedName,
    ctx: &super::super::Ctx<'_>,
    _destructive: bool,
    _destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    // Drop the invalid index then re-create it from the source IR.
    // If the index is unknown in the source (e.g., it was dropped in the
    // same migration), emit only the drop so we don't fail the plan.
    let source_idx = ctx.source.indexes.iter().find(|i| i.qname == qname);
    // Drop step — use concurrent rewrite if policy allows and target
    // index exists (we know it does because the drift report saw it).
    let drop_step = if concurrent_index::should_rewrite_drop(&qname, ctx.target, ctx.policy) {
        concurrent_index::drop_step(&qname, false, None)
    } else {
        RawStep {
            step_no: 0,
            kind: StepKind::DropIndex,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qname.clone()],
            sql: sql::drop_index(&qname, false),
            transactional: TransactionConstraint::InTransaction,
        }
    };
    out.push(drop_step);
    if let Some(idx) = source_idx {
        let create_step = if concurrent_index::should_rewrite_create(idx, ctx.target, ctx.policy) {
            concurrent_index::create_step(idx, false, None)
        } else {
            RawStep {
                step_no: 0,
                kind: StepKind::CreateIndex,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![idx.qname.clone(), idx.on.qname().clone()],
                sql: sql::create_index(idx, false),
                transactional: TransactionConstraint::InTransaction,
            }
        };
        out.push(create_step);
    }
}
