//! `CREATE EXTENSION` → [`crate::ir::extension::Extension`].
//!
//! Accepts:
//! ```sql
//! CREATE EXTENSION [IF NOT EXISTS] name
//!     [WITH] [SCHEMA schema_name] [VERSION 'version']
//! ```
//!
//! Rejects `CASCADE`, `FROM old_version`, and any unknown options with a
//! [`ParseError::Structural`] error explaining the restriction.

use pg_query::NodeEnum;
use pg_query::protobuf::CreateExtensionStmt;

use crate::ir::extension::Extension;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build an [`Extension`] from a `CREATE EXTENSION` AST node.
///
/// * `IF NOT EXISTS` is accepted (it has no effect on the desired-state IR).
/// * `WITH SCHEMA s` sets `schema` to `Some(s)`.
/// * `VERSION 'v'` sets `version` to `Some(v)`.
/// * `CASCADE` is rejected — pgevolve requires all extensions to be declared
///   explicitly so the dependency graph is visible.
/// * `FROM old_version` (`old_version` `DefElem`) is rejected — it is a
///   migration concept not applicable to declarative desired-state files.
pub(crate) fn build_extension(
    stmt: &CreateExtensionStmt,
    location: &SourceLocation,
) -> Result<Extension, ParseError> {
    let name = shared::ident(&stmt.extname, location)?;

    let mut schema: Option<crate::identifier::Identifier> = None;
    let mut version: Option<String> = None;

    for opt_node in &stmt.options {
        let Some(NodeEnum::DefElem(de)) = opt_node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE EXTENSION {name}: unexpected option node"),
            });
        };

        match de.defname.as_str() {
            "schema" => {
                let s = string_from_def_elem(de).ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: format!("CREATE EXTENSION {name}: SCHEMA option missing value"),
                })?;
                schema = Some(shared::ident(&s, location)?);
            }
            "new_version" => {
                let v = string_from_def_elem(de).ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: format!("CREATE EXTENSION {name}: VERSION option missing value"),
                })?;
                version = Some(v);
            }
            "cascade" => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "CREATE EXTENSION {name}: CASCADE is not supported — \
                         declare every required extension explicitly so that \
                         pgevolve can manage it"
                    ),
                });
            }
            "old_version" => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "CREATE EXTENSION {name}: FROM old_version is not supported in \
                         source files — declare the desired state with VERSION"
                    ),
                });
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("CREATE EXTENSION {name}: unknown option {other:?}"),
                });
            }
        }
    }

    Ok(Extension {
        name,
        schema,
        version,
        comment: None,
    })
}

/// Extract a string value from a `DefElem.arg`.
fn string_from_def_elem(de: &pg_query::protobuf::DefElem) -> Option<String> {
    let arg = de.arg.as_ref()?;
    match arg.node.as_ref()? {
        NodeEnum::String(s) => Some(s.sval.clone()),
        NodeEnum::Integer(i) => Some(i.ival.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_extension(sql: &str) -> CreateExtensionStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateExtensionStmt(s) = node else {
            panic!("expected CreateExtensionStmt, got: {node:?}");
        };
        s
    }

    fn build(sql: &str) -> Extension {
        let stmt = parse_extension(sql);
        build_extension(&stmt, &loc()).expect("build")
    }

    #[test]
    fn parses_bare() {
        let ext = build("CREATE EXTENSION pgcrypto;");
        assert_eq!(ext.name.as_str(), "pgcrypto");
        assert!(ext.schema.is_none());
        assert!(ext.version.is_none());
        assert!(ext.comment.is_none());
    }

    #[test]
    fn parses_with_schema() {
        let ext = build("CREATE EXTENSION pgcrypto WITH SCHEMA app;");
        assert_eq!(ext.name.as_str(), "pgcrypto");
        assert_eq!(
            ext.schema
                .as_ref()
                .map(crate::identifier::Identifier::as_str),
            Some("app")
        );
        assert!(ext.version.is_none());
    }

    #[test]
    fn parses_with_version() {
        let ext = build("CREATE EXTENSION pgcrypto VERSION '1.3';");
        assert_eq!(ext.name.as_str(), "pgcrypto");
        assert!(ext.schema.is_none());
        assert_eq!(ext.version.as_deref(), Some("1.3"));
    }

    #[test]
    fn parses_if_not_exists_with_schema_and_version() {
        let ext = build("CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public VERSION '1.6';");
        assert_eq!(ext.name.as_str(), "pg_trgm");
        assert_eq!(
            ext.schema
                .as_ref()
                .map(crate::identifier::Identifier::as_str),
            Some("public")
        );
        assert_eq!(ext.version.as_deref(), Some("1.6"));
        assert!(ext.comment.is_none());
    }

    #[test]
    fn rejects_cascade() {
        let stmt = parse_extension("CREATE EXTENSION pgcrypto CASCADE;");
        let err = build_extension(&stmt, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.to_lowercase().contains("cascade"),
            "should mention CASCADE: {msg}"
        );
    }

    #[test]
    fn rejects_from_clause() {
        // FROM old_version maps to the "old_version" DefElem in pg_query.
        // pg_query parses `CREATE EXTENSION foo FROM 'bar'` but the FROM
        // syntax was removed in PG14; we build the node manually to test
        // the rejection path.
        use pg_query::protobuf::{CreateExtensionStmt, DefElem, Node};

        let old_version_opt = Node {
            node: Some(NodeEnum::DefElem(Box::new(DefElem {
                defnamespace: String::new(),
                defname: "old_version".into(),
                arg: Some(Box::new(Node {
                    node: Some(NodeEnum::String(pg_query::protobuf::String {
                        sval: "1.0".into(),
                    })),
                })),
                defaction: 0,
                location: 0,
            }))),
        };

        let stmt = CreateExtensionStmt {
            extname: "pgcrypto".into(),
            if_not_exists: false,
            options: vec![old_version_opt],
        };

        let err = build_extension(&stmt, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("old_version") || msg.to_lowercase().contains("from"),
            "should mention old_version/FROM: {msg}"
        );
    }
}
