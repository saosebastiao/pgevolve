//! SQL emission for trigger planner steps.
//!
//! Each helper produces a single canonical SQL statement string ending
//! with `;`, deterministic for byte-stable plan output.

#![allow(dead_code)]

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::trigger::{
    TransitionKind, Trigger, TriggerEvent, TriggerLevel, TriggerTiming,
};

/// `CREATE [CONSTRAINT] TRIGGER name { BEFORE | AFTER | INSTEAD OF }
///     event [ OR ... ] ON table
///     [ DEFERRABLE [ INITIALLY ... ] ]
///     [ REFERENCING ... ]
///     [ FOR EACH ROW | STATEMENT ]
///     [ WHEN ( ... ) ]
///     EXECUTE FUNCTION fn(args);`
pub(crate) fn create_trigger(t: &Trigger) -> String {
    let mut sql = String::new();
    sql.push_str("CREATE ");
    if t.is_constraint {
        sql.push_str("CONSTRAINT ");
    }
    sql.push_str("TRIGGER ");
    sql.push_str(t.qname.name.as_str());
    sql.push(' ');
    sql.push_str(render_timing(t.timing));
    sql.push(' ');
    sql.push_str(&render_events(&t.events));
    sql.push_str(" ON ");
    sql.push_str(&t.table.render_sql());

    if t.is_constraint {
        sql.push(' ');
        sql.push_str(render_deferrable(t.deferrable));
    }

    if !t.transition_tables.is_empty() {
        sql.push_str(" REFERENCING");
        for tr in &t.transition_tables {
            let kind = match tr.kind {
                TransitionKind::NewTable => "NEW TABLE AS",
                TransitionKind::OldTable => "OLD TABLE AS",
            };
            sql.push(' ');
            sql.push_str(kind);
            sql.push(' ');
            sql.push_str(tr.name.as_str());
        }
    }

    sql.push_str(" FOR EACH ");
    sql.push_str(match t.level {
        TriggerLevel::Row => "ROW",
        TriggerLevel::Statement => "STATEMENT",
    });

    if let Some(when) = &t.when_clause {
        sql.push_str(" WHEN (");
        sql.push_str(&when.canonical_text);
        sql.push(')');
    }

    sql.push_str(" EXECUTE FUNCTION ");
    sql.push_str(&t.function_qname.render_sql());
    sql.push('(');
    sql.push_str(&render_args(&t.function_args));
    sql.push_str(");");

    sql
}

/// `DROP TRIGGER name ON table;`
pub(crate) fn drop_trigger(qname: &QualifiedName, table: &QualifiedName) -> String {
    format!(
        "DROP TRIGGER {} ON {};",
        qname.name.as_str(),
        table.render_sql()
    )
}

/// `COMMENT ON TRIGGER name ON table IS '...';` / `IS NULL;`
pub(crate) fn comment_on_trigger(
    qname: &QualifiedName,
    table: &QualifiedName,
    comment: Option<&str>,
) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON TRIGGER {} ON {} IS '{}';",
            qname.name.as_str(),
            table.render_sql(),
            escape_sql_string(c),
        ),
        None => format!(
            "COMMENT ON TRIGGER {} ON {} IS NULL;",
            qname.name.as_str(),
            table.render_sql(),
        ),
    }
}

const fn render_timing(t: TriggerTiming) -> &'static str {
    match t {
        TriggerTiming::Before => "BEFORE",
        TriggerTiming::After => "AFTER",
        TriggerTiming::InsteadOf => "INSTEAD OF",
    }
}

fn render_events(events: &[TriggerEvent]) -> String {
    events
        .iter()
        .map(render_event)
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn render_event(e: &TriggerEvent) -> String {
    match e {
        TriggerEvent::Insert => "INSERT".to_string(),
        TriggerEvent::Update { columns } if columns.is_empty() => "UPDATE".to_string(),
        TriggerEvent::Update { columns } => format!(
            "UPDATE OF {}",
            columns
                .iter()
                .map(Identifier::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TriggerEvent::Delete => "DELETE".to_string(),
        TriggerEvent::Truncate => "TRUNCATE".to_string(),
    }
}

const fn render_deferrable(d: crate::ir::constraint::Deferrable) -> &'static str {
    use crate::ir::constraint::Deferrable as D;
    match d {
        D::NotDeferrable => "NOT DEFERRABLE",
        D::Deferrable {
            initially_deferred: true,
        } => "DEFERRABLE INITIALLY DEFERRED",
        D::Deferrable {
            initially_deferred: false,
        } => "DEFERRABLE INITIALLY IMMEDIATE",
    }
}

fn render_args(args: &[String]) -> String {
    args.iter()
        .map(|a| format!("'{}'", escape_sql_string(a)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::constraint::Deferrable;
    use crate::ir::default_expr::NormalizedExpr;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn trg() -> Trigger {
        Trigger {
            qname: qn("app", "t"),
            table: qn("app", "users"),
            timing: TriggerTiming::Before,
            events: vec![TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_clause: None,
            transition_tables: vec![],
            function_qname: qn("app", "f"),
            function_args: vec![],
            is_constraint: false,
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn simple_create() {
        let s = create_trigger(&trg());
        assert_eq!(
            s,
            "CREATE TRIGGER t BEFORE INSERT ON app.users FOR EACH ROW EXECUTE FUNCTION app.f();"
        );
    }

    #[test]
    fn drop_emits_table() {
        assert_eq!(
            drop_trigger(&qn("app", "t"), &qn("app", "users")),
            "DROP TRIGGER t ON app.users;"
        );
    }

    #[test]
    fn comment_set_and_clear() {
        assert_eq!(
            comment_on_trigger(&qn("app", "t"), &qn("app", "users"), Some("docs")),
            "COMMENT ON TRIGGER t ON app.users IS 'docs';"
        );
        assert_eq!(
            comment_on_trigger(&qn("app", "t"), &qn("app", "users"), None),
            "COMMENT ON TRIGGER t ON app.users IS NULL;"
        );
    }

    #[test]
    fn update_of_columns_renders() {
        let mut t = trg();
        t.events = vec![TriggerEvent::Update {
            columns: vec![id("a"), id("b")],
        }];
        let s = create_trigger(&t);
        assert!(s.contains("UPDATE OF a, b"), "got {s}");
    }

    #[test]
    fn constraint_trigger_includes_deferrable() {
        let mut t = trg();
        t.is_constraint = true;
        t.timing = TriggerTiming::After;
        t.deferrable = Deferrable::Deferrable {
            initially_deferred: true,
        };
        let s = create_trigger(&t);
        assert!(s.contains("CREATE CONSTRAINT TRIGGER"), "got {s}");
        assert!(s.contains("DEFERRABLE INITIALLY DEFERRED"), "got {s}");
    }

    #[test]
    fn when_clause_renders() {
        let mut t = trg();
        t.when_clause = Some(NormalizedExpr::from_text("new.id > 0"));
        let s = create_trigger(&t);
        assert!(s.contains("WHEN (new.id > 0)"), "got {s}");
    }
}
