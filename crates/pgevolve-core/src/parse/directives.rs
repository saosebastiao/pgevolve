//! `-- @pgevolve` file-level directives.
//!
//! Directives are SQL line-comments of the form
//!
//! ```text
//! -- @pgevolve <key>=<value>[ <key>=<value>...]
//! ```
//!
//! In phase 2 the only source-side directive is `schema=<name>`, which sets the
//! default schema for unqualified object names within the file. Plan-format
//! directives (`step=`, `group=`, etc.) are emitted by the planner; we recognize
//! and ignore them here so that round-tripping a plan back through the parser
//! does not error.
//!
//! Additionally, `dep:` directives (Decision 11) declare explicit dependencies
//! for PL/pgSQL bodies that reference objects via dynamic SQL:
//!
//! ```text
//! -- @pgevolve dep: schema.object_name
//! ```

use std::path::Path;

use crate::identifier::Identifier;
use crate::parse::error::{ParseError, SourceLocation};

/// A `-- @pgevolve dep: <qualified-name>` directive.
///
/// Closes the PL/pgSQL dynamic-SQL gap (Decision 11). For v0.1 the
/// recognizer parses these but no consumer reads them yet; v0.2
/// function sub-spec will populate `AstDeclared` edges from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepDirective {
    /// Fully qualified target. Stored verbatim; the consumer resolves.
    pub target: String,
}

/// Directives extracted from a single source file's leading comment block.
#[derive(Debug, Clone, Default)]
pub struct FileDirectives {
    /// Default schema for unqualified object names in this file.
    pub schema: Option<Identifier>,
    /// Explicit dependency declarations for dynamic-SQL bodies.
    pub deps: Vec<DepDirective>,
}

/// Plan-format directive keys that are silently ignored when reading source SQL.
///
/// These are emitted by the planner in later phases; the parser must accept them
/// without complaint so that plans round-trip.
const IGNORED_PLAN_KEYS: &[&str] = &[
    "plan",
    "step",
    "group",
    "kind",
    "destructive",
    "intent_id",
    "version",
    "created",
    "source_rev",
    "target",
    "intents_required",
    "transactional",
    "targets",
];

/// Scan the file's leading comment block for `-- @pgevolve` directives.
///
/// Scanning stops at the first non-empty, non-comment line — directives must
/// appear before any SQL.
pub fn extract_file_directives(sql: &str, file: &Path) -> Result<FileDirectives, ParseError> {
    let mut out = FileDirectives::default();
    for (line_no, raw_line) in sql.lines().enumerate() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("--") else {
            break;
        };
        let rest = rest.trim();
        let Some(payload) = rest.strip_prefix("@pgevolve") else {
            continue;
        };
        let payload = payload.trim();
        // `dep:` is a special form that takes the rest of the line as its
        // target value, rather than using the `key=value` tokenization.
        if let Some(target_raw) = payload.strip_prefix("dep:") {
            let target = target_raw.trim().to_string();
            if target.is_empty() {
                return Err(ParseError::InvalidDirective {
                    location: SourceLocation::new(file.into(), line_no + 1, 1),
                    message: "dep: requires a non-empty target".into(),
                });
            }
            out.deps.push(DepDirective { target });
            continue;
        }
        for kv in payload.split_whitespace() {
            let Some((k, v)) = kv.split_once('=') else {
                return Err(ParseError::InvalidDirective {
                    location: SourceLocation::new(file.into(), line_no + 1, 1),
                    message: format!("expected `key=value`, got {kv:?}"),
                });
            };
            apply_kv(&mut out, k, v, file, line_no)?;
        }
    }
    Ok(out)
}

fn apply_kv(
    out: &mut FileDirectives,
    key: &str,
    value: &str,
    file: &Path,
    line_no: usize,
) -> Result<(), ParseError> {
    match key {
        "schema" => {
            if value.is_empty() {
                return Err(ParseError::InvalidDirective {
                    location: SourceLocation::new(file.into(), line_no + 1, 1),
                    message: "schema= requires a non-empty identifier".into(),
                });
            }
            let id =
                Identifier::from_unquoted(value).map_err(|e| ParseError::InvalidDirective {
                    location: SourceLocation::new(file.into(), line_no + 1, 1),
                    message: format!("invalid schema identifier: {e}"),
                })?;
            out.schema = Some(id);
            Ok(())
        }
        k if IGNORED_PLAN_KEYS.contains(&k) => Ok(()),
        other => Err(ParseError::InvalidDirective {
            location: SourceLocation::new(file.into(), line_no + 1, 1),
            message: format!("unknown directive key: {other}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse(sql: &str) -> Result<FileDirectives, ParseError> {
        extract_file_directives(sql, &PathBuf::from("test.sql"))
    }

    #[test]
    fn schema_directive_recognized() {
        let d = parse("-- @pgevolve schema=app\nCREATE TABLE x (id int);").unwrap();
        assert_eq!(
            d.schema.as_ref().map(crate::identifier::Identifier::as_str),
            Some("app")
        );
    }

    #[test]
    fn schema_directive_lowercases() {
        let d = parse("-- @pgevolve schema=Foo\n").unwrap();
        assert_eq!(
            d.schema.as_ref().map(crate::identifier::Identifier::as_str),
            Some("foo")
        );
    }

    #[test]
    fn directive_in_multi_line_header() {
        let sql = "\
            -- header comment\n\
            -- another comment\n\
            -- @pgevolve schema=billing\n\
            CREATE TABLE x (id int);\n";
        let d = parse(sql).unwrap();
        assert_eq!(
            d.schema.as_ref().map(crate::identifier::Identifier::as_str),
            Some("billing")
        );
    }

    #[test]
    fn directive_after_first_sql_is_ignored() {
        let sql = "\
            CREATE TABLE x (id int);\n\
            -- @pgevolve schema=app\n";
        let d = parse(sql).unwrap();
        assert!(d.schema.is_none());
    }

    #[test]
    fn empty_value_rejected() {
        let err = parse("-- @pgevolve schema=\n").unwrap_err();
        assert!(matches!(err, ParseError::InvalidDirective { .. }));
    }

    #[test]
    fn missing_equals_rejected() {
        let err = parse("-- @pgevolve schema\n").unwrap_err();
        match err {
            ParseError::InvalidDirective { message, .. } => {
                assert!(message.contains("key=value"), "got: {message}");
            }
            other => panic!("expected InvalidDirective, got {other:?}"),
        }
    }

    #[test]
    fn unknown_key_rejected() {
        let err = parse("-- @pgevolve unknown=x\n").unwrap_err();
        match err {
            ParseError::InvalidDirective { message, .. } => {
                assert!(message.contains("unknown"), "got: {message}");
            }
            other => panic!("expected InvalidDirective, got {other:?}"),
        }
    }

    #[test]
    fn plan_format_keys_silently_ignored() {
        let d = parse("-- @pgevolve schema=app step=1 group=migrations kind=create\n").unwrap();
        assert_eq!(
            d.schema.as_ref().map(crate::identifier::Identifier::as_str),
            Some("app")
        );
    }

    #[test]
    fn empty_lines_in_header_are_skipped() {
        let sql = "\n\n-- @pgevolve schema=app\nCREATE TABLE x (id int);\n";
        let d = parse(sql).unwrap();
        assert_eq!(
            d.schema.as_ref().map(crate::identifier::Identifier::as_str),
            Some("app")
        );
    }

    #[test]
    fn no_directive_returns_default() {
        let d = parse("-- just a comment\nCREATE TABLE x (id int);\n").unwrap();
        assert!(d.schema.is_none());
    }

    // --- dep: directive tests ---

    #[test]
    fn dep_directive_parses() {
        let d = parse("-- @pgevolve dep: app.audit_log\n").unwrap();
        assert_eq!(d.deps.len(), 1);
        assert_eq!(d.deps[0].target, "app.audit_log");
    }

    #[test]
    fn dep_directive_trims_whitespace() {
        let d = parse("-- @pgevolve dep:   app.users.email   \n").unwrap();
        assert_eq!(d.deps.len(), 1);
        assert_eq!(d.deps[0].target, "app.users.email");
    }

    #[test]
    fn multiple_dep_directives_collected() {
        let sql = "-- @pgevolve dep: app.foo\n-- @pgevolve dep: app.bar\nSELECT 1;\n";
        let d = parse(sql).unwrap();
        let targets: Vec<&str> = d.deps.iter().map(|dep| dep.target.as_str()).collect();
        assert!(
            targets.contains(&"app.foo"),
            "expected app.foo in {targets:?}"
        );
        assert!(
            targets.contains(&"app.bar"),
            "expected app.bar in {targets:?}"
        );
    }

    #[test]
    fn dep_directive_coexists_with_schema_directive() {
        let sql = "-- @pgevolve schema=app\n-- @pgevolve dep: app.audit_log\n";
        let d = parse(sql).unwrap();
        assert_eq!(d.schema.as_ref().map(Identifier::as_str), Some("app"));
        assert_eq!(d.deps.len(), 1);
        assert_eq!(d.deps[0].target, "app.audit_log");
    }

    #[test]
    fn dep_after_first_sql_is_ignored() {
        // Directives after the first non-comment SQL line are not scanned.
        let sql = "SELECT 1;\n-- @pgevolve dep: app.foo\n";
        let d = parse(sql).unwrap();
        assert!(d.deps.is_empty());
    }

    #[test]
    fn dep_directive_with_empty_target_is_rejected() {
        let err = parse("-- @pgevolve dep:\n").unwrap_err();
        match err {
            ParseError::InvalidDirective { message, .. } => {
                assert!(message.contains("non-empty target"), "got: {message}");
            }
            other => panic!("expected InvalidDirective, got {other:?}"),
        }
    }
}
