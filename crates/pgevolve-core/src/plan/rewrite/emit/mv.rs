//! Dispatcher for `Change::Mv(MvChange)`.

use crate::diff::change::MvChange;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::views::{
    emit_comment_on_materialized_view, emit_comment_on_mv_column,
    emit_create_materialized_view, emit_drop_materialized_view, emit_refresh_mv,
};

pub fn emit(
    mc: MvChange,
    _destructive: bool,
    _destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    use crate::diff::change::MvChange as M;

    match mc {
        M::Create(mv) => {
            let qname = mv.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_materialized_view(&mv),
                transactional: TransactionConstraint::InTransaction,
            });
            // Always follow up with a REFRESH; concurrently=false here — T8 flips it.
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::RefreshMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_refresh_mv(&qname, false),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &mv.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnView,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_materialized_view(&qname, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
            for col in &mv.columns {
                if let Some(comment) = &col.comment {
                    out.push(RawStep {
                        step_no: 0,
                        kind: StepKind::CommentOnView,
                        destructive: false,
                        destructive_reason: None,
                        intent_id: None,
                        targets: vec![qname.clone()],
                        sql: emit_comment_on_mv_column(&qname, &col.name, Some(comment)),
                        transactional: TransactionConstraint::InTransaction,
                    });
                }
            }
        }
        M::Drop(qname) => {
            // MV drops are NOT destructive — materialized views are derived data.
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_materialized_view(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        M::ReplaceBody { source, catalog } => {
            let qname = source.qname.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_materialized_view(&catalog.qname),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_materialized_view(&source),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::RefreshMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_refresh_mv(&qname, false),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        M::SetComment { qname, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_comment_on_materialized_view(&qname, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        M::SetColumnComment {
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
                sql: emit_comment_on_mv_column(&qname, &column, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}
