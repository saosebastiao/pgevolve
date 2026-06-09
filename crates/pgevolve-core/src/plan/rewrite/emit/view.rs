//! Dispatcher for `Change::View(ViewChange)`.

use crate::diff::change::{BodyReplaceStrategy, ViewChange};
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

pub fn emit(
    vc: ViewChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    use crate::diff::change::ViewChange as V;
    use crate::plan::rewrite::views::{
        emit_alter_view_set_reloption, emit_comment_on_view, emit_comment_on_view_column,
        emit_create_view, emit_drop_view,
    };

    match vc {
        V::Create(v) => {
            let qname = v.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_view(&v, false),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &v.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnView,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_view(&qname, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
            for col in &v.columns {
                if let Some(comment) = &col.comment {
                    out.push(RawStep {
                        step_no: 0,
                        kind: StepKind::CommentOnView,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![qname.clone()],
                        sql: emit_comment_on_view_column(&qname, &col.name, Some(comment)),
                        transactional: TransactionConstraint::InTransaction,
                    });
                }
            }
        }
        V::Drop(qname) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropView,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_view(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::ReplaceBody {
            source,
            catalog: _,
            strategy: BodyReplaceStrategy::InPlace,
        } => {
            let qname = source.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname],
                sql: emit_create_view(&source, true),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::ReplaceBody {
            source,
            catalog,
            strategy: BodyReplaceStrategy::Recreate,
        } => {
            let qname = source.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropView,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_view(&catalog.qname),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname],
                sql: emit_create_view(&source, false),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::SetReloption {
            qname,
            security_barrier,
            security_invoker,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterViewSetReloption,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_view_set_reloption(&qname, security_barrier, security_invoker),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::SetComment { qname, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_view(&qname, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        V::SetColumnComment {
            qname,
            column,
            comment,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_view_column(&qname, &column, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}
