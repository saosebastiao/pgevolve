//! `Trigger` — a Postgres trigger declared via `CREATE [CONSTRAINT] TRIGGER`.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::constraint::Deferrable;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::eq::DiffMacro;

/// A Postgres trigger.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Trigger {
    /// Schema-qualified trigger name. Schema mirrors the owning table's schema.
    pub qname: QualifiedName,
    /// Owning table (or view, for `INSTEAD OF` triggers).
    pub table: QualifiedName,
    /// When the trigger fires relative to the event.
    #[diff(via_debug)]
    pub timing: TriggerTiming,
    /// Events that fire this trigger.
    #[diff(via_debug)]
    pub events: Vec<TriggerEvent>,
    /// Row-level or statement-level.
    #[diff(via_debug)]
    pub level: TriggerLevel,
    /// Optional `WHEN (condition)` predicate.
    #[diff(via_debug)]
    pub when_clause: Option<NormalizedExpr>,
    /// Statement-level transition tables (`REFERENCING NEW TABLE AS n`).
    #[diff(via_debug)]
    pub transition_tables: Vec<TransitionTable>,
    /// Qualified name of the trigger function.
    pub function_qname: QualifiedName,
    /// Literal string arguments passed to the trigger function.
    #[diff(via_debug)]
    pub function_args: Vec<String>,
    /// `true` for `CREATE CONSTRAINT TRIGGER`.
    pub is_constraint: bool,
    /// Deferrability of constraint triggers.
    #[diff(via_debug)]
    pub deferrable: Deferrable,
    /// Optional `COMMENT ON TRIGGER` text.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

/// When a trigger fires relative to the triggering event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerTiming {
    /// Fires before the triggering event.
    Before,
    /// Fires after the triggering event.
    After,
    /// Replaces the triggering event (for views only).
    InsteadOf,
}

/// An event that can trigger a trigger.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerEvent {
    /// Trigger fires on `INSERT`.
    Insert,
    /// Trigger fires on `UPDATE` (optionally of specific columns).
    Update {
        /// Optional column list (`UPDATE OF col1, col2`). Empty = any column.
        columns: Vec<Identifier>,
    },
    /// Trigger fires on `DELETE`.
    Delete,
    /// Trigger fires on `TRUNCATE`.
    Truncate,
}

/// Whether a trigger fires for each row or each statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerLevel {
    /// Fires once per affected row.
    Row,
    /// Fires once per statement.
    Statement,
}

/// A statement-level transition table reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransitionTable {
    /// The name of the transition table.
    pub name: Identifier,
    /// Whether this is a NEW or OLD table.
    pub kind: TransitionKind,
}

/// Whether a transition table is for NEW or OLD rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// The `NEW` table containing inserted/updated rows.
    NewTable,
    /// The `OLD` table containing deleted/pre-updated rows.
    OldTable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn trg(name: &str) -> Trigger {
        Trigger {
            qname: qn("app", name),
            table: qn("app", "users"),
            timing: TriggerTiming::Before,
            events: vec![TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_clause: None,
            transition_tables: vec![],
            function_qname: qn("app", "fn"),
            function_args: vec![],
            is_constraint: false,
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn identical_triggers_diff_empty() {
        assert!(trg("t1").canonical_eq(&trg("t1")));
    }

    #[test]
    fn different_timing_diffs_reports_timing() {
        let a = trg("t1");
        let mut b = trg("t1");
        b.timing = TriggerTiming::After;
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "timing"));
    }

    #[test]
    fn different_events_diffs() {
        let a = trg("t1");
        let mut b = trg("t1");
        b.events = vec![TriggerEvent::Delete];
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "events"));
    }
}
