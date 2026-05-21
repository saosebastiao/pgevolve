//! Dispatcher for `Change::Trigger(TriggerChange)`.

use crate::diff::change::TriggerChange;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::triggers as sql;

pub fn emit(
    tc: TriggerChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match tc {
        TriggerChange::Create(t) => {
            let qname = t.qname.clone();
            let table = t.table.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateTrigger,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::create_trigger(&t),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(comment) = &t.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnTrigger,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::comment_on_trigger(&qname, &table, Some(comment)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        TriggerChange::Drop { qname, table } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropTrigger,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::drop_trigger(&qname, &table),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        TriggerChange::Replace(t) => {
            let qname = t.qname.clone();
            let table = t.table.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropTrigger,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::drop_trigger(&qname, &table),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateTrigger,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::create_trigger(&t),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(comment) = &t.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnTrigger,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: sql::comment_on_trigger(&qname, &table, Some(comment)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        TriggerChange::CommentOn {
            qname,
            table,
            comment,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnTrigger,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: sql::comment_on_trigger(&qname, &table, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::constraint::Deferrable;
    use crate::ir::trigger::{Trigger, TriggerEvent, TriggerLevel, TriggerTiming};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn make_trigger() -> Trigger {
        Trigger {
            qname: qn("app", "trg_audit"),
            table: qn("app", "users"),
            timing: TriggerTiming::Before,
            events: vec![TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_clause: None,
            transition_tables: vec![],
            function_qname: qn("app", "audit_fn"),
            function_args: vec![],
            is_constraint: false,
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn create_without_comment_produces_one_step() {
        let t = make_trigger();
        let mut out = Vec::new();
        emit(TriggerChange::Create(t), false, None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CreateTrigger);
        assert!(out[0].sql.contains("CREATE TRIGGER"));
    }

    #[test]
    fn create_with_comment_produces_two_steps() {
        let mut t = make_trigger();
        t.comment = Some("audit all inserts".to_string());
        let mut out = Vec::new();
        emit(TriggerChange::Create(t), false, None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, StepKind::CreateTrigger);
        assert_eq!(out[1].kind, StepKind::CommentOnTrigger);
        assert!(out[1].sql.contains("COMMENT ON TRIGGER"));
        assert!(out[1].sql.contains("audit all inserts"));
    }

    #[test]
    fn drop_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TriggerChange::Drop {
                qname: qn("app", "trg_audit"),
                table: qn("app", "users"),
            },
            true,
            Some("data loss".to_string()),
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::DropTrigger);
        assert!(out[0].sql.contains("DROP TRIGGER"));
        assert!(out[0].destructive);
    }

    #[test]
    fn replace_without_comment_produces_two_steps_drop_then_create() {
        let t = make_trigger();
        let mut out = Vec::new();
        emit(TriggerChange::Replace(t), true, None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, StepKind::DropTrigger);
        assert_eq!(out[1].kind, StepKind::CreateTrigger);
        assert!(out[0].destructive);
        assert!(!out[1].destructive);
    }

    #[test]
    fn replace_with_comment_produces_three_steps() {
        let mut t = make_trigger();
        t.comment = Some("replacement trigger".to_string());
        let mut out = Vec::new();
        emit(TriggerChange::Replace(t), true, None, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, StepKind::DropTrigger);
        assert_eq!(out[1].kind, StepKind::CreateTrigger);
        assert_eq!(out[2].kind, StepKind::CommentOnTrigger);
    }

    #[test]
    fn comment_on_produces_one_step() {
        let mut out = Vec::new();
        emit(
            TriggerChange::CommentOn {
                qname: qn("app", "trg_audit"),
                table: qn("app", "users"),
                comment: Some("updated comment".to_string()),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnTrigger);
        assert!(out[0].sql.contains("updated comment"));
    }

    #[test]
    fn comment_on_none_clears_comment() {
        let mut out = Vec::new();
        emit(
            TriggerChange::CommentOn {
                qname: qn("app", "trg_audit"),
                table: qn("app", "users"),
                comment: None,
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnTrigger);
        assert!(out[0].sql.contains("IS NULL"));
    }
}
