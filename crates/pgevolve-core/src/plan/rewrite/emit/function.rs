//! Dispatcher for `Change::Function(FunctionChange)`.

use crate::diff::change::FunctionChange;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(
    fc: FunctionChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    use crate::plan::rewrite::functions::{
        emit_comment_on_function, emit_create_or_replace_function, emit_drop_function,
    };

    match fc {
        FunctionChange::Create(f) => {
            let qname = f.qname.clone();
            let args = f.arg_types_normalized.clone();
            let comment = f.comment.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateOrReplaceFunction,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_or_replace_function(&f),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnFunction,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_function(&qname, &args, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        FunctionChange::Drop { qname, args } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropFunction,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_function(&qname, &args),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        FunctionChange::CreateOrReplace(f) => {
            let qname = f.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateOrReplaceFunction,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname],
                sql: emit_create_or_replace_function(&f),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        FunctionChange::ReplaceWithCascade { source, catalog } => {
            // DROP … CASCADE (destructive — requires approval).
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropFunction,
                destructive,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![catalog.qname.clone()],
                sql: emit_drop_function(&catalog.qname, &catalog.arg_types_normalized),
                transactional: TransactionConstraint::InTransaction,
            });
            // CREATE OR REPLACE for the source (also destructive — same gate).
            let qname = source.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateOrReplaceFunction,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname],
                sql: emit_create_or_replace_function(&source),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        FunctionChange::SetComment {
            qname,
            args,
            comment,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnFunction,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_function(&qname, &args, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}
