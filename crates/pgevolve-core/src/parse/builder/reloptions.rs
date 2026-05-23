//! Decode `WITH (key = value, ...)` reloption clauses from `DefElem` nodes.
//!
//! Shared across `CREATE TABLE`, `CREATE INDEX`, `CREATE MATERIALIZED VIEW`,
//! and `ALTER TABLE/INDEX/MATERIALIZED VIEW ... SET (...)`.

use crate::ir::reloptions::{
    AutovacuumOptions, BufferingMode, IndexStorageOptions, NotNanF64, TableStorageOptions,
};
use crate::parse::error::{ParseError, SourceLocation};

/// Decode reloption clauses for a table or materialized view.
pub(crate) fn decode_table_options(
    options: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<TableStorageOptions, ParseError> {
    let mut out = TableStorageOptions::default();
    for opt_node in options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        if assign_autovacuum(&mut out.autovacuum, def, loc)? {
            continue;
        }
        let key = def.defname.as_str();
        let value = extract_value(def, loc)?;
        match key {
            "fillfactor" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 10..=100, "fillfactor (table)", loc)?;
                out.fillfactor = Some(n);
            }
            "parallel_workers" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 0..=1024, "parallel_workers", loc)?;
                out.parallel_workers = Some(n);
            }
            "toast_tuple_target" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 128..=8160, "toast_tuple_target", loc)?;
                out.toast_tuple_target = Some(n);
            }
            "user_catalog_table" => out.user_catalog_table = Some(parse_bool(&value, key, loc)?),
            "vacuum_truncate" => out.vacuum_truncate = Some(parse_bool(&value, key, loc)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

/// Decode reloption clauses for an index.
///
/// `access_method` is the `USING ...` clause from the surrounding
/// `CreateIndexStmt`; it is used to validate the fillfactor range, which
/// differs per access method.
pub(crate) fn decode_index_options(
    options: &[pg_query::protobuf::Node],
    access_method: &str,
    loc: &SourceLocation,
) -> Result<IndexStorageOptions, ParseError> {
    let mut out = IndexStorageOptions::default();
    for opt_node in options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        let key = def.defname.as_str();
        let value = extract_value(def, loc)?;
        match key {
            "fillfactor" => {
                let n = parse_u32(&value, key, loc)?;
                validate_index_fillfactor(n, access_method, loc)?;
                out.fillfactor = Some(n);
            }
            "fastupdate" => out.fastupdate = Some(parse_bool(&value, key, loc)?),
            "gin_pending_list_limit" => {
                out.gin_pending_list_limit = Some(parse_u64(&value, key, loc)?);
            }
            "buffering" => {
                out.buffering =
                    Some(
                        value
                            .parse::<BufferingMode>()
                            .map_err(|()| ParseError::Structural {
                                location: loc.clone(),
                                message: format!(
                                    "buffering value {value:?} invalid; expected on/off/auto"
                                ),
                            })?,
                    );
            }
            "deduplicate_items" => out.deduplicate_items = Some(parse_bool(&value, key, loc)?),
            "pages_per_range" => {
                let n = parse_u32(&value, key, loc)?;
                validate_range(n, 1..=131_072, "pages_per_range", loc)?;
                out.pages_per_range = Some(n);
            }
            "autosummarize" => out.autosummarize = Some(parse_bool(&value, key, loc)?),
            _ => {
                out.extra.insert(key.to_owned(), value);
            }
        }
    }
    Ok(out)
}

/// Validate fillfactor for a specific index access method.
///
/// PG enforces different lower bounds depending on the AM:
/// - B-tree: 50..=100
/// - `GiST` / Hash: 10..=100
/// - `SP-GiST`: 90..=100
/// - GIN / BRIN: fillfactor is not a supported option
fn validate_index_fillfactor(n: u32, method: &str, loc: &SourceLocation) -> Result<(), ParseError> {
    let valid_range = match method.to_ascii_lowercase().as_str() {
        "btree" | "" => 50..=100u32, // empty = pg default btree
        "spgist" => 90..=100,
        "brin" | "gin" => {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("fillfactor is not supported for {method} indexes"),
            });
        }
        _ => 10..=100, // gist, hash, and unknown AMs all accept 10..=100
    };
    validate_range(n, valid_range, &format!("fillfactor ({method} index)"), loc)
}

fn validate_range(
    n: u32,
    range: std::ops::RangeInclusive<u32>,
    label: &str,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    if !range.contains(&n) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "{label} = {n} out of range; valid: {}..={}",
                range.start(),
                range.end()
            ),
        });
    }
    Ok(())
}

fn assign_autovacuum(
    out: &mut AutovacuumOptions,
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<bool, ParseError> {
    let key = def.defname.as_str();
    let value = extract_value(def, loc)?;
    match key {
        "autovacuum_enabled" => out.enabled = Some(parse_bool(&value, key, loc)?),
        "autovacuum_vacuum_threshold" => {
            out.vacuum_threshold = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_vacuum_scale_factor" => {
            out.vacuum_scale_factor = Some(parse_notnan(&value, key, loc)?);
        }
        "autovacuum_vacuum_cost_delay" => {
            out.vacuum_cost_delay = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_vacuum_cost_limit" => {
            out.vacuum_cost_limit = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_analyze_threshold" => {
            out.analyze_threshold = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_analyze_scale_factor" => {
            out.analyze_scale_factor = Some(parse_notnan(&value, key, loc)?);
        }
        "autovacuum_freeze_max_age" => {
            out.freeze_max_age = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_freeze_min_age" => {
            out.freeze_min_age = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_freeze_table_age" => {
            out.freeze_table_age = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_multixact_freeze_max_age" => {
            out.multixact_freeze_max_age = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_multixact_freeze_min_age" => {
            out.multixact_freeze_min_age = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_multixact_freeze_table_age" => {
            out.multixact_freeze_table_age = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_vacuum_insert_threshold" => {
            out.vacuum_insert_threshold = Some(parse_u64(&value, key, loc)?);
        }
        "autovacuum_vacuum_insert_scale_factor" => {
            out.vacuum_insert_scale_factor = Some(parse_notnan(&value, key, loc)?);
        }
        "log_autovacuum_min_duration" => {
            out.log_min_duration = Some(parse_i64(&value, key, loc)?);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn extract_value(
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<String, ParseError> {
    let Some(arg) = def.arg.as_ref().and_then(|n| n.node.as_ref()) else {
        // Bare boolean reloptions (`WITH (autovacuum_enabled)`) mean true.
        return Ok("true".to_string());
    };
    match arg {
        // Raw scalar nodes (rare but possible for some callers)
        pg_query::NodeEnum::Integer(i) => Ok(i.ival.to_string()),
        pg_query::NodeEnum::Float(f) => Ok(f.fval.clone()),
        pg_query::NodeEnum::String(s) => Ok(s.sval.clone()),
        pg_query::NodeEnum::Boolean(b) => Ok(if b.boolval {
            "true".into()
        } else {
            "false".into()
        }),
        // pg_query 6.x wraps literal values in AConst; this is the common path
        // for reloption values written in source SQL.
        pg_query::NodeEnum::AConst(ac) => extract_aconst_value(ac, def, loc),
        // pg_query encodes some bare-keyword reloption values (e.g. `off`, `on`,
        // `auto`, `false`) as a TypeName node whose `names` list holds a single
        // String node. Treat the first name as the textual value.
        pg_query::NodeEnum::TypeName(tn) => {
            // Extract the last part of the names list (the unqualified name).
            let name_str = tn.names.iter().rev().find_map(|n| match n.node.as_ref() {
                Some(pg_query::NodeEnum::String(s)) if !s.sval.is_empty() => Some(s.sval.clone()),
                _ => None,
            });
            name_str.ok_or_else(|| ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "reloption {}: TypeName value had no extractable name",
                    def.defname
                ),
            })
        }
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "reloption {}: unexpected value node type {:?}",
                def.defname,
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Extract a string representation from an `A_Const` literal node.
fn extract_aconst_value(
    ac: &pg_query::protobuf::AConst,
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<String, ParseError> {
    use pg_query::protobuf::a_const::Val;
    match ac.val.as_ref() {
        Some(Val::Ival(i)) => Ok(i.ival.to_string()),
        Some(Val::Fval(f)) => Ok(f.fval.clone()),
        Some(Val::Sval(s)) => Ok(s.sval.clone()),
        Some(Val::Boolval(b)) => Ok(if b.boolval {
            "true".into()
        } else {
            "false".into()
        }),
        Some(Val::Bsval(bs)) => Ok(bs.bsval.clone()),
        None if ac.isnull => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("reloption {}: NULL value is not allowed", def.defname),
        }),
        None => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("reloption {}: empty AConst value node", def.defname),
        }),
    }
}

fn parse_u32(v: &str, key: &str, loc: &SourceLocation) -> Result<u32, ParseError> {
    v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_u64(v: &str, key: &str, loc: &SourceLocation) -> Result<u64, ParseError> {
    v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_i64(v: &str, key: &str, loc: &SourceLocation) -> Result<i64, ParseError> {
    v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })
}

fn parse_bool(v: &str, key: &str, loc: &SourceLocation) -> Result<bool, ParseError> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" => Ok(true),
        "false" | "off" | "0" => Ok(false),
        _ => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("reloption {key} = {v:?} not a recognized bool"),
        }),
    }
}

fn parse_notnan(v: &str, key: &str, loc: &SourceLocation) -> Result<NotNanF64, ParseError> {
    let f: f64 = v.parse().map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} = {v:?} parse error: {e}"),
    })?;
    NotNanF64::new(f).map_err(|_| ParseError::Structural {
        location: loc.clone(),
        message: format!("reloption {key} value is NaN"),
    })
}

/// Extract a `DefElem` list from an `AlterTableCmd.def` node.
///
/// For `AT_SetRelOptions` / `AT_ResetRelOptions`, `pg_query` stores the options
/// list as a `NodeEnum::List` inside `cmd.def`. This helper unpacks it.
pub(crate) fn extract_def_list(
    def: Option<&pg_query::protobuf::Node>,
    loc: &SourceLocation,
) -> Result<Vec<pg_query::protobuf::Node>, ParseError> {
    let node = def
        .and_then(|d| d.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: "ALTER ... SET (...) missing options list".into(),
        })?;
    match node {
        pg_query::NodeEnum::List(list) => Ok(list.items.clone()),
        _ => Err(ParseError::Structural {
            location: loc.clone(),
            message: "ALTER ... SET (...) options node was not a List".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn decode_table(sql: &str) -> TableStorageOptions {
        let parsed = pg_query::parse(sql).expect("parse");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let pg_query::NodeEnum::CreateStmt(create) = stmt else {
            panic!("expected CreateStmt")
        };
        decode_table_options(&create.options, &loc()).expect("decode")
    }

    fn try_decode_table(sql: &str) -> Result<TableStorageOptions, ParseError> {
        let parsed = pg_query::parse(sql).expect("parse");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let pg_query::NodeEnum::CreateStmt(create) = stmt else {
            panic!("expected CreateStmt")
        };
        decode_table_options(&create.options, &loc())
    }

    fn decode_index(sql: &str) -> IndexStorageOptions {
        let parsed = pg_query::parse(sql).expect("parse");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let pg_query::NodeEnum::IndexStmt(idx) = stmt else {
            panic!("expected IndexStmt")
        };
        decode_index_options(&idx.options, &idx.access_method, &loc()).expect("decode")
    }

    fn try_decode_index(sql: &str) -> Result<IndexStorageOptions, ParseError> {
        let parsed = pg_query::parse(sql).expect("parse");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let pg_query::NodeEnum::IndexStmt(idx) = stmt else {
            panic!("expected IndexStmt")
        };
        decode_index_options(&idx.options, &idx.access_method, &loc())
    }

    // ── Table reloptions ─────────────────────────────────────────────────────

    #[test]
    fn create_table_with_fillfactor() {
        let s = decode_table("CREATE TABLE app.t (id integer) WITH (fillfactor = 80);");
        assert_eq!(s.fillfactor, Some(80));
    }

    #[test]
    fn create_table_fillfactor_at_lower_bound() {
        let s = decode_table("CREATE TABLE app.t (id integer) WITH (fillfactor = 10);");
        assert_eq!(s.fillfactor, Some(10));
    }

    #[test]
    fn create_table_fillfactor_at_upper_bound() {
        let s = decode_table("CREATE TABLE app.t (id integer) WITH (fillfactor = 100);");
        assert_eq!(s.fillfactor, Some(100));
    }

    #[test]
    fn create_table_fillfactor_out_of_range_errors() {
        // 9 is below the 10..=100 table range.
        let err =
            try_decode_table("CREATE TABLE app.t (id integer) WITH (fillfactor = 9);").unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if message.contains("out of range")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn create_table_fillfactor_above_100_errors() {
        let err = try_decode_table("CREATE TABLE app.t (id integer) WITH (fillfactor = 101);")
            .unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn create_table_autovacuum_disabled() {
        let s = decode_table("CREATE TABLE app.t (id integer) WITH (autovacuum_enabled = false);");
        assert_eq!(s.autovacuum.enabled, Some(false));
    }

    #[test]
    fn create_table_autovacuum_scale_factor() {
        let s = decode_table(
            "CREATE TABLE app.t (id integer) WITH (autovacuum_vacuum_scale_factor = 0.05);",
        );
        let sf = s.autovacuum.vacuum_scale_factor.expect("scale factor");
        assert!((sf.get() - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn create_table_unknown_extra_key() {
        // Use a plain unknown key (without dot); dotted keys like pg_partman.something
        // are encoded by pg_query with a separate defnamespace field and are
        // accepted but rounded through the extra bag with a simple key form.
        let s = decode_table(
            "CREATE TABLE app.t (id integer) WITH (my_extension_option = 'somevalue');",
        );
        assert_eq!(
            s.extra.get("my_extension_option").map(String::as_str),
            Some("somevalue")
        );
    }

    #[test]
    fn create_table_bool_accepts_on_true() {
        let s = decode_table("CREATE TABLE app.t (id integer) WITH (autovacuum_enabled = on);");
        assert_eq!(s.autovacuum.enabled, Some(true));
    }

    #[test]
    fn create_table_malformed_bool_errors() {
        let err = try_decode_table(
            "CREATE TABLE app.t (id integer) WITH (autovacuum_enabled = 'maybe');",
        )
        .unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if message.contains("recognized bool")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn create_table_parallel_workers() {
        let s = decode_table("CREATE TABLE app.t (id integer) WITH (parallel_workers = 4);");
        assert_eq!(s.parallel_workers, Some(4));
    }

    #[test]
    fn create_table_vacuum_truncate() {
        let s = decode_table("CREATE TABLE app.t (id integer) WITH (vacuum_truncate = off);");
        assert_eq!(s.vacuum_truncate, Some(false));
    }

    // ── Index reloptions ─────────────────────────────────────────────────────

    #[test]
    fn create_index_btree_fillfactor_in_range() {
        let s = decode_index("CREATE INDEX i ON app.t USING btree (a) WITH (fillfactor = 75);");
        assert_eq!(s.fillfactor, Some(75));
    }

    #[test]
    fn create_index_btree_fillfactor_too_low_errors() {
        // B-tree requires 50..=100; 49 is below.
        let err =
            try_decode_index("CREATE INDEX i ON app.t USING btree (a) WITH (fillfactor = 49);")
                .unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if message.contains("out of range")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn create_index_default_method_btree_fillfactor_too_low_errors() {
        // No USING clause → treated as btree.
        let err =
            try_decode_index("CREATE INDEX i ON app.t (a) WITH (fillfactor = 40);").unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn create_index_gist_fillfactor_in_range() {
        // GiST allows 10..=100.
        let s = decode_index("CREATE INDEX i ON app.t USING gist (a) WITH (fillfactor = 15);");
        assert_eq!(s.fillfactor, Some(15));
    }

    #[test]
    fn create_index_brin_fillfactor_errors() {
        // BRIN does not support fillfactor.
        let err =
            try_decode_index("CREATE INDEX i ON app.t USING brin (a) WITH (fillfactor = 80);")
                .unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if message.contains("not supported")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn create_index_gin_fillfactor_errors() {
        // GIN does not support fillfactor.
        let err = try_decode_index("CREATE INDEX i ON app.t USING gin (a) WITH (fillfactor = 80);")
            .unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if message.contains("not supported")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn create_index_gin_fastupdate() {
        let s = decode_index("CREATE INDEX i ON app.t USING gin (a) WITH (fastupdate = off);");
        assert_eq!(s.fastupdate, Some(false));
    }

    #[test]
    fn create_index_buffering_auto() {
        let s = decode_index("CREATE INDEX i ON app.t USING gist (a) WITH (buffering = auto);");
        assert_eq!(s.buffering, Some(BufferingMode::Auto));
    }

    #[test]
    fn create_index_buffering_invalid_errors() {
        // pg_query may reject this at parse time; if not, our decoder must.
        let result =
            pg_query::parse("CREATE INDEX i ON app.t USING gist (a) WITH (buffering = 'bogus');");
        match result {
            Err(_) => {} // pg_query rejected it — fine
            Ok(parsed) => {
                let stmt = parsed
                    .protobuf
                    .stmts
                    .into_iter()
                    .next()
                    .and_then(|r| r.stmt)
                    .and_then(|n| n.node)
                    .unwrap();
                let pg_query::NodeEnum::IndexStmt(idx) = stmt else {
                    panic!("expected IndexStmt")
                };
                let err =
                    decode_index_options(&idx.options, &idx.access_method, &loc()).unwrap_err();
                assert!(matches!(err, ParseError::Structural { .. }));
            }
        }
    }

    #[test]
    fn create_index_spgist_fillfactor_low_end() {
        // SP-GiST: 90..=100; 90 is the lower bound.
        let s = decode_index("CREATE INDEX i ON app.t USING spgist (a) WITH (fillfactor = 90);");
        assert_eq!(s.fillfactor, Some(90));
    }

    #[test]
    fn create_index_spgist_fillfactor_too_low_errors() {
        let err =
            try_decode_index("CREATE INDEX i ON app.t USING spgist (a) WITH (fillfactor = 89);")
                .unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn create_index_pages_per_range() {
        let s =
            decode_index("CREATE INDEX i ON app.t USING brin (a) WITH (pages_per_range = 128);");
        assert_eq!(s.pages_per_range, Some(128));
    }

    #[test]
    fn nan_rejected_for_scale_factor() {
        // NotNanF64::new rejects NaN; pg_query will likely reject "NaN" as a
        // float literal; if not, our parse_notnan must.
        let result = pg_query::parse(
            "CREATE TABLE app.t (id integer) WITH (autovacuum_vacuum_scale_factor = 'NaN');",
        );
        match result {
            Err(_) => {} // pg_query rejected it
            Ok(parsed) => {
                let stmt = parsed
                    .protobuf
                    .stmts
                    .into_iter()
                    .next()
                    .and_then(|r| r.stmt)
                    .and_then(|n| n.node)
                    .unwrap();
                let pg_query::NodeEnum::CreateStmt(create) = stmt else {
                    panic!("expected CreateStmt")
                };
                // If pg_query returns the string, our decoder must reject it.
                if !create.options.is_empty() {
                    let res = decode_table_options(&create.options, &loc());
                    assert!(res.is_err(), "NaN must be rejected");
                }
            }
        }
    }
}
