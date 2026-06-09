//! `Trigger` — a Postgres trigger declared via `CREATE [CONSTRAINT] TRIGGER`.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::constraint::Deferrable;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};

/// A Postgres trigger.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Trigger {
    /// Schema-qualified trigger name. Schema mirrors the owning table's schema.
    pub qname: QualifiedName,
    /// Owning table (or view, for `INSTEAD OF` triggers).
    pub table: QualifiedName,
    /// When the trigger fires relative to the event.
    pub timing: TriggerTiming,
    /// Events that fire this trigger.
    pub events: Vec<TriggerEvent>,
    /// Row-level or statement-level.
    pub level: TriggerLevel,
    /// Optional `WHEN (condition)` predicate.
    pub when_clause: Option<NormalizedExpr>,
    /// Statement-level transition tables (`REFERENCING NEW TABLE AS n`).
    pub transition_tables: Vec<TransitionTable>,
    /// Qualified name of the trigger function.
    pub function_qname: QualifiedName,
    /// Literal string arguments passed to the trigger function.
    pub function_args: Vec<String>,
    /// `true` for `CREATE CONSTRAINT TRIGGER`.
    pub is_constraint: bool,
    /// Deferrability of constraint triggers.
    pub deferrable: Deferrable,
    /// Optional `COMMENT ON TRIGGER` text.
    pub comment: Option<String>,
}

impl Diff for Trigger {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let Self {
            qname: _,
            table: _,
            timing: _,
            events: _,
            level: _,
            when_clause: _,
            transition_tables: _,
            function_qname: _,
            function_args: _,
            is_constraint: _,
            deferrable: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field("table", &self.table, &other.table));
        out.extend(diff_field(
            "timing",
            &format!("{:?}", self.timing),
            &format!("{:?}", other.timing),
        ));
        out.extend(diff_field(
            "events",
            &format!("{:?}", self.events),
            &format!("{:?}", other.events),
        ));
        out.extend(diff_field(
            "level",
            &format!("{:?}", self.level),
            &format!("{:?}", other.level),
        ));
        out.extend(diff_field(
            "when_clause",
            &format!("{:?}", self.when_clause),
            &format!("{:?}", other.when_clause),
        ));
        out.extend(diff_field(
            "transition_tables",
            &format!("{:?}", self.transition_tables),
            &format!("{:?}", other.transition_tables),
        ));
        out.extend(diff_field(
            "function_qname",
            &self.function_qname,
            &other.function_qname,
        ));
        out.extend(diff_field(
            "function_args",
            &format!("{:?}", self.function_args),
            &format!("{:?}", other.function_args),
        ));
        out.extend(diff_field(
            "is_constraint",
            &self.is_constraint,
            &other.is_constraint,
        ));
        out.extend(diff_field(
            "deferrable",
            &format!("{:?}", self.deferrable),
            &format!("{:?}", other.deferrable),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
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
