//! Dispatcher for `Change::Procedure(ProcedureChange)`.

use crate::diff::change::ProcedureChange;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(
    pc: ProcedureChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    use crate::plan::rewrite::functions::{
        emit_comment_on_procedure, emit_create_or_replace_procedure, emit_drop_procedure,
    };

    match pc {
        ProcedureChange::Create(p) => {
            let qname = p.qname.clone();
            let comment = p.comment.clone();
            // Procedures with COMMIT/ROLLBACK in body must run outside a transaction.
            let transactional = if p.commits_in_body {
                TransactionConstraint::OutsideTransaction
            } else {
                TransactionConstraint::InTransaction
            };
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateOrReplaceProcedure,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_or_replace_procedure(&p),
                transactional,
            });
            if let Some(c) = &comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnProcedure,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_procedure(&qname, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        ProcedureChange::Drop(qname) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropProcedure,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_procedure(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        ProcedureChange::CreateOrReplace(p) => {
            let qname = p.qname.clone();
            let transactional = if p.commits_in_body {
                TransactionConstraint::OutsideTransaction
            } else {
                TransactionConstraint::InTransaction
            };
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateOrReplaceProcedure,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname],
                sql: emit_create_or_replace_procedure(&p),
                transactional,
            });
        }
        ProcedureChange::SetComment { qname, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnProcedure,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_procedure(&qname, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}
