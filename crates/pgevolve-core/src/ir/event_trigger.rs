//! `EVENT TRIGGER` IR. Database-global (bare name, no schema), independently
//! ownable; modeled like `Publication` (lenient owner, lenient drop in the diff).

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// A `CREATE EVENT TRIGGER` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventTrigger {
    /// Global object name — event triggers are not schema-qualified.
    pub name: Identifier,
    /// The DDL event the trigger fires on.
    pub event: EventTriggerEvent,
    /// `WHEN TAG IN (...)` command-tag filter; empty = no filter.
    /// Canon sorts + dedupes this list.
    pub tag_filter: Vec<String>,
    /// Schema-qualified name of the `EXECUTE FUNCTION` function.
    pub function: QualifiedName,
    /// Fire state (`pg_event_trigger.evtenabled`).
    pub enabled: EventTriggerEnabled,
    /// Lenient owner: `None` = unmanaged (matches `Publication`).
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

impl Equiv for EventTrigger {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a field is added
        // without being handled below. Bindings are unused (read via `self`).
        let Self {
            name: _,
            event: _,
            tag_filter: _,
            function: _,
            enabled: _,
            owner: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("name", &self.name, &other.name));
        out.extend(field_difference(
            "event",
            &format!("{:?}", self.event),
            &format!("{:?}", other.event),
        ));
        out.extend(field_difference(
            "tag_filter",
            &format!("{:?}", self.tag_filter),
            &format!("{:?}", other.tag_filter),
        ));
        out.extend(field_difference(
            "function",
            &self.function,
            &other.function,
        ));
        out.extend(field_difference(
            "enabled",
            &format!("{:?}", self.enabled),
            &format!("{:?}", other.enabled),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

/// The DDL event an event trigger fires on (`pg_event_trigger.evtevent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTriggerEvent {
    /// `ddl_command_start`
    DdlCommandStart,
    /// `ddl_command_end`
    DdlCommandEnd,
    /// `sql_drop`
    SqlDrop,
    /// `table_rewrite`
    TableRewrite,
}

impl EventTriggerEvent {
    /// The SQL keyword used in `ON <event>`.
    #[must_use]
    pub const fn sql(self) -> &'static str {
        match self {
            Self::DdlCommandStart => "ddl_command_start",
            Self::DdlCommandEnd => "ddl_command_end",
            Self::SqlDrop => "sql_drop",
            Self::TableRewrite => "table_rewrite",
        }
    }

    /// Parse from the SQL event name (lower-cased).
    #[must_use]
    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "ddl_command_start" => Some(Self::DdlCommandStart),
            "ddl_command_end" => Some(Self::DdlCommandEnd),
            "sql_drop" => Some(Self::SqlDrop),
            "table_rewrite" => Some(Self::TableRewrite),
            _ => None,
        }
    }
}

/// Fire state (`pg_event_trigger.evtenabled`: `O`/`D`/`R`/`A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTriggerEnabled {
    /// `O` — fires in origin/local sessions (the default).
    Enabled,
    /// `D` — never fires.
    Disabled,
    /// `R` — fires only when `session_replication_role = replica`.
    Replica,
    /// `A` — fires always (origin and replica).
    Always,
}

impl EventTriggerEnabled {
    /// Decode the single-char `pg_event_trigger.evtenabled` code.
    #[must_use]
    pub const fn from_pg_char(c: char) -> Option<Self> {
        match c {
            'O' => Some(Self::Enabled),
            'D' => Some(Self::Disabled),
            'R' => Some(Self::Replica),
            'A' => Some(Self::Always),
            _ => None,
        }
    }

    /// The `ALTER EVENT TRIGGER name <clause>` body for this state.
    #[must_use]
    pub const fn alter_clause(self) -> &'static str {
        match self {
            Self::Enabled => "ENABLE",
            Self::Disabled => "DISABLE",
            Self::Replica => "ENABLE REPLICA",
            Self::Always => "ENABLE ALWAYS",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_sql_roundtrips() {
        for e in [
            EventTriggerEvent::DdlCommandStart,
            EventTriggerEvent::DdlCommandEnd,
            EventTriggerEvent::SqlDrop,
            EventTriggerEvent::TableRewrite,
        ] {
            assert_eq!(EventTriggerEvent::from_sql(e.sql()), Some(e));
        }
        assert_eq!(EventTriggerEvent::from_sql("bogus"), None);
    }

    #[test]
    fn enabled_decodes_pg_chars() {
        assert_eq!(
            EventTriggerEnabled::from_pg_char('O'),
            Some(EventTriggerEnabled::Enabled)
        );
        assert_eq!(
            EventTriggerEnabled::from_pg_char('D'),
            Some(EventTriggerEnabled::Disabled)
        );
        assert_eq!(
            EventTriggerEnabled::from_pg_char('R'),
            Some(EventTriggerEnabled::Replica)
        );
        assert_eq!(
            EventTriggerEnabled::from_pg_char('A'),
            Some(EventTriggerEnabled::Always)
        );
        assert_eq!(EventTriggerEnabled::from_pg_char('x'), None);
    }

    #[test]
    fn alter_clauses() {
        assert_eq!(EventTriggerEnabled::Disabled.alter_clause(), "DISABLE");
        assert_eq!(
            EventTriggerEnabled::Replica.alter_clause(),
            "ENABLE REPLICA"
        );
        assert_eq!(EventTriggerEnabled::Always.alter_clause(), "ENABLE ALWAYS");
        assert_eq!(EventTriggerEnabled::Enabled.alter_clause(), "ENABLE");
    }
}
