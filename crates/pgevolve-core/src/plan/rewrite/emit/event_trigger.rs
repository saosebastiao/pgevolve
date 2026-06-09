//! Dispatcher for `Change::EventTrigger(EventTriggerChange)`.

use std::sync::LazyLock;

use crate::diff::change::EventTriggerChange;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled};
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

/// Synthetic schema component for event-trigger targets.
///
/// Event triggers are database-global (no schema). We use a synthetic
/// `pg_event_trigger.<name>` `QualifiedName` as the target — matching the
/// convention that `extension_target` uses for extensions.
static PG_EVENT_TRIGGER_SCHEMA: LazyLock<Identifier> = LazyLock::new(|| {
    Identifier::from_unquoted("pg_event_trigger")
        .expect("'pg_event_trigger' is a valid ASCII identifier — compile-time constant")
});

pub fn emit(
    etc: EventTriggerChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match etc {
        EventTriggerChange::Create(et) => {
            emit_create(&et, destructive, destructive_reason, out);
        }
        EventTriggerChange::Replace { from, to } => {
            // First: drop the old (destructive).
            let from_target = event_trigger_target(&from.name);
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropEventTrigger,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![from_target],
                sql: drop_sql(&from.name),
                transactional: TransactionConstraint::InTransaction,
            });
            // Then: create the new (safe) plus follow-ups.
            emit_create(&to, false, None, out);
        }
        EventTriggerChange::Drop { name } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropEventTrigger,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![event_trigger_target(&name)],
                sql: drop_sql(&name),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        EventTriggerChange::AlterEnable { name, enabled } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterEventTriggerEnable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![event_trigger_target(&name)],
                sql: alter_enable_sql(&name, enabled),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        EventTriggerChange::AlterOwner { name, owner } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterEventTriggerOwner,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![event_trigger_target(&name)],
                sql: alter_owner_sql(&name, &owner),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        EventTriggerChange::CommentOn { name, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnEventTrigger,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![event_trigger_target(&name)],
                sql: comment_sql(&name, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

/// Emit a CREATE step plus optional follow-up steps for enable/owner/comment.
///
/// A freshly created event trigger is always `ENABLED`, owned by the current
/// role, and has no comment. When the desired state differs, append the
/// necessary ALTER / COMMENT steps.
fn emit_create(
    et: &EventTrigger,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    let target = event_trigger_target(&et.name);
    out.push(RawStep {
        step_no: 0,
        kind: StepKind::CreateEventTrigger,
        destructive,
        destructive_reason,
        intent_id: None,
        targets: vec![target.clone()],
        sql: create_sql(et),
        transactional: TransactionConstraint::InTransaction,
    });
    if et.enabled != EventTriggerEnabled::Enabled {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterEventTriggerEnable,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![target.clone()],
            sql: alter_enable_sql(&et.name, et.enabled),
            transactional: TransactionConstraint::InTransaction,
        });
    }
    if let Some(owner) = &et.owner {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::AlterEventTriggerOwner,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![target.clone()],
            sql: alter_owner_sql(&et.name, owner),
            transactional: TransactionConstraint::InTransaction,
        });
    }
    if let Some(comment) = &et.comment {
        out.push(RawStep {
            step_no: 0,
            kind: StepKind::CommentOnEventTrigger,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![target],
            sql: comment_sql(&et.name, Some(comment.as_str())),
            transactional: TransactionConstraint::InTransaction,
        });
    }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

/// `CREATE EVENT TRIGGER name ON event [WHEN TAG IN ('a', 'b')] EXECUTE FUNCTION fn();`
fn create_sql(et: &EventTrigger) -> String {
    let mut sql = format!(
        "CREATE EVENT TRIGGER {} ON {}",
        et.name.render_sql(),
        et.event.sql(),
    );
    if !et.tag_filter.is_empty() {
        sql.push_str(" WHEN TAG IN (");
        let mut first = true;
        for tag in &et.tag_filter {
            if !first {
                sql.push_str(", ");
            }
            first = false;
            sql.push('\'');
            sql.push_str(&crate::plan::rewrite::sql::escape_sql_literal_body(tag));
            sql.push('\'');
        }
        sql.push(')');
    }
    sql.push_str(&format!(
        " EXECUTE FUNCTION {}();",
        et.function.render_sql()
    ));
    sql
}

/// `DROP EVENT TRIGGER name;`
fn drop_sql(name: &Identifier) -> String {
    format!("DROP EVENT TRIGGER {};", name.render_sql())
}

/// `ALTER EVENT TRIGGER name {ENABLE|DISABLE|ENABLE REPLICA|ENABLE ALWAYS};`
fn alter_enable_sql(name: &Identifier, enabled: EventTriggerEnabled) -> String {
    format!(
        "ALTER EVENT TRIGGER {} {};",
        name.render_sql(),
        enabled.alter_clause(),
    )
}

/// `ALTER EVENT TRIGGER name OWNER TO owner;`
fn alter_owner_sql(name: &Identifier, owner: &Identifier) -> String {
    format!(
        "ALTER EVENT TRIGGER {} OWNER TO {};",
        name.render_sql(),
        owner.render_sql(),
    )
}

/// `COMMENT ON EVENT TRIGGER name IS '...';` or `... IS NULL;`
fn comment_sql(name: &Identifier, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON EVENT TRIGGER {} IS '{}';",
            name.render_sql(),
            crate::plan::rewrite::sql::escape_sql_literal_body(c),
        ),
        None => format!("COMMENT ON EVENT TRIGGER {} IS NULL;", name.render_sql()),
    }
}

/// Synthetic `pg_event_trigger.<name>` target for the `targets` field.
fn event_trigger_target(name: &Identifier) -> QualifiedName {
    QualifiedName::new(PG_EVENT_TRIGGER_SCHEMA.clone(), name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn make_et() -> EventTrigger {
        EventTrigger {
            name: id("audit_ddl"),
            event: EventTriggerEvent::DdlCommandStart,
            tag_filter: vec![],
            function: {
                use crate::identifier::QualifiedName;
                QualifiedName::new(id("public"), id("audit_fn"))
            },
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        }
    }

    // --- create_sql ---

    #[test]
    fn create_sql_simple_no_tags() {
        let et = make_et();
        let sql = create_sql(&et);
        assert_eq!(
            sql,
            "CREATE EVENT TRIGGER audit_ddl ON ddl_command_start EXECUTE FUNCTION public.audit_fn();"
        );
    }

    #[test]
    fn create_sql_with_tag_filter() {
        let mut et = make_et();
        et.tag_filter = vec!["CREATE TABLE".to_string(), "ALTER TABLE".to_string()];
        let sql = create_sql(&et);
        assert_eq!(
            sql,
            "CREATE EVENT TRIGGER audit_ddl ON ddl_command_start WHEN TAG IN ('CREATE TABLE', 'ALTER TABLE') EXECUTE FUNCTION public.audit_fn();"
        );
    }

    #[test]
    fn create_sql_tag_with_embedded_single_quote() {
        let mut et = make_et();
        et.tag_filter = vec!["O'Brien".to_string()];
        let sql = create_sql(&et);
        assert!(sql.contains("'O''Brien'"), "got: {sql}");
    }

    // --- drop_sql ---

    #[test]
    fn drop_sql_renders_correctly() {
        let sql = drop_sql(&id("audit_ddl"));
        assert_eq!(sql, "DROP EVENT TRIGGER audit_ddl;");
    }

    // --- alter_enable_sql ---

    #[test]
    fn alter_enable_all_states() {
        for (state, expected) in [
            (
                EventTriggerEnabled::Enabled,
                "ALTER EVENT TRIGGER audit_ddl ENABLE;",
            ),
            (
                EventTriggerEnabled::Disabled,
                "ALTER EVENT TRIGGER audit_ddl DISABLE;",
            ),
            (
                EventTriggerEnabled::Replica,
                "ALTER EVENT TRIGGER audit_ddl ENABLE REPLICA;",
            ),
            (
                EventTriggerEnabled::Always,
                "ALTER EVENT TRIGGER audit_ddl ENABLE ALWAYS;",
            ),
        ] {
            let sql = alter_enable_sql(&id("audit_ddl"), state);
            assert_eq!(sql, expected, "state={state:?}");
        }
    }

    // --- alter_owner_sql ---

    #[test]
    fn alter_owner_renders_correctly() {
        let sql = alter_owner_sql(&id("audit_ddl"), &id("myuser"));
        assert_eq!(sql, "ALTER EVENT TRIGGER audit_ddl OWNER TO myuser;");
    }

    // --- comment_sql ---

    #[test]
    fn comment_sql_set() {
        let sql = comment_sql(&id("audit_ddl"), Some("tracks DDL changes"));
        assert_eq!(
            sql,
            "COMMENT ON EVENT TRIGGER audit_ddl IS 'tracks DDL changes';"
        );
    }

    #[test]
    fn comment_sql_clear_is_null() {
        let sql = comment_sql(&id("audit_ddl"), None);
        assert_eq!(sql, "COMMENT ON EVENT TRIGGER audit_ddl IS NULL;");
    }

    #[test]
    fn comment_sql_escapes_single_quotes() {
        let sql = comment_sql(&id("audit_ddl"), Some("O'Brien's trigger"));
        assert!(sql.contains("'O''Brien''s trigger'"), "got: {sql}");
    }

    // --- emit() integration ---

    #[test]
    fn emit_create_simple_produces_one_step() {
        let et = make_et();
        let mut out = Vec::new();
        emit(EventTriggerChange::Create(et), false, None, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CreateEventTrigger);
        assert!(out[0].sql.contains("CREATE EVENT TRIGGER"));
        assert!(!out[0].destructive);
    }

    #[test]
    fn emit_create_with_non_default_enabled_produces_two_steps() {
        let mut et = make_et();
        et.enabled = EventTriggerEnabled::Disabled;
        let mut out = Vec::new();
        emit(EventTriggerChange::Create(et), false, None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, StepKind::CreateEventTrigger);
        assert_eq!(out[1].kind, StepKind::AlterEventTriggerEnable);
        assert!(out[1].sql.contains("DISABLE"));
    }

    #[test]
    fn emit_create_with_owner_produces_two_steps() {
        let mut et = make_et();
        et.owner = Some(id("myuser"));
        let mut out = Vec::new();
        emit(EventTriggerChange::Create(et), false, None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, StepKind::CreateEventTrigger);
        assert_eq!(out[1].kind, StepKind::AlterEventTriggerOwner);
        assert!(out[1].sql.contains("OWNER TO"));
    }

    #[test]
    fn emit_create_with_comment_produces_two_steps() {
        let mut et = make_et();
        et.comment = Some("tracks DDL".to_string());
        let mut out = Vec::new();
        emit(EventTriggerChange::Create(et), false, None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, StepKind::CreateEventTrigger);
        assert_eq!(out[1].kind, StepKind::CommentOnEventTrigger);
        assert!(out[1].sql.contains("tracks DDL"));
    }

    #[test]
    fn emit_create_with_all_followups_produces_four_steps() {
        let mut et = make_et();
        et.enabled = EventTriggerEnabled::Replica;
        et.owner = Some(id("admin"));
        et.comment = Some("full test".to_string());
        let mut out = Vec::new();
        emit(EventTriggerChange::Create(et), false, None, &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].kind, StepKind::CreateEventTrigger);
        assert_eq!(out[1].kind, StepKind::AlterEventTriggerEnable);
        assert_eq!(out[2].kind, StepKind::AlterEventTriggerOwner);
        assert_eq!(out[3].kind, StepKind::CommentOnEventTrigger);
    }

    #[test]
    fn emit_drop_produces_one_destructive_step() {
        let mut out = Vec::new();
        emit(
            EventTriggerChange::Drop {
                name: id("audit_ddl"),
            },
            true,
            Some("removing event trigger".to_string()),
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::DropEventTrigger);
        assert!(out[0].destructive);
        assert!(out[0].sql.contains("DROP EVENT TRIGGER"));
    }

    #[test]
    fn emit_replace_first_step_is_drop() {
        let from = make_et();
        let mut to = make_et();
        to.event = EventTriggerEvent::DdlCommandEnd;
        let mut out = Vec::new();
        emit(
            EventTriggerChange::Replace { from, to },
            true,
            None,
            &mut out,
        );
        assert!(
            out.len() >= 2,
            "expected at least 2 steps, got {}",
            out.len()
        );
        assert_eq!(out[0].kind, StepKind::DropEventTrigger);
        assert!(out[0].destructive);
        assert_eq!(out[1].kind, StepKind::CreateEventTrigger);
        assert!(!out[1].destructive);
    }

    #[test]
    fn emit_alter_enable_produces_one_step() {
        let mut out = Vec::new();
        emit(
            EventTriggerChange::AlterEnable {
                name: id("audit_ddl"),
                enabled: EventTriggerEnabled::Always,
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AlterEventTriggerEnable);
        assert!(out[0].sql.contains("ENABLE ALWAYS"));
    }

    #[test]
    fn emit_alter_owner_produces_one_step() {
        let mut out = Vec::new();
        emit(
            EventTriggerChange::AlterOwner {
                name: id("audit_ddl"),
                owner: id("newrole"),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::AlterEventTriggerOwner);
        assert!(out[0].sql.contains("OWNER TO newrole"));
    }

    #[test]
    fn emit_comment_on_set_produces_one_step() {
        let mut out = Vec::new();
        emit(
            EventTriggerChange::CommentOn {
                name: id("audit_ddl"),
                comment: Some("my comment".to_string()),
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnEventTrigger);
        assert!(out[0].sql.contains("my comment"));
    }

    #[test]
    fn emit_comment_on_none_renders_is_null() {
        let mut out = Vec::new();
        emit(
            EventTriggerChange::CommentOn {
                name: id("audit_ddl"),
                comment: None,
            },
            false,
            None,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, StepKind::CommentOnEventTrigger);
        assert!(out[0].sql.contains("IS NULL"));
    }
}
