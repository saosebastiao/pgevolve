//! Dispatcher for deferred FK additions emitted at the end of the plan.

use crate::plan::ordered::DeferredFkAdd;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(fk: &DeferredFkAdd, _ctx: &super::super::Ctx<'_>, out: &mut Vec<RawStep>) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::AddConstraint,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![fk.table.clone()],
        sql: super::super::sql::alter_table_add_constraint(&fk.table, &fk.constraint),
        transactional: TransactionConstraint::InTransaction,
    });
}
