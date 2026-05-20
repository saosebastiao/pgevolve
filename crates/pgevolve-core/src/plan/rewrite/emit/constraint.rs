//! Dispatcher for drift-recovery constraint changes.
//!
//! Today this module handles `Change::ValidateConstraint` only. Other
//! constraint operations are expressed as `TableOp`s and handled by
//! `emit/table.rs`.

use crate::identifier::{Identifier, QualifiedName};
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

pub fn validate(
    table: QualifiedName,
    constraint: Identifier,
    _destructive: bool,
    _destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::ValidateConstraint,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![table.clone()],
        sql: sql::alter_table_validate_constraint(&table, &constraint),
        transactional: TransactionConstraint::InTransaction,
    });
}
