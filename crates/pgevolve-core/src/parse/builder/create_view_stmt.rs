//! `CREATE VIEW` → [`crate::ir::view::View`].
//!
//! Produces a *provisional* IR record: `body_canonical` is set to the empty
//! sentinel and `body_dependencies` is empty. T4's AST canonicalization pass
//! fills in both fields immediately after source IR is assembled.

use pg_query::NodeEnum;
use pg_query::protobuf::{AConst, ViewStmt, a_const};

use crate::identifier::Identifier;
use crate::ir::column_type::ColumnType;
use crate::ir::view::{CheckOption, View, ViewColumn};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_body::NormalizedBody;

/// Build a provisional [`View`] from a `CREATE [OR REPLACE] VIEW` AST node.
///
/// Column names are taken from the explicit alias list when present; otherwise
/// the `body_canonical` column list is left empty until T4 fills it in.
pub(crate) fn build_view(
    stmt: &ViewStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<View, ParseError> {
    let range_var = stmt.view.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "CREATE VIEW missing view name".into(),
    })?;
    let qname = shared::resolve_qname(range_var, default_schema, location)?;
    let columns = view_columns_from_aliases(&stmt.aliases, location)?;
    let (security_barrier, security_invoker) = view_reloptions(&stmt.options, location)?;
    let check_option = extract_check_option(stmt, location)?;

    // Extract a deparseable SELECT body from the query node. T4's
    // canonicalization pass will re-parse this string to fill
    // body_canonical and body_dependencies.
    let raw_body = extract_query_body(stmt.query.as_deref(), location)?;

    Ok(View {
        qname,
        columns,
        body_canonical: NormalizedBody::empty(),
        body_dependencies: Vec::new(),
        security_barrier,
        security_invoker,
        check_option,
        comment: None,
        raw_body,
        owner: None,
        grants: vec![],
    })
}

/// Deparse the query node of a `CREATE VIEW` into a SELECT SQL string.
///
/// The deparsed text may differ in whitespace and keyword case from the
/// original source, but is semantically equivalent. T4 canonicalizes it
/// further via [`NormalizedBody::from_sql`].
fn extract_query_body(
    query_node: Option<&pg_query::protobuf::Node>,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    let Some(node) = query_node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "CREATE VIEW missing query body".into(),
        });
    };
    let Some(node_inner) = &node.node else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "CREATE VIEW query body node is empty".into(),
        });
    };
    // Use NodeRef::deparse() which correctly sets PG_VERSION_NUM in the
    // internal ParseResult it builds.
    let deparsed = node_inner
        .to_ref()
        .deparse()
        .map_err(|e| ParseError::Structural {
            location: location.clone(),
            message: format!("failed to deparse view query body: {e}"),
        })?;
    if deparsed.trim().is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "deparsed view query body is empty (node type: {:?})",
                std::mem::discriminant(node_inner)
            ),
        });
    }
    Ok(deparsed)
}

/// Extract explicit column alias list from `CREATE VIEW v(a, b, ...) AS ...`.
///
/// Returns an empty vec when no alias list was provided (the common case).
/// When the explicit alias list is empty, columns are derived from the
/// SELECT target list. This derivation requires walking the SELECT AST,
/// which T4's AST canonicalization pass already does for `body_canonical`
/// and `body_dependencies`. T3 leaves `columns` as an empty Vec; T4 fills
/// it during the same AST walk per PG's column-naming algorithm:
///   1. `ResTarget.name` (explicit alias) wins
///   2. Otherwise, the rightmost name in a `ColumnRef` (`users.email` → "email")
///   3. Otherwise, `"?column?"` (PG's fallback)
///
/// See arch spec views sub-spec §5.1 for the AST-canonicalization-pass
/// contract that includes column derivation.
fn view_columns_from_aliases(
    aliases: &[pg_query::protobuf::Node],
    location: &SourceLocation,
) -> Result<Vec<ViewColumn>, ParseError> {
    if aliases.is_empty() {
        return Ok(Vec::new());
    }
    aliases
        .iter()
        .map(|node| {
            let Some(NodeEnum::String(s)) = node.node.as_ref() else {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: "expected string node in view alias list".into(),
                });
            };
            let name = shared::ident(&s.sval, location)?;
            Ok(ViewColumn {
                name,
                // Type is unknown at parse time (T3); T4 resolves it.
                // `ColumnType::Other { raw: "unresolved" }` is the sentinel.
                column_type: ColumnType::Other {
                    raw: "unresolved".to_string(),
                },
                comment: None,
            })
        })
        .collect()
}

/// Parse the `WITH (...)` reloptions list for `security_barrier`,
/// `security_invoker`, and `check_option` options.
///
/// Unknown options are rejected: silently swallowing them would discard
/// user intent with no diagnostic, which constitutes silent data loss.
fn view_reloptions(
    options: &[pg_query::protobuf::Node],
    location: &SourceLocation,
) -> Result<(Option<bool>, Option<bool>), ParseError> {
    let mut security_barrier: Option<bool> = None;
    let mut security_invoker: Option<bool> = None;

    for node in options {
        let Some(NodeEnum::DefElem(de)) = node.node.as_ref() else {
            continue;
        };
        match de.defname.as_str() {
            "security_barrier" => {
                security_barrier = Some(def_elem_bool(de, location)?);
            }
            "security_invoker" => {
                security_invoker = Some(def_elem_bool(de, location)?);
            }
            // check_option is handled by extract_check_option; allow it here
            // so it doesn't trip the unknown-option guard.
            "check_option" => {}
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "unsupported view reloption: {other} (pgevolve supports \
                         security_barrier, security_invoker, and check_option; see arch spec \
                         §13 lint-at-plan tier for opt-in via [[lint_waiver]] \
                         once future reloption support lands)"
                    ),
                });
            }
        }
    }

    Ok((security_barrier, security_invoker))
}

/// Extract `WITH [LOCAL | CASCADED] CHECK OPTION` from a `CREATE VIEW` AST.
///
/// Handles two forms:
/// 1. **SQL-clause form**: `WITH LOCAL CHECK OPTION` / `WITH CASCADED CHECK OPTION` /
///    bare `WITH CHECK OPTION` (PG default = Cascaded). Encoded in
///    `stmt.with_check_option` (an i32 enum from `ViewCheckOption`).
/// 2. **WITH-options form**: `WITH (check_option = 'local' | 'cascaded')`. Encoded
///    in `stmt.options` as a `DefElem` with `defname = "check_option"`.
///
/// When both are present PG enforces they agree; we prefer the SQL-clause
/// form (its value supersedes the options form).
fn extract_check_option(
    stmt: &ViewStmt,
    location: &SourceLocation,
) -> Result<Option<CheckOption>, ParseError> {
    // 1. SQL-clause form: stmt.with_check_option
    //    pg_query 6.x ViewCheckOption: 0=Undefined, 1=NoCheckOption, 2=Local, 3=Cascaded
    let sql_clause = match stmt.with_check_option {
        0 | 1 => None, // Undefined or NoCheckOption
        2 => Some(CheckOption::Local),
        3 => Some(CheckOption::Cascaded),
        other => return Err(ParseError::UnknownCheckOptionVariant(other)),
    };

    // If the SQL-clause form set a value, use it directly.
    if sql_clause.is_some() {
        return Ok(sql_clause);
    }

    // 2. WITH-options form: scan stmt.options for check_option DefElem.
    for opt in &stmt.options {
        let Some(NodeEnum::DefElem(de)) = opt.node.as_ref() else {
            continue;
        };
        if de.defname.as_str() != "check_option" {
            continue;
        }
        // Value is a String node containing 'local' or 'cascaded'.
        let val = def_elem_string(de, location)?;
        return match val.to_ascii_lowercase().as_str() {
            "local" => Ok(Some(CheckOption::Local)),
            "cascaded" => Ok(Some(CheckOption::Cascaded)),
            other => Err(ParseError::UnknownCheckOptionValue(other.to_string())),
        };
    }

    Ok(None)
}

/// Extract a string value from a `DefElem` node.
fn def_elem_string(
    de: &pg_query::protobuf::DefElem,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    let Some(arg_box) = de.arg.as_ref() else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("option {:?} missing value", de.defname),
        });
    };
    let Some(node) = arg_box.node.as_ref() else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("option {:?} has empty value node", de.defname),
        });
    };
    match node {
        NodeEnum::String(s) => Ok(s.sval.clone()),
        NodeEnum::AConst(c) => {
            if let Some(a_const::Val::Sval(s)) = c.val.as_ref() {
                Ok(s.sval.clone())
            } else {
                Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("option {:?} value is not a string", de.defname),
                })
            }
        }
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("option {:?} value has unexpected node type", de.defname),
        }),
    }
}

/// Extract a boolean from a `DefElem`.
///
/// When the arg is absent (bare `security_barrier` without `= true`) Postgres
/// treats it as `true`. We do the same.
fn def_elem_bool(
    de: &pg_query::protobuf::DefElem,
    location: &SourceLocation,
) -> Result<bool, ParseError> {
    let Some(arg_box) = de.arg.as_ref() else {
        // Bare option name — Postgres defaults to true.
        return Ok(true);
    };
    let Some(node) = arg_box.node.as_ref() else {
        return Ok(true);
    };
    match node {
        NodeEnum::Boolean(b) => Ok(b.boolval),
        NodeEnum::Integer(i) => Ok(i.ival != 0),
        NodeEnum::AConst(c) => aconst_to_bool(c, location),
        // pg_query 6.x encodes view reloption boolean values as String nodes
        // (e.g. `String { sval: "true" }`).
        NodeEnum::String(s) => str_to_bool(&s.sval, &de.defname, location),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unsupported value type for reloption {:?}", de.defname),
        }),
    }
}

fn str_to_bool(s: &str, defname: &str, location: &SourceLocation) -> Result<bool, ParseError> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "unrecognised boolean reloption value {other:?} for option {defname:?}"
            ),
        }),
    }
}

fn aconst_to_bool(c: &AConst, location: &SourceLocation) -> Result<bool, ParseError> {
    if c.isnull {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "NULL is not a valid boolean reloption value".into(),
        });
    }
    match c.val.as_ref() {
        Some(a_const::Val::Boolval(b)) => Ok(b.boolval),
        Some(a_const::Val::Ival(i)) => Ok(i.ival != 0),
        Some(a_const::Val::Sval(s)) => match s.sval.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            other => Err(ParseError::Structural {
                location: location.clone(),
                message: format!("unrecognised boolean reloption value {other:?}"),
            }),
        },
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "unsupported AConst type for boolean reloption".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::view::CheckOption;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_view(sql: &str) -> ViewStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::ViewStmt(s) = node else {
            panic!("expected ViewStmt, got: {node:?}");
        };
        *s
    }

    fn build(sql: &str) -> View {
        let stmt = parse_view(sql);
        build_view(&stmt, None, &loc()).expect("build_view")
    }

    #[test]
    fn simple_view() {
        let v = build("CREATE VIEW app.active_users AS SELECT id FROM app.users;");
        assert_eq!(v.qname.to_string(), "app.active_users");
        assert!(v.columns.is_empty());
        assert!(v.body_canonical.canonical_text().is_empty());
        assert!(v.security_barrier.is_none());
        assert!(v.security_invoker.is_none());
    }

    #[test]
    fn view_with_column_aliases() {
        let v = build("CREATE VIEW app.v(a, b) AS SELECT 1, 2;");
        assert_eq!(v.columns.len(), 2);
        assert_eq!(v.columns[0].name.as_str(), "a");
        assert_eq!(v.columns[1].name.as_str(), "b");
    }

    #[test]
    fn view_with_security_barrier() {
        let v = build("CREATE VIEW app.v WITH (security_barrier = true) AS SELECT 1;");
        assert_eq!(v.security_barrier, Some(true));
        assert!(v.security_invoker.is_none());
    }

    #[test]
    fn view_with_security_invoker() {
        let v = build("CREATE VIEW app.v WITH (security_invoker) AS SELECT 1;");
        assert_eq!(v.security_invoker, Some(true));
    }

    #[test]
    fn unqualified_view_uses_default_schema() {
        let stmt = parse_view("CREATE VIEW myview AS SELECT 1;");
        let app = Identifier::from_unquoted("app").unwrap();
        let v = build_view(&stmt, Some(&app), &loc()).unwrap();
        assert_eq!(v.qname.to_string(), "app.myview");
    }

    #[test]
    fn unqualified_view_without_schema_errors() {
        let stmt = parse_view("CREATE VIEW myview AS SELECT 1;");
        let err = build_view(&stmt, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }

    #[test]
    fn body_canonical_is_empty_sentinel() {
        let v = build("CREATE VIEW app.v AS SELECT 1;");
        assert!(v.body_canonical.canonical_text().is_empty());
        assert_eq!(v.body_canonical.canonical_hash(), &[0u8; 32]);
    }

    #[test]
    fn view_with_unsupported_reloption_rejects() {
        // An unknown option (not security_barrier, security_invoker, or check_option) should error.
        let stmt = parse_view("CREATE VIEW app.v WITH (unknown_option = true) AS SELECT 1;");
        let err = build_view(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural error, got: {other:?}"),
        };
        assert!(
            msg.contains("unknown_option"),
            "error message should mention 'unknown_option', got: {msg}"
        );
    }

    // ── WITH CHECK OPTION tests ───────────────────────────────────────────────

    #[test]
    fn create_view_with_local_check_option_parses() {
        let v = build("CREATE VIEW app.v AS SELECT 1 AS x WITH LOCAL CHECK OPTION;");
        assert_eq!(v.check_option, Some(CheckOption::Local));
    }

    #[test]
    fn create_view_with_cascaded_check_option_parses() {
        let v = build("CREATE VIEW app.v AS SELECT 1 AS x WITH CASCADED CHECK OPTION;");
        assert_eq!(v.check_option, Some(CheckOption::Cascaded));
    }

    #[test]
    fn create_view_bare_with_check_option_defaults_to_cascaded() {
        // PG treats bare WITH CHECK OPTION as CASCADED.
        let v = build("CREATE VIEW app.v AS SELECT 1 AS x WITH CHECK OPTION;");
        assert_eq!(v.check_option, Some(CheckOption::Cascaded));
    }

    #[test]
    fn create_view_with_options_form_parses() {
        let v = build("CREATE VIEW app.v WITH (check_option = 'local') AS SELECT 1 AS x;");
        assert_eq!(v.check_option, Some(CheckOption::Local));
    }

    #[test]
    fn create_view_no_check_option_is_none() {
        let v = build("CREATE VIEW app.v AS SELECT 1 AS x;");
        assert_eq!(v.check_option, None);
    }
}
