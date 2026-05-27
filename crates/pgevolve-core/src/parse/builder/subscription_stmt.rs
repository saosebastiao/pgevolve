//! Parser for `CREATE SUBSCRIPTION` and `ALTER SUBSCRIPTION` statements.
//!
//! `pg_query` emits `CreateSubscriptionStmt` for CREATE and
//! `AlterSubscriptionStmt` for ALTER. Both are folded into one `Subscription`
//! per name — the same pattern as v0.3.4 PUBLICATION where `CREATE … WITH (…)`
//! and subsequent `ALTER … ADD/DROP/SET PUBLICATION / CONNECTION / SET (…)` all
//! unify into one IR record.
//!
//! Spec: `docs/superpowers/specs/2026-05-26-subscriptions-design.md`
//! Plan Stage 6: `docs/superpowers/plans/2026-05-26-subscriptions.md`

use std::collections::BTreeMap;

use pg_query::NodeEnum;
use pg_query::protobuf::{AlterSubscriptionStmt, AlterSubscriptionType, CreateSubscriptionStmt};

use crate::identifier::Identifier;
use crate::ir::subscription::{OriginMode, StreamingMode, Subscription, SubscriptionOptions};
use crate::parse::error::{ParseError, SourceLocation};

/// Apply a `CREATE SUBSCRIPTION` statement to the accumulator map.
///
/// Rejects duplicates. Parses `CONNECTION`, `PUBLICATION`, and `WITH (…)`.
/// The `CONNECTION` string is stored verbatim — no `${VAR}` resolution here.
pub(crate) fn parse_create_subscription(
    stmt: &CreateSubscriptionStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Subscription>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.subname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.subname.clone(), e.to_string()))?;

    if existing.contains_key(&name) {
        return Err(ParseError::DuplicateSubscription(name, source_loc));
    }

    if stmt.conninfo.is_empty() {
        return Err(ParseError::SubscriptionEmptyConnection(name, source_loc));
    }

    let publications = parse_publication_list(&stmt.publication, &name, &source_loc)?;
    if publications.is_empty() {
        return Err(ParseError::SubscriptionEmptyPublications(name, source_loc));
    }

    let options = parse_subscription_options(&stmt.options, &name, source_loc)?;

    existing.insert(
        name.clone(),
        Subscription {
            name,
            connection: stmt.conninfo.clone(),
            publications,
            options,
            owner: None,
            comment: None,
        },
    );
    Ok(())
}

/// Apply an `ALTER SUBSCRIPTION` statement to the accumulator map.
///
/// Folds ADD/DROP/SET PUBLICATION, CONNECTION, and SET (…) option changes
/// into the existing `Subscription` record. Rejects REFRESH, SKIP, and
/// standalone ENABLE/DISABLE. ALTER-before-CREATE returns an error.
pub(crate) fn parse_alter_subscription(
    stmt: &AlterSubscriptionStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Subscription>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.subname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.subname.clone(), e.to_string()))?;

    let kind =
        AlterSubscriptionType::try_from(stmt.kind).unwrap_or(AlterSubscriptionType::Undefined);

    // REFRESH and SKIP are rejected before the existence check so we can emit a
    // more specific error even without a prior CREATE.
    match kind {
        AlterSubscriptionType::AlterSubscriptionRefresh => {
            return Err(ParseError::SubscriptionRefreshNotSupported(
                name, source_loc,
            ));
        }
        AlterSubscriptionType::AlterSubscriptionSkip => {
            return Err(ParseError::SubscriptionSkipNotSupported(name, source_loc));
        }
        _ => {}
    }

    let sub = existing.get_mut(&name).ok_or_else(|| {
        ParseError::AlterSubscriptionBeforeCreate(name.clone(), source_loc.clone())
    })?;

    match kind {
        AlterSubscriptionType::AlterSubscriptionAddPublication => {
            let new_pubs = parse_publication_list(&stmt.publication, &name, &source_loc)?;
            for p in new_pubs {
                if !sub.publications.contains(&p) {
                    sub.publications.push(p);
                }
            }
        }
        AlterSubscriptionType::AlterSubscriptionDropPublication => {
            let drop_pubs = parse_publication_list(&stmt.publication, &name, &source_loc)?;
            sub.publications.retain(|p| !drop_pubs.contains(p));
        }
        AlterSubscriptionType::AlterSubscriptionSetPublication => {
            let new_pubs = parse_publication_list(&stmt.publication, &name, &source_loc)?;
            sub.publications = new_pubs;
        }
        AlterSubscriptionType::AlterSubscriptionConnection => {
            sub.connection.clone_from(&stmt.conninfo);
        }
        AlterSubscriptionType::AlterSubscriptionOptions => {
            let delta = parse_subscription_options(&stmt.options, &name, source_loc)?;
            merge_options(&mut sub.options, delta);
        }
        AlterSubscriptionType::AlterSubscriptionEnabled => {
            // Standalone ENABLE/DISABLE (`ALTER SUBSCRIPTION s ENABLE`) is not
            // supported. Users should declare `WITH (enabled = true/false)`.
            return Err(ParseError::SubscriptionStandaloneEnableDisableNotSupported(
                name, source_loc,
            ));
        }
        AlterSubscriptionType::AlterSubscriptionRefresh
        | AlterSubscriptionType::AlterSubscriptionSkip => {
            // Already handled above; unreachable here.
            unreachable!("refresh/skip handled before existence check");
        }
        AlterSubscriptionType::Undefined => {
            // Unrecognised kind — treat as a no-op; the source SQL might be
            // for a future PG version.
        }
    }

    Ok(())
}

// ── Publication list parsing ──────────────────────────────────────────────────

/// Parse a repeated `publication` field from CREATE/ALTER SUBSCRIPTION.
///
/// Each entry in the list is a `String` node whose `sval` is a publication name.
fn parse_publication_list(
    nodes: &[pg_query::protobuf::Node],
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<Vec<Identifier>, ParseError> {
    nodes
        .iter()
        .map(|n| {
            let s = extract_string_value(n).ok_or_else(|| {
                ParseError::SubscriptionOptionMalformed(name.clone(), loc.clone())
            })?;
            Identifier::from_unquoted(&s)
                .map_err(|e| ParseError::InvalidIdentifier(s, e.to_string()))
        })
        .collect()
}

// ── WITH (…) option parsing ───────────────────────────────────────────────────

/// Parse a subscription `WITH (…)` options list into a `SubscriptionOptions`.
///
/// Unknown option names → `UnknownSubscriptionOption`. PG-version-gated
/// validation (e.g. `streaming = parallel` requires PG 16) is deferred to the
/// Stage 9 lint rule — this function just stores what source declared.
fn parse_subscription_options(
    options: &[pg_query::protobuf::Node],
    name: &Identifier,
    loc: SourceLocation,
) -> Result<SubscriptionOptions, ParseError> {
    let mut opts = SubscriptionOptions::default();

    for opt in options {
        let Some(NodeEnum::DefElem(def)) = opt.node.as_ref() else {
            return Err(ParseError::SubscriptionOptionMalformed(name.clone(), loc));
        };
        match def.defname.as_str() {
            "enabled" => {
                opts.enabled = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "slot_name" => {
                let s = extract_def_elem_text(def, name, &loc)?;
                let id = Identifier::from_unquoted(&s)
                    .map_err(|e| ParseError::InvalidIdentifier(s, e.to_string()))?;
                opts.slot_name = Some(id);
            }
            "create_slot" => {
                opts.create_slot = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "copy_data" => {
                opts.copy_data = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "synchronous_commit" => {
                opts.synchronous_commit = Some(extract_def_elem_text(def, name, &loc)?);
            }
            "binary" => {
                opts.binary = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "streaming" => {
                let s = extract_def_elem_text(def, name, &loc)?;
                opts.streaming = Some(parse_streaming_mode(&s, name, &loc)?);
            }
            "two_phase" => {
                opts.two_phase = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "disable_on_error" => {
                opts.disable_on_error = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "password_required" => {
                opts.password_required = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "run_as_owner" => {
                opts.run_as_owner = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            "origin" => {
                let s = extract_def_elem_text(def, name, &loc)?;
                opts.origin = Some(parse_origin_mode(&s, name, &loc)?);
            }
            "failover" => {
                opts.failover = Some(extract_def_elem_bool(def, name, &loc)?);
            }
            other => {
                return Err(ParseError::UnknownSubscriptionOption(
                    other.to_string(),
                    name.clone(),
                    loc,
                ));
            }
        }
    }

    Ok(opts)
}

fn parse_streaming_mode(
    s: &str,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<StreamingMode, ParseError> {
    match s.to_ascii_lowercase().as_str() {
        "off" | "false" => Ok(StreamingMode::Off),
        "on" | "true" => Ok(StreamingMode::On),
        "parallel" => Ok(StreamingMode::Parallel),
        _ => Err(ParseError::UnknownStreamingMode(
            s.to_string(),
            name.clone(),
            loc.clone(),
        )),
    }
}

fn parse_origin_mode(
    s: &str,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<OriginMode, ParseError> {
    match s.to_ascii_lowercase().as_str() {
        "any" => Ok(OriginMode::Any),
        "none" => Ok(OriginMode::None),
        _ => Err(ParseError::UnknownOriginMode(
            s.to_string(),
            name.clone(),
            loc.clone(),
        )),
    }
}

/// Merge a delta `SubscriptionOptions` (from ALTER SET (…)) into `base`.
///
/// Only `Some` fields in `delta` overwrite the corresponding field in `base`.
fn merge_options(base: &mut SubscriptionOptions, delta: SubscriptionOptions) {
    if delta.enabled.is_some() {
        base.enabled = delta.enabled;
    }
    if delta.slot_name.is_some() {
        base.slot_name = delta.slot_name;
    }
    if delta.create_slot.is_some() {
        base.create_slot = delta.create_slot;
    }
    if delta.copy_data.is_some() {
        base.copy_data = delta.copy_data;
    }
    if delta.synchronous_commit.is_some() {
        base.synchronous_commit = delta.synchronous_commit;
    }
    if delta.binary.is_some() {
        base.binary = delta.binary;
    }
    if delta.streaming.is_some() {
        base.streaming = delta.streaming;
    }
    if delta.two_phase.is_some() {
        base.two_phase = delta.two_phase;
    }
    if delta.disable_on_error.is_some() {
        base.disable_on_error = delta.disable_on_error;
    }
    if delta.password_required.is_some() {
        base.password_required = delta.password_required;
    }
    if delta.run_as_owner.is_some() {
        base.run_as_owner = delta.run_as_owner;
    }
    if delta.origin.is_some() {
        base.origin = delta.origin;
    }
    if delta.failover.is_some() {
        base.failover = delta.failover;
    }
}

// ── Node extraction helpers ───────────────────────────────────────────────────

fn extract_string_value(node: &pg_query::protobuf::Node) -> Option<String> {
    match node.node.as_ref()? {
        NodeEnum::String(s) => Some(s.sval.clone()),
        _ => None,
    }
}

/// Extract the string value from a `DefElem.arg`.
///
/// Handles:
/// - `String(sval)` — bare identifier or quoted string
/// - `AConst { Sval }` — string constant
/// - `TypeName` — `pg_query` encodes bare keywords (e.g. `parallel`, `off`) as
///   `TypeName` nodes when they appear as option values without quoting
fn extract_def_elem_text(
    def: &pg_query::protobuf::DefElem,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<String, ParseError> {
    let arg = def
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::SubscriptionOptionMalformed(name.clone(), loc.clone()))?;

    match arg {
        NodeEnum::String(s) => Ok(s.sval.clone()),
        NodeEnum::AConst(ac) => {
            use pg_query::protobuf::a_const::Val;
            match ac.val.as_ref() {
                Some(Val::Sval(s)) => Ok(s.sval.clone()),
                _ => Err(ParseError::SubscriptionOptionMalformed(
                    name.clone(),
                    loc.clone(),
                )),
            }
        }
        NodeEnum::TypeName(tn) => {
            // pg_query encodes bare-keyword option values as TypeName nodes.
            // The last non-empty String within `names` is the actual keyword.
            tn.names
                .iter()
                .rev()
                .find_map(|n| match n.node.as_ref() {
                    Some(NodeEnum::String(s)) if !s.sval.is_empty() => Some(s.sval.clone()),
                    _ => None,
                })
                .ok_or_else(|| ParseError::SubscriptionOptionMalformed(name.clone(), loc.clone()))
        }
        _ => Err(ParseError::SubscriptionOptionMalformed(
            name.clone(),
            loc.clone(),
        )),
    }
}

/// Extract a boolean value from a `DefElem.arg`.
///
/// Handles all encoding forms `pg_query` may use:
/// - `Boolean { boolval: true/false }`
/// - `AConst` with `Boolval` or `Sval`
/// - `TypeName` (bare-keyword encoding for `true`/`false`/`on`/`off`)
/// - `String` node
/// - Missing `arg` (bare flag means true, e.g. `WITH (binary)`)
fn extract_def_elem_bool(
    def: &pg_query::protobuf::DefElem,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<bool, ParseError> {
    let Some(arg_node) = def.arg.as_ref().and_then(|n| n.node.as_ref()) else {
        // Bare flag (`WITH (binary)`) → true.
        return Ok(true);
    };

    match arg_node {
        NodeEnum::Boolean(b) => Ok(b.boolval),
        NodeEnum::AConst(ac) => {
            use pg_query::protobuf::a_const::Val;
            match ac.val.as_ref() {
                Some(Val::Boolval(b)) => Ok(b.boolval),
                Some(Val::Sval(s)) => parse_bool_str(&s.sval, &def.defname, name, loc),
                _ => Err(ParseError::SubscriptionOptionMalformed(
                    name.clone(),
                    loc.clone(),
                )),
            }
        }
        NodeEnum::TypeName(tn) => {
            // pg_query encodes bare-keyword booleans (true/false/on/off) as TypeName.
            let raw = tn
                .names
                .iter()
                .rev()
                .find_map(|n| match n.node.as_ref() {
                    Some(NodeEnum::String(s)) if !s.sval.is_empty() => Some(s.sval.clone()),
                    _ => None,
                })
                .ok_or_else(|| {
                    ParseError::SubscriptionOptionMalformed(name.clone(), loc.clone())
                })?;
            parse_bool_str(&raw, &def.defname, name, loc)
        }
        NodeEnum::String(s) => parse_bool_str(&s.sval, &def.defname, name, loc),
        _ => Err(ParseError::SubscriptionOptionMalformed(
            name.clone(),
            loc.clone(),
        )),
    }
}

fn parse_bool_str(
    v: &str,
    key: &str,
    name: &Identifier,
    loc: &SourceLocation,
) -> Result<bool, ParseError> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" | "yes" => Ok(true),
        "false" | "off" | "0" | "no" => Ok(false),
        _ => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "subscription {name:?}: option {key:?} = {v:?} is not a valid boolean"
            ),
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::parse::parse_directory;

    fn write(dir: &std::path::Path, rel: &str, contents: &str) {
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

    fn parse_one_create_stmt(sql: &str) -> pg_query::protobuf::CreateSubscriptionStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateSubscriptionStmt(s) = node else {
            panic!("expected CreateSubscriptionStmt, got something else");
        };
        s
    }

    fn parse_one_alter_stmt(sql: &str) -> pg_query::protobuf::AlterSubscriptionStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterSubscriptionStmt(s) = node else {
            panic!("expected AlterSubscriptionStmt, got something else");
        };
        s
    }

    // ── CREATE tests ──────────────────────────────────────────────────────────

    #[test]
    fn create_minimal() {
        let sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&stmt, loc(), &mut acc).expect("ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.name.as_str(), "s");
        assert_eq!(sub.connection, "host=x");
        assert_eq!(sub.publications.len(), 1);
        assert_eq!(sub.publications[0].as_str(), "p");
    }

    #[test]
    fn create_multi_publication() {
        let sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p, q;";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&stmt, loc(), &mut acc).expect("ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.publications.len(), 2);
        let names: Vec<_> = sub
            .publications
            .iter()
            .map(crate::identifier::Identifier::as_str)
            .collect();
        assert!(names.contains(&"p"));
        assert!(names.contains(&"q"));
    }

    #[test]
    fn create_with_options() {
        let sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p \
                   WITH (enabled = false, binary = true, streaming = parallel);";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&stmt, loc(), &mut acc).expect("ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.options.enabled, Some(false));
        assert_eq!(sub.options.binary, Some(true));
        assert_eq!(sub.options.streaming, Some(StreamingMode::Parallel));
    }

    #[test]
    fn create_connstr_stored_verbatim() {
        // Connection string with a ${VAR} placeholder must be stored as-is —
        // no expansion at parse time.
        let sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x password=${PWD}' PUBLICATION p;";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&stmt, loc(), &mut acc).expect("ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.connection, "host=x password=${PWD}");
    }

    // ── ALTER folding tests ───────────────────────────────────────────────────

    #[test]
    fn alter_add_publication() {
        let create_sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;";
        let alter_sql = "ALTER SUBSCRIPTION s ADD PUBLICATION q;";
        let create = parse_one_create_stmt(create_sql);
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&create, loc(), &mut acc).expect("create ok");
        parse_alter_subscription(&alter, loc(), &mut acc).expect("alter ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.publications.len(), 2);
        let names: Vec<_> = sub
            .publications
            .iter()
            .map(crate::identifier::Identifier::as_str)
            .collect();
        assert!(names.contains(&"p"));
        assert!(names.contains(&"q"));
    }

    #[test]
    fn alter_drop_publication() {
        let create_sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p, q;";
        let alter_sql = "ALTER SUBSCRIPTION s DROP PUBLICATION q;";
        let create = parse_one_create_stmt(create_sql);
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&create, loc(), &mut acc).expect("create ok");
        parse_alter_subscription(&alter, loc(), &mut acc).expect("alter ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.publications.len(), 1);
        assert_eq!(sub.publications[0].as_str(), "p");
    }

    #[test]
    fn alter_set_options() {
        let create_sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;";
        let alter_sql = "ALTER SUBSCRIPTION s SET (binary = true);";
        let create = parse_one_create_stmt(create_sql);
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&create, loc(), &mut acc).expect("create ok");
        parse_alter_subscription(&alter, loc(), &mut acc).expect("alter ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.options.binary, Some(true));
    }

    #[test]
    fn alter_connection() {
        let create_sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;";
        let alter_sql = "ALTER SUBSCRIPTION s CONNECTION 'host=y';";
        let create = parse_one_create_stmt(create_sql);
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&create, loc(), &mut acc).expect("create ok");
        parse_alter_subscription(&alter, loc(), &mut acc).expect("alter ok");
        let sub = acc.values().next().unwrap();
        assert_eq!(sub.connection, "host=y");
    }

    // ── Error cases ───────────────────────────────────────────────────────────

    #[test]
    fn alter_refresh_publication_errors() {
        let alter_sql = "ALTER SUBSCRIPTION s REFRESH PUBLICATION;";
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        let err = parse_alter_subscription(&alter, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::SubscriptionRefreshNotSupported(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn alter_skip_errors() {
        let alter_sql = "ALTER SUBSCRIPTION s SKIP (lsn = '0/0');";
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        let err = parse_alter_subscription(&alter, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::SubscriptionSkipNotSupported(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn alter_enable_errors() {
        let alter_sql = "ALTER SUBSCRIPTION s ENABLE;";
        let alter = parse_one_alter_stmt(alter_sql);
        // Need a prior CREATE for the existence check (ENABLE is checked after).
        let create_sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;";
        let create = parse_one_create_stmt(create_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&create, loc(), &mut acc).expect("create ok");
        let err = parse_alter_subscription(&alter, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(
                err,
                ParseError::SubscriptionStandaloneEnableDisableNotSupported(_, _)
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn rename_subscription_in_source_errors() {
        let sql = "ALTER SUBSCRIPTION s RENAME TO t;";
        let err = parse_source(sql).expect_err("should fail");
        assert!(
            matches!(
                err,
                ParseError::SubscriptionRenameNotSupported(_, _)
                    | ParseError::Structural { .. }
                    | ParseError::UnsupportedObjectKind { .. }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn create_empty_connection_errors() {
        // pg_query accepts an empty conninfo string at the AST level, so we
        // have to reject it ourselves.
        let mut stmt =
            parse_one_create_stmt("CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;");
        stmt.conninfo = String::new();
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        let err = parse_create_subscription(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::SubscriptionEmptyConnection(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn create_empty_publications_errors() {
        let mut stmt =
            parse_one_create_stmt("CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;");
        stmt.publication.clear();
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        let err = parse_create_subscription(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::SubscriptionEmptyPublications(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn create_unknown_streaming_mode_errors() {
        // Inject a bad streaming value via a crafted stmt.
        let mut stmt = parse_one_create_stmt(
            "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p WITH (streaming = on);",
        );
        // Patch the DefElem for streaming to have value "bogus".
        if let Some(node) = stmt.options.iter_mut().next()
            && let Some(NodeEnum::DefElem(ref mut def)) = node.node
            && def.defname == "streaming"
        {
            // Replace arg with a String node containing "bogus".
            def.arg = Some(Box::new(pg_query::protobuf::Node {
                node: Some(NodeEnum::String(pg_query::protobuf::String {
                    sval: "bogus".to_string(),
                })),
            }));
        }
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        let err = parse_create_subscription(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::UnknownStreamingMode(ref s, _, _) if s == "bogus"),
            "got: {err:?}"
        );
    }

    #[test]
    fn duplicate_subscription_errors() {
        let sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;";
        let stmt = parse_one_create_stmt(sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        parse_create_subscription(&stmt, loc(), &mut acc).expect("first ok");
        let err = parse_create_subscription(&stmt, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::DuplicateSubscription(_, _)),
            "got: {err:?}"
        );
    }

    #[test]
    fn alter_before_create_errors() {
        let alter_sql = "ALTER SUBSCRIPTION s ADD PUBLICATION q;";
        let alter = parse_one_alter_stmt(alter_sql);
        let mut acc: BTreeMap<Identifier, Subscription> = BTreeMap::new();
        let err = parse_alter_subscription(&alter, loc(), &mut acc).unwrap_err();
        assert!(
            matches!(err, ParseError::AlterSubscriptionBeforeCreate(_, _)),
            "got: {err:?}"
        );
    }

    // ── Integration tests via parse_directory ──────────────────────────────────

    #[test]
    fn parse_directory_create_subscription_minimal() {
        let sql = "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.subscriptions.len(), 1);
        assert_eq!(cat.subscriptions[0].name.as_str(), "s");
        assert_eq!(cat.subscriptions[0].connection, "host=x");
        assert_eq!(cat.subscriptions[0].publications.len(), 1);
    }

    #[test]
    fn parse_directory_folded_create_and_alter() {
        let sql = "
            CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p;
            ALTER SUBSCRIPTION s ADD PUBLICATION q;
        ";
        let cat = parse_source(sql).expect("parses");
        assert_eq!(cat.subscriptions.len(), 1);
        assert_eq!(cat.subscriptions[0].publications.len(), 2);
    }
}
