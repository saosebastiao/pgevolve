//! Assemble `pg_event_trigger` rows into `Vec<EventTrigger>`.
//!
//! Event triggers are database-global (not schema-scoped), independently
//! ownable, and modeled like `Publication`. The `EXECUTE FUNCTION` target is
//! resolved to a schema-qualified [`QualifiedName`] directly from
//! `pg_proc`/`pg_namespace` (mirroring how the trigger reader resolves its
//! function qname through the parser), so source-side and catalog-side
//! canonical forms compare equal.
//!
//! Extension-owned event triggers (`pg_depend.deptype = 'e'`) are excluded by
//! the SQL `WHERE NOT EXISTS (...)` clause, so they never reach this module.

// `CatalogError` embeds `IrError` and `ParseError`, both of which are large.
// Boxing them would add indirection noise without measurable benefit — errors
// here are cold-path (catalog reads, not hot loops).
#![allow(clippy::result_large_err)]

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};

const Q: CatalogQuery = CatalogQuery::EventTriggers;

/// Decode all `pg_event_trigger` rows into [`EventTrigger`] IR entries.
pub(super) fn assemble_event_triggers(rows: &[Row]) -> Result<Vec<EventTrigger>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(decode_event_trigger_row(row)?);
    }
    Ok(out)
}

fn decode_event_trigger_row(row: &Row) -> Result<EventTrigger, CatalogError> {
    let name_str = row.get_text(Q, "name")?;
    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: Q,
        column: "name".to_string(),
        message: format!("invalid event trigger name {name_str:?}: {e}"),
    })?;

    let event_str = row.get_text(Q, "event")?;
    let event =
        EventTriggerEvent::from_sql(&event_str).ok_or_else(|| CatalogError::BadColumnType {
            query: Q,
            column: "event".to_string(),
            message: format!("unknown event trigger event {event_str:?}"),
        })?;

    let enabled_str = row.get_text(Q, "enabled")?;
    let enabled_char = enabled_str
        .chars()
        .next()
        .ok_or_else(|| CatalogError::BadColumnType {
            query: Q,
            column: "enabled".to_string(),
            message: "empty evtenabled value".to_string(),
        })?;
    let enabled = EventTriggerEnabled::from_pg_char(enabled_char).ok_or_else(|| {
        CatalogError::BadColumnType {
            query: Q,
            column: "enabled".to_string(),
            message: format!("unknown evtenabled code {enabled_char:?}"),
        }
    })?;

    // `evttags` is `text[]`, NULL when there is no `WHEN TAG IN (...)` filter.
    let tag_filter = if row.is_null("tags") {
        Vec::new()
    } else {
        row.get_text_array(Q, "tags")?
    };

    let function_schema = row.get_text(Q, "function_schema")?;
    let function_name = row.get_text(Q, "function_name")?;
    let function_schema_ident =
        Identifier::from_unquoted(&function_schema).map_err(|e| CatalogError::BadColumnType {
            query: Q,
            column: "function_schema".to_string(),
            message: format!("invalid function schema {function_schema:?}: {e}"),
        })?;
    let function_name_ident =
        Identifier::from_unquoted(&function_name).map_err(|e| CatalogError::BadColumnType {
            query: Q,
            column: "function_name".to_string(),
            message: format!("invalid function name {function_name:?}: {e}"),
        })?;
    let function = QualifiedName::new(function_schema_ident, function_name_ident);

    let owner_str = row.get_text(Q, "owner")?;
    let owner = if owner_str.is_empty() {
        None
    } else {
        Some(
            Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::BadColumnType {
                query: Q,
                column: "owner".to_string(),
                message: format!("invalid owner name {owner_str:?}: {e}"),
            })?,
        )
    };

    let comment = match row.get_opt_text(Q, "comment")? {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    };

    Ok(EventTrigger {
        name,
        event,
        tag_filter,
        function,
        enabled,
        owner,
        comment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    /// Build a minimal valid event-trigger row.
    fn et_row(name: &str, event: &str, enabled: &str) -> Row {
        Row::new()
            .with("name", Value::Text(name.to_string()))
            .with("event", Value::Text(event.to_string()))
            .with("enabled", Value::Text(enabled.to_string()))
            .with("tags", Value::Null)
            .with("function_schema", Value::Text("app".to_string()))
            .with("function_name", Value::Text("audit_fn".to_string()))
            .with("owner", Value::Text("pg_user".to_string()))
            .with("comment", Value::Null)
    }

    #[test]
    fn simple_event_trigger_no_tags_enabled_origin() {
        let rows = vec![et_row("audit", "ddl_command_start", "O")];
        let ets = assemble_event_triggers(&rows).unwrap();
        assert_eq!(ets.len(), 1);
        let et = &ets[0];
        assert_eq!(et.name.as_str(), "audit");
        assert_eq!(et.event, EventTriggerEvent::DdlCommandStart);
        assert!(et.tag_filter.is_empty());
        // Function qname is schema-qualified from pg_proc/pg_namespace.
        assert_eq!(et.function.schema.as_str(), "app");
        assert_eq!(et.function.name.as_str(), "audit_fn");
        assert_eq!(et.enabled, EventTriggerEnabled::Enabled);
        assert_eq!(et.owner.as_ref().unwrap().as_str(), "pg_user");
        assert!(et.comment.is_none());
    }

    #[test]
    fn with_tag_filter() {
        let mut r = et_row("guard", "ddl_command_end", "O");
        r.insert(
            "tags",
            Value::TextArray(vec!["CREATE TABLE".to_string(), "ALTER TABLE".to_string()]),
        );
        let ets = assemble_event_triggers(&[r]).unwrap();
        assert_eq!(ets[0].event, EventTriggerEvent::DdlCommandEnd);
        assert_eq!(
            ets[0].tag_filter,
            vec!["CREATE TABLE".to_string(), "ALTER TABLE".to_string()]
        );
    }

    #[test]
    fn null_tags_decodes_to_empty_vec() {
        let rows = vec![et_row("t", "sql_drop", "O")];
        let ets = assemble_event_triggers(&rows).unwrap();
        assert_eq!(ets[0].event, EventTriggerEvent::SqlDrop);
        assert!(ets[0].tag_filter.is_empty());
    }

    #[test]
    fn each_enabled_code_decodes() {
        for (code, expected) in [
            ("O", EventTriggerEnabled::Enabled),
            ("D", EventTriggerEnabled::Disabled),
            ("R", EventTriggerEnabled::Replica),
            ("A", EventTriggerEnabled::Always),
        ] {
            let ets = assemble_event_triggers(&[et_row("e", "table_rewrite", code)]).unwrap();
            assert_eq!(ets[0].enabled, expected, "code {code}");
            assert_eq!(ets[0].event, EventTriggerEvent::TableRewrite);
        }
    }

    #[test]
    fn null_comment_is_none() {
        let ets = assemble_event_triggers(&[et_row("e", "ddl_command_start", "O")]).unwrap();
        assert!(ets[0].comment.is_none());
    }

    #[test]
    fn empty_comment_is_none() {
        let mut r = et_row("e", "ddl_command_start", "O");
        r.insert("comment", Value::Text(String::new()));
        let ets = assemble_event_triggers(&[r]).unwrap();
        assert!(ets[0].comment.is_none());
    }

    #[test]
    fn non_empty_comment_is_some() {
        let mut r = et_row("e", "ddl_command_start", "O");
        r.insert("comment", Value::Text("watches DDL".to_string()));
        let ets = assemble_event_triggers(&[r]).unwrap();
        assert_eq!(ets[0].comment.as_deref(), Some("watches DDL"));
    }

    #[test]
    fn empty_owner_is_none() {
        let mut r = et_row("e", "ddl_command_start", "O");
        r.insert("owner", Value::Text(String::new()));
        let ets = assemble_event_triggers(&[r]).unwrap();
        assert!(ets[0].owner.is_none());
    }

    #[test]
    fn unknown_event_errors() {
        let err = assemble_event_triggers(&[et_row("e", "bogus_event", "O")]).unwrap_err();
        assert!(matches!(err, CatalogError::BadColumnType { .. }));
    }

    #[test]
    fn unknown_enabled_code_errors() {
        let err = assemble_event_triggers(&[et_row("e", "ddl_command_start", "x")]).unwrap_err();
        assert!(matches!(err, CatalogError::BadColumnType { .. }));
    }

    #[test]
    fn empty_rows_returns_empty_vec() {
        assert!(assemble_event_triggers(&[]).unwrap().is_empty());
    }

    // Note: extension-owned event triggers (`pg_depend.deptype = 'e'`) are
    // excluded by the SQL WHERE clause in EVENT_TRIGGERS_QUERY, so that filter
    // is covered by the query, not unit-testable at the Row layer.
}
