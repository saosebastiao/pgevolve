//! Dispatchers for `Change::CreateSchema`, `Change::DropSchema`,
//! `Change::AlterSchema`.

use crate::identifier::Identifier;
use crate::ir::schema::Schema;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::sql;

use super::super::schema_target;

pub fn create(
    s: Schema,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateSchema,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![schema_target(&s.name)],
        sql: sql::create_schema(&s),
        transactional: TransactionConstraint::InTransaction,
    });
    if let Some(c) = &s.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterSchemaComment,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![schema_target(&s.name)],
            sql: sql::comment_on_schema(&s.name, Some(c)),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

pub fn drop_(
    name: Identifier,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::DropSchema,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![schema_target(&name)],
        sql: sql::drop_schema(&name),
        transactional: TransactionConstraint::InTransaction,
    });
}

pub fn alter(name: Identifier, comment: Option<String>, out: &mut Vec<RawStep>) {
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::AlterSchemaComment,
        destructive: false,
        destructive_reason: None,
        intent_id: None,
        targets: vec![schema_target(&name)],
        sql: sql::comment_on_schema(&name, comment.as_deref()),
        transactional: TransactionConstraint::InTransaction,
    });
}
