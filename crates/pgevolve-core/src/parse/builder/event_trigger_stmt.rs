//! Parser for `CREATE EVENT TRIGGER` and `ALTER EVENT TRIGGER` statements.
//!
//! `pg_query` emits `CreateEventTrigStmt` for CREATE and `AlterEventTrigStmt`
//! for the `ENABLE` / `DISABLE` / `ENABLE REPLICA` / `ENABLE ALWAYS` form. Both
//! fold into one [`EventTrigger`] per name — the same accumulator-keyed-by-name
//! pattern used for publications and subscriptions (event triggers are also
//! database-global objects with no schema qualifier).
//!
//! `COMMENT ON EVENT TRIGGER` and `ALTER EVENT TRIGGER … OWNER TO role` are
//! applied by name against the same accumulator from `parse/mod.rs`
//! (see `apply_event_trigger_comment` / `apply_event_trigger_owner`), mirroring
//! how `COMMENT ON STATISTICS` is folded inline against the statistics
//! accumulator. `RENAME` is rejected up front in `statement.rs`.

use std::collections::BTreeMap;

use pg_query::NodeEnum;
use pg_query::protobuf::{AlterEventTrigStmt, CreateEventTrigStmt};

use crate::identifier::Identifier;
use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply a `CREATE EVENT TRIGGER` statement to the accumulator map.
///
/// Builds the [`EventTrigger`] with `enabled = Enabled`, `owner = None`, and
/// `comment = None`. Rejects duplicate names and unknown events.
pub(crate) fn parse_create_event_trigger(
    stmt: &CreateEventTrigStmt,
    default_schema: Option<&Identifier>,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, EventTrigger>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.trigname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.trigname.clone(), e.to_string()))?;

    if existing.contains_key(&name) {
        return Err(ParseError::DuplicateEventTrigger(name, source_loc));
    }

    let event = EventTriggerEvent::from_sql(&stmt.eventname.to_lowercase()).ok_or_else(|| {
        ParseError::UnknownEventTriggerEvent(
            stmt.eventname.clone(),
            name.clone(),
            source_loc.clone(),
        )
    })?;

    let tag_filter = extract_tag_filter(&stmt.whenclause, &name, &source_loc)?;

    // `EXECUTE FUNCTION fn()` — resolve unqualified names against the file's
    // default schema (matching how other builders resolve bare function names).
    let function = shared::qname_from_string_list(&stmt.funcname, default_schema, &source_loc)?;

    existing.insert(
        name.clone(),
        EventTrigger {
            name,
            event,
            tag_filter,
            function,
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        },
    );
    Ok(())
}

/// Apply an `ALTER EVENT TRIGGER name {ENABLE|DISABLE|…}` statement.
///
/// Sets `enabled` from the `tgenabled` single-char code on the existing record.
/// Rejects ALTER-before-CREATE and unknown enable codes.
pub(crate) fn parse_alter_event_trigger(
    stmt: &AlterEventTrigStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, EventTrigger>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.trigname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.trigname.clone(), e.to_string()))?;

    let et = existing.get_mut(&name).ok_or_else(|| {
        ParseError::AlterEventTriggerBeforeCreate(name.clone(), source_loc.clone())
    })?;

    // `tgenabled` is a single character ("O" / "D" / "R" / "A").
    let code = stmt.tgenabled.chars().next().ok_or_else(|| {
        ParseError::UnknownEventTriggerEnabled(
            stmt.tgenabled.clone(),
            name.clone(),
            source_loc.clone(),
        )
    })?;
    let enabled = EventTriggerEnabled::from_pg_char(code).ok_or_else(|| {
        ParseError::UnknownEventTriggerEnabled(stmt.tgenabled.clone(), name.clone(), source_loc)
    })?;
    et.enabled = enabled;
    Ok(())
}

/// Apply a `COMMENT ON EVENT TRIGGER name IS '…'` against the accumulator.
///
/// Called inline from `parse/mod.rs` (not deferred) because event triggers live
/// in a `BTreeMap` accumulator that is flushed into the catalog after the
/// deferred-comment phase. The object reference is a bare `String` node.
pub(crate) fn apply_event_trigger_comment(
    stmt: &pg_query::protobuf::CommentStmt,
    location: &SourceLocation,
    existing: &mut BTreeMap<Identifier, EventTrigger>,
) -> Result<(), ParseError> {
    let name = string_object_name(stmt.object.as_deref(), location)?;
    let comment = if stmt.comment.is_empty() {
        None
    } else {
        Some(stmt.comment.clone())
    };
    let et = existing.get_mut(&name).ok_or_else(|| {
        ParseError::CommentOnEventTriggerBeforeCreate(name.clone(), location.clone())
    })?;
    et.comment = comment;
    Ok(())
}

/// Apply an `ALTER EVENT TRIGGER name OWNER TO role` against the accumulator.
///
/// Called inline from `parse/mod.rs`. The owner name is extracted from the
/// `newowner` `RoleSpec`; the target is a bare `String` node.
pub(crate) fn apply_event_trigger_owner(
    stmt: &pg_query::protobuf::AlterOwnerStmt,
    location: &SourceLocation,
    existing: &mut BTreeMap<Identifier, EventTrigger>,
) -> Result<(), ParseError> {
    let name = string_object_name(stmt.object.as_deref(), location)?;
    let new_owner = crate::parse::builder::owner_stmt::extract_new_owner(stmt, location)?;
    let et = existing
        .get_mut(&name)
        .ok_or_else(|| ParseError::EventTriggerOwnerBeforeCreate(name.clone(), location.clone()))?;
    et.owner = Some(new_owner);
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the `WHEN TAG IN ('…', …)` command-tag filter from a `whenclause`.
///
/// The clause is a list of `DefElem` nodes; the `tag` element's `arg` is a
/// `List` of `String` nodes. An empty `whenclause` (no `WHEN`) yields an empty
/// filter. Tag strings are returned as written; canon sorts and dedupes.
fn extract_tag_filter(
    whenclause: &[pg_query::protobuf::Node],
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<Vec<String>, ParseError> {
    let mut tags: Vec<String> = Vec::new();
    for node in whenclause {
        let Some(NodeEnum::DefElem(def)) = node.node.as_ref() else {
            return Err(ParseError::EventTriggerWhenClauseMalformed(
                name.clone(),
                loc.clone(),
            ));
        };
        if def.defname != "tag" {
            // The only WHEN variable PG supports for event triggers is `tag`.
            return Err(ParseError::EventTriggerWhenClauseMalformed(
                name.clone(),
                loc.clone(),
            ));
        }
        let Some(NodeEnum::List(list)) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
            return Err(ParseError::EventTriggerWhenClauseMalformed(
                name.clone(),
                loc.clone(),
            ));
        };
        for item in &list.items {
            match item.node.as_ref() {
                Some(NodeEnum::String(s)) => tags.push(s.sval.clone()),
                _ => {
                    return Err(ParseError::EventTriggerWhenClauseMalformed(
                        name.clone(),
                        loc.clone(),
                    ));
                }
            }
        }
    }
    Ok(tags)
}

/// Decode a bare `String`-node object reference (used by both COMMENT and
/// OWNER for event triggers) into an [`Identifier`].
fn string_object_name(
    object: Option<&pg_query::protobuf::Node>,
    loc: &SourceLocation,
) -> Result<Identifier, ParseError> {
    let node = object
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: "EVENT TRIGGER reference missing object name".into(),
        })?;
    match node {
        NodeEnum::String(s) => Identifier::from_unquoted(&s.sval)
            .map_err(|e| ParseError::InvalidIdentifier(s.sval.clone(), e.to_string())),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected String node for event-trigger name, got {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::parse::parse_directory;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    fn parse_source(sql: &str) -> Result<Catalog, ParseError> {
        let tmp = tempdir().expect("tempdir");
        write(tmp.path(), "schema.sql", sql);
        parse_directory(tmp.path(), &[])
    }

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_one_create(sql: &str) -> CreateEventTrigStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateEventTrigStmt(s) = node else {
            panic!("expected CreateEventTrigStmt");
        };
        s
    }

    fn parse_one_alter(sql: &str) -> AlterEventTrigStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterEventTrigStmt(s) = node else {
            panic!("expected AlterEventTrigStmt");
        };
        s
    }

    // ── CREATE ────────────────────────────────────────────────────────────────

    #[test]
    fn create_simple() {
        let stmt = parse_one_create(
            "CREATE EVENT TRIGGER e ON ddl_command_start EXECUTE FUNCTION app.f();",
        );
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        parse_create_event_trigger(&stmt, None, loc(), &mut acc).expect("ok");
        let et = acc.values().next().unwrap();
        assert_eq!(et.name.as_str(), "e");
        assert_eq!(et.event, EventTriggerEvent::DdlCommandStart);
        assert!(et.tag_filter.is_empty());
        assert_eq!(et.function.schema.as_str(), "app");
        assert_eq!(et.function.name.as_str(), "f");
        assert_eq!(et.enabled, EventTriggerEnabled::Enabled);
        assert!(et.owner.is_none());
        assert!(et.comment.is_none());
    }

    #[test]
    fn create_unqualified_function_uses_default_schema() {
        let stmt = parse_one_create("CREATE EVENT TRIGGER e ON sql_drop EXECUTE FUNCTION f();");
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        let app = Identifier::from_unquoted("app").unwrap();
        parse_create_event_trigger(&stmt, Some(&app), loc(), &mut acc).expect("ok");
        let et = acc.values().next().unwrap();
        assert_eq!(et.function.schema.as_str(), "app");
        assert_eq!(et.function.name.as_str(), "f");
        assert_eq!(et.event, EventTriggerEvent::SqlDrop);
    }

    #[test]
    fn create_with_tag_filter() {
        let stmt = parse_one_create(
            "CREATE EVENT TRIGGER e ON ddl_command_end \
             WHEN TAG IN ('CREATE TABLE', 'CREATE INDEX') EXECUTE FUNCTION public.f();",
        );
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        parse_create_event_trigger(&stmt, None, loc(), &mut acc).expect("ok");
        let et = acc.values().next().unwrap();
        assert_eq!(et.event, EventTriggerEvent::DdlCommandEnd);
        assert_eq!(
            et.tag_filter,
            vec!["CREATE TABLE".to_string(), "CREATE INDEX".to_string()]
        );
    }

    #[test]
    fn create_duplicate_name_errors() {
        let stmt =
            parse_one_create("CREATE EVENT TRIGGER e ON sql_drop EXECUTE FUNCTION public.f();");
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        parse_create_event_trigger(&stmt, None, loc(), &mut acc).expect("first ok");
        let err = parse_create_event_trigger(&stmt, None, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::DuplicateEventTrigger(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn create_unknown_event_errors() {
        // pg_query accepts an arbitrary event name; our enum rejects it.
        let stmt = parse_one_create("CREATE EVENT TRIGGER e ON login EXECUTE FUNCTION public.f();");
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        let err = parse_create_event_trigger(&stmt, None, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::UnknownEventTriggerEvent(ref s, _, _) if s == "login"),
            "got: {err:?}"
        );
    }

    // ── ALTER ─────────────────────────────────────────────────────────────────

    #[test]
    fn alter_disable() {
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        let create =
            parse_one_create("CREATE EVENT TRIGGER e ON sql_drop EXECUTE FUNCTION public.f();");
        parse_create_event_trigger(&create, None, loc(), &mut acc).expect("create ok");
        let alter = parse_one_alter("ALTER EVENT TRIGGER e DISABLE;");
        parse_alter_event_trigger(&alter, loc(), &mut acc).expect("alter ok");
        assert_eq!(
            acc.values().next().unwrap().enabled,
            EventTriggerEnabled::Disabled
        );
    }

    #[test]
    fn alter_enable_replica() {
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        let create =
            parse_one_create("CREATE EVENT TRIGGER e ON sql_drop EXECUTE FUNCTION public.f();");
        parse_create_event_trigger(&create, None, loc(), &mut acc).expect("create ok");
        let alter = parse_one_alter("ALTER EVENT TRIGGER e ENABLE REPLICA;");
        parse_alter_event_trigger(&alter, loc(), &mut acc).expect("alter ok");
        assert_eq!(
            acc.values().next().unwrap().enabled,
            EventTriggerEnabled::Replica
        );
    }

    #[test]
    fn alter_before_create_errors() {
        let alter = parse_one_alter("ALTER EVENT TRIGGER e DISABLE;");
        let mut acc: BTreeMap<Identifier, EventTrigger> = BTreeMap::new();
        let err = parse_alter_event_trigger(&alter, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::AlterEventTriggerBeforeCreate(_, _)),
            "got: {err:?}"
        );
    }

    // ── COMMENT / OWNER (via parse_directory) ─────────────────────────────────

    #[test]
    fn comment_applies() {
        let sql = "
            CREATE EVENT TRIGGER e ON sql_drop EXECUTE FUNCTION public.f();
            COMMENT ON EVENT TRIGGER e IS 'audit drops';
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.event_triggers.len(), 1);
        assert_eq!(
            cat.event_triggers[0].comment.as_deref(),
            Some("audit drops")
        );
    }

    #[test]
    fn owner_applies() {
        let sql = "
            CREATE EVENT TRIGGER e ON sql_drop EXECUTE FUNCTION public.f();
            ALTER EVENT TRIGGER e OWNER TO alice;
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.event_triggers.len(), 1);
        assert_eq!(
            cat.event_triggers[0].owner,
            Some(Identifier::from_unquoted("alice").unwrap())
        );
    }

    // ── RENAME rejection ──────────────────────────────────────────────────────

    #[test]
    fn rename_rejected() {
        let sql = "ALTER EVENT TRIGGER e RENAME TO f;";
        let err = parse_source(sql).expect_err("should fail");
        assert!(
            matches!(err, ParseError::EventTriggerRenameNotSupported(_, _)),
            "got: {err:?}"
        );
    }

    // ── Integration: fold CREATE + ALTER through parse_directory ──────────────

    #[test]
    fn parse_directory_folds_create_and_alter() {
        let sql = "
            CREATE EVENT TRIGGER guard ON ddl_command_start
                WHEN TAG IN ('DROP TABLE') EXECUTE FUNCTION public.block();
            ALTER EVENT TRIGGER guard ENABLE ALWAYS;
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.event_triggers.len(), 1);
        let et = &cat.event_triggers[0];
        assert_eq!(et.name.as_str(), "guard");
        assert_eq!(et.event, EventTriggerEvent::DdlCommandStart);
        assert_eq!(et.tag_filter, vec!["DROP TABLE".to_string()]);
        assert_eq!(et.function.schema.as_str(), "public");
        assert_eq!(et.function.name.as_str(), "block");
        assert_eq!(et.enabled, EventTriggerEnabled::Always);
    }
}
