//! Source-side parser for `CREATE COLLATION qname (option = value, …)`.
//!
//! `pg_query` 6.x encodes this statement as a [`pg_query::protobuf::DefineStmt`]
//! with `kind = ObjectType::ObjectCollation`:
//! - `defnames` is a list of `String` nodes (1-2 parts: `[name]` or `[schema, name]`).
//! - `definition` is a list of `DefElem` nodes, one per option.
//!   - `provider` arrives as a `TypeName` wrapping a single `String` (bare keyword).
//!   - `locale`, `lc_collate`, `lc_ctype` arrive as plain `String` nodes
//!     (single-quoted string literals).
//!   - `deterministic` arrives as a `String { sval: "true"/"false" }` (bare keyword).
//!
//! The IR always stores `lc_collate` + `lc_ctype` separately; when the source
//! uses the `locale = 'X'` shorthand both fields are set to `X`. Mixing
//! `locale` with `lc_collate`/`lc_ctype` is rejected. Unknown options are
//! rejected with a clear error naming the bad key. The provider defaults to
//! [`CollationProvider::Libc`] when omitted; `deterministic` defaults to `true`.

use pg_query::NodeEnum;
use pg_query::protobuf::{DefElem, DefineStmt};

use crate::identifier::Identifier;
use crate::ir::collation::{Collation, CollationProvider};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`Collation`] from a `CREATE COLLATION` AST node.
///
/// * `default_schema` — filled in when the source omits the schema prefix.
/// * Returns [`ParseError::Structural`] for unknown options, missing locale,
///   or `locale` combined with `lc_collate`/`lc_ctype`.
pub(crate) fn build_collation(
    stmt: &DefineStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Collation, ParseError> {
    let qname = shared::qname_from_string_list(&stmt.defnames, default_schema, location)?;

    let mut provider: Option<CollationProvider> = None;
    let mut locale: Option<String> = None;
    let mut lc_collate: Option<String> = None;
    let mut lc_ctype: Option<String> = None;
    let mut deterministic: Option<bool> = None;

    for node in &stmt.definition {
        let Some(NodeEnum::DefElem(de)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE COLLATION {qname}: unexpected non-DefElem in option list"),
            });
        };
        match de.defname.as_str() {
            "provider" => {
                let raw = extract_keyword_or_string(de, "provider", &qname, location)?;
                provider = Some(parse_provider(&raw, &qname, location)?);
            }
            "locale" => {
                locale = Some(extract_string(de, "locale", &qname, location)?);
            }
            "lc_collate" => {
                lc_collate = Some(extract_string(de, "lc_collate", &qname, location)?);
            }
            "lc_ctype" => {
                lc_ctype = Some(extract_string(de, "lc_ctype", &qname, location)?);
            }
            "deterministic" => {
                let raw = extract_keyword_or_string(de, "deterministic", &qname, location)?;
                deterministic = Some(parse_bool(&raw, "deterministic", &qname, location)?);
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("CREATE COLLATION {qname}: unknown option `{other}`"),
                });
            }
        }
    }

    let (lc_collate, lc_ctype) = match (lc_collate, lc_ctype, locale) {
        (Some(c), Some(t), None) => (c, t),
        (None, None, Some(loc)) => (loc.clone(), loc),
        (None, None, None) => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE COLLATION {qname}: must specify either `locale` or both \
                     `lc_collate` and `lc_ctype`"
                ),
            });
        }
        _ => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE COLLATION {qname}: `locale` must not be combined with \
                     `lc_collate` / `lc_ctype`"
                ),
            });
        }
    };

    Ok(Collation {
        qname,
        provider: provider.unwrap_or(CollationProvider::Libc),
        lc_collate,
        lc_ctype,
        deterministic: deterministic.unwrap_or(true),
        version: None,
        owner: None,
        comment: None,
    })
}

/// Extract a bare-string value from a `DefElem.arg` that arrives as a
/// `NodeEnum::String` (quoted-literal options like `locale = 'C'`).
fn extract_string(
    de: &DefElem,
    option: &str,
    qname: &crate::identifier::QualifiedName,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    let arg = de
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE COLLATION {qname}: option `{option}` has no value"),
        })?;
    match arg {
        NodeEnum::String(s) => Ok(s.sval.clone()),
        NodeEnum::AConst(ac) => {
            use pg_query::protobuf::a_const::Val;
            match ac.val.as_ref() {
                Some(Val::Sval(s)) => Ok(s.sval.clone()),
                _ => Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "CREATE COLLATION {qname}: option `{option}` value must be a string"
                    ),
                }),
            }
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE COLLATION {qname}: option `{option}` value must be a string, got {:?}",
                std::mem::discriminant(other),
            ),
        }),
    }
}

/// Extract a bare keyword or string value from a `DefElem.arg`.
///
/// `pg_query` encodes bare keywords (`provider = libc`, `deterministic = false`)
/// as either `TypeName { names: [String] }` or a plain `String`, depending on
/// the parser path. This helper accepts both.
fn extract_keyword_or_string(
    de: &DefElem,
    option: &str,
    qname: &crate::identifier::QualifiedName,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    let arg = de
        .arg
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE COLLATION {qname}: option `{option}` has no value"),
        })?;
    match arg {
        NodeEnum::String(s) => Ok(s.sval.clone()),
        NodeEnum::TypeName(tn) => tn
            .names
            .iter()
            .rev()
            .find_map(|n| match n.node.as_ref() {
                Some(NodeEnum::String(s)) if !s.sval.is_empty() => Some(s.sval.clone()),
                _ => None,
            })
            .ok_or_else(|| ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE COLLATION {qname}: option `{option}` value is empty"),
            }),
        NodeEnum::Boolean(b) => Ok(b.boolval.to_string()),
        NodeEnum::AConst(ac) => {
            use pg_query::protobuf::a_const::Val;
            match ac.val.as_ref() {
                Some(Val::Sval(s)) => Ok(s.sval.clone()),
                Some(Val::Boolval(b)) => Ok(b.boolval.to_string()),
                _ => Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "CREATE COLLATION {qname}: option `{option}` has unrecognized value"
                    ),
                }),
            }
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE COLLATION {qname}: option `{option}` has unrecognized value kind {:?}",
                std::mem::discriminant(other),
            ),
        }),
    }
}

fn parse_provider(
    raw: &str,
    qname: &crate::identifier::QualifiedName,
    location: &SourceLocation,
) -> Result<CollationProvider, ParseError> {
    match raw.to_ascii_lowercase().as_str() {
        "libc" => Ok(CollationProvider::Libc),
        "icu" => Ok(CollationProvider::Icu),
        "builtin" => Ok(CollationProvider::Builtin),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE COLLATION {qname}: provider must be one of \
                 `libc`, `icu`, `builtin`; got `{other}`"
            ),
        }),
    }
}

fn parse_bool(
    raw: &str,
    option: &str,
    qname: &crate::identifier::QualifiedName,
    location: &SourceLocation,
) -> Result<bool, ParseError> {
    match raw.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" | "yes" => Ok(true),
        "false" | "off" | "0" | "no" => Ok(false),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE COLLATION {qname}: option `{option}` = `{other}` is not a boolean"
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::parse::Statement;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    /// Parse a single SQL statement through the same classify+dispatch path
    /// used by `parse_directory`, returning the resulting catalog (or first
    /// `ParseError`). Wraps just enough of the public surface to keep parser
    /// tests self-contained.
    fn parse_to_catalog(sql: &str) -> Result<Catalog, ParseError> {
        let parsed = pg_query::parse(sql).map_err(|e| ParseError::PgQuery {
            location: loc(),
            message: e.to_string(),
        })?;
        let mut cat = Catalog::empty();
        for raw in parsed.protobuf.stmts {
            let Some(node) = raw.stmt.and_then(|n| n.node) else {
                continue;
            };
            let stmt = Statement::classify(node, loc())?;
            match stmt {
                Statement::CreateCollation(s) => {
                    let coll = build_collation(&s, None, &loc())?;
                    cat.collations.push(coll);
                }
                other => {
                    return Err(ParseError::Structural {
                        location: loc(),
                        message: format!("test helper does not handle {other:?}"),
                    });
                }
            }
        }
        Ok(cat)
    }

    #[test]
    fn parse_libc_collation() {
        // PG folds unquoted identifiers to lowercase: `de_DE` → `de_de`. The
        // quoted-string `'de_DE.utf8'` payload is preserved verbatim.
        let cat = parse_to_catalog(
            "CREATE COLLATION app.de_DE (provider = libc, locale = 'de_DE.utf8');",
        )
        .unwrap();
        let c = &cat.collations[0];
        assert_eq!(c.qname.to_string(), "app.de_de");
        assert_eq!(c.provider, CollationProvider::Libc);
        assert_eq!(c.lc_collate, "de_DE.utf8");
        assert_eq!(c.lc_ctype, "de_DE.utf8");
        assert!(c.deterministic);
        assert!(c.version.is_none());
        assert!(c.owner.is_none());
        assert!(c.comment.is_none());
    }

    #[test]
    fn parse_icu_nondeterministic() {
        let cat = parse_to_catalog(
            "CREATE COLLATION app.ci (provider = icu, locale = 'und', deterministic = false);",
        )
        .unwrap();
        let c = &cat.collations[0];
        assert_eq!(c.provider, CollationProvider::Icu);
        assert!(!c.deterministic);
        assert_eq!(c.lc_collate, "und");
        assert_eq!(c.lc_ctype, "und");
    }

    #[test]
    fn parse_separate_lc_fields() {
        let cat = parse_to_catalog(
            "CREATE COLLATION app.x (provider = libc, lc_collate = 'C', lc_ctype = 'en_US.utf8');",
        )
        .unwrap();
        let c = &cat.collations[0];
        assert_eq!(c.lc_collate, "C");
        assert_eq!(c.lc_ctype, "en_US.utf8");
    }

    #[test]
    fn parse_defaults_provider_to_libc_and_deterministic_to_true() {
        let cat = parse_to_catalog("CREATE COLLATION app.x (locale = 'C');").unwrap();
        let c = &cat.collations[0];
        assert_eq!(c.provider, CollationProvider::Libc);
        assert!(c.deterministic);
    }

    #[test]
    fn parse_builtin_provider() {
        let cat =
            parse_to_catalog("CREATE COLLATION app.b (provider = builtin, locale = 'C');").unwrap();
        assert_eq!(cat.collations[0].provider, CollationProvider::Builtin);
    }

    #[test]
    fn parse_rejects_unknown_option() {
        let err =
            parse_to_catalog("CREATE COLLATION app.x (locale = 'C', bogus = 1);").unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("bogus"), "expected bad key named in: {msg}");
    }

    #[test]
    fn parse_rejects_locale_with_lc_fields() {
        let err = parse_to_catalog("CREATE COLLATION app.x (locale = 'C', lc_collate = 'C');")
            .unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("locale"),
            "expected error to mention locale: {msg}"
        );
    }

    #[test]
    fn parse_rejects_missing_locale() {
        let err = parse_to_catalog("CREATE COLLATION app.x (provider = libc);").unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("locale"),
            "expected error to mention locale: {msg}"
        );
    }

    #[test]
    fn parse_rejects_only_lc_collate() {
        // `lc_collate` alone (without `lc_ctype`) should fail.
        let err = parse_to_catalog("CREATE COLLATION app.x (provider = libc, lc_collate = 'C');")
            .unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn parse_rejects_invalid_provider() {
        let err = parse_to_catalog("CREATE COLLATION app.x (provider = nonsense, locale = 'C');")
            .unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("nonsense") && msg.contains("provider"),
            "expected error to mention bad provider value: {msg}"
        );
    }
}
