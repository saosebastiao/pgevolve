//! Source-side parser for `CREATE TYPE x AS (...)` (composite types).

use pg_query::NodeEnum;
use pg_query::protobuf::CompositeTypeStmt;

use crate::identifier::Identifier;
use crate::ir::user_type::{CompositeAttribute, UserType, UserTypeKind};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`UserType`] with `kind = Composite` from a `CREATE TYPE ... AS (...)`
/// AST node.
///
/// * `default_schema` — filled in when the source omits the schema prefix
///   (e.g. a `-- @pgevolve schema=app` directive is in effect).
/// * Empty composite (zero attributes) is rejected.
/// * Duplicate attribute names within the same composite are rejected.
pub(crate) fn build_composite(
    stmt: &CompositeTypeStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<UserType, ParseError> {
    let typevar = stmt
        .typevar
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CREATE TYPE ... AS: missing target name".into(),
        })?;

    let qname = shared::resolve_qname(typevar, default_schema, location)?;

    if stmt.coldeflist.is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE TYPE {qname} AS (...) requires at least one attribute"),
        });
    }

    let mut attributes: Vec<CompositeAttribute> = Vec::with_capacity(stmt.coldeflist.len());
    let mut seen = std::collections::BTreeSet::<String>::new();

    for node in &stmt.coldeflist {
        let Some(NodeEnum::ColumnDef(cd)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE TYPE {qname}: unexpected attribute node"),
            });
        };
        if cd.colname.is_empty() {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE TYPE {qname}: composite attribute missing name"),
            });
        }
        let attr_name = shared::ident(&cd.colname, location)?;
        if !seen.insert(attr_name.as_str().to_string()) {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TYPE {qname}: duplicate attribute name {:?}",
                    attr_name.as_str(),
                ),
            });
        }
        let type_name = cd
            .type_name
            .as_ref()
            .ok_or_else(|| ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TYPE {qname}: attribute {} missing type",
                    attr_name.as_str(),
                ),
            })?;
        let ty = shared::type_name_to_column_type(type_name, location).map_err(|e| match e {
            ParseError::Structural {
                location: loc,
                message,
            } => ParseError::Structural {
                location: loc,
                message: format!(
                    "CREATE TYPE {qname}: attribute {} has unsupported type — {message}",
                    attr_name.as_str(),
                ),
            },
            other => other,
        })?;
        // Composite attribute collations are not modeled in v0.2.
        attributes.push(CompositeAttribute {
            name: attr_name,
            ty,
            collation: None,
        });
    }

    Ok(UserType {
        qname,
        kind: UserTypeKind::Composite { attributes },
        comment: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use pg_query::protobuf::CompositeTypeStmt as PgCompositeTypeStmt;

    use crate::ir::column_type::ColumnType;
    use crate::ir::user_type::UserTypeKind;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_composite(sql: &str) -> PgCompositeTypeStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CompositeTypeStmt(s) = node else {
            panic!("expected CompositeTypeStmt, got: {node:?}");
        };
        s
    }

    fn build(sql: &str) -> UserType {
        let stmt = parse_composite(sql);
        build_composite(&stmt, None, &loc()).expect("build_composite")
    }

    #[test]
    fn simple_composite_three_text_attributes() {
        let ut = build("CREATE TYPE app.address AS (street text, city text, zip text);");
        assert_eq!(ut.qname.to_string(), "app.address");
        let UserTypeKind::Composite { attributes } = ut.kind else {
            panic!("not composite kind");
        };
        assert_eq!(attributes.len(), 3);
        assert_eq!(attributes[0].name.as_str(), "street");
        assert_eq!(attributes[0].ty, ColumnType::Text);
        assert_eq!(attributes[1].name.as_str(), "city");
        assert_eq!(attributes[1].ty, ColumnType::Text);
        assert_eq!(attributes[2].name.as_str(), "zip");
        assert_eq!(attributes[2].ty, ColumnType::Text);
        assert!(attributes.iter().all(|a| a.collation.is_none()));
        assert!(ut.comment.is_none());
    }

    #[test]
    fn mixed_types_text_integer_numeric() {
        let ut =
            build("CREATE TYPE app.line_item AS (sku text, qty integer, price numeric(10, 2));");
        assert_eq!(ut.qname.to_string(), "app.line_item");
        let UserTypeKind::Composite { attributes } = ut.kind else {
            panic!("not composite kind");
        };
        assert_eq!(attributes.len(), 3);
        assert_eq!(attributes[0].name.as_str(), "sku");
        assert_eq!(attributes[0].ty, ColumnType::Text);
        assert_eq!(attributes[1].name.as_str(), "qty");
        assert_eq!(attributes[1].ty, ColumnType::Integer);
        assert_eq!(attributes[2].name.as_str(), "price");
        assert_eq!(
            attributes[2].ty,
            ColumnType::Numeric {
                precision: Some(10),
                scale: Some(2)
            }
        );
    }

    #[test]
    fn unqualified_name_uses_default_schema() {
        let stmt = parse_composite("CREATE TYPE address AS (street text);");
        let app = Identifier::from_unquoted("app").unwrap();
        let ut = build_composite(&stmt, Some(&app), &loc()).unwrap();
        assert_eq!(ut.qname.to_string(), "app.address");
    }

    #[test]
    fn unqualified_without_schema_errors() {
        let stmt = parse_composite("CREATE TYPE address AS (street text);");
        let err = build_composite(&stmt, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }

    #[test]
    fn duplicate_attribute_name_rejected() {
        let stmt = parse_composite("CREATE TYPE app.bad AS (x text, x integer);");
        let err = build_composite(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("duplicate"), "expected 'duplicate' in: {msg}");
        assert!(
            msg.contains("\"x\"") || msg.contains('x'),
            "expected attr name in: {msg}"
        );
    }

    #[test]
    fn empty_composite_rejected() {
        // pg_query itself rejects `CREATE TYPE x AS ()` as a syntax error
        // before this builder ever sees the statement, so we construct a
        // synthetic protobuf with an empty `coldeflist` to exercise the
        // belt-and-suspenders guard directly.
        use pg_query::protobuf::{CompositeTypeStmt as PgCompositeTypeStmt, RangeVar};
        let synthetic = PgCompositeTypeStmt {
            typevar: Some(RangeVar {
                schemaname: "app".into(),
                relname: "empty".into(),
                ..Default::default()
            }),
            coldeflist: vec![],
        };
        let err = build_composite(&synthetic, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("at least one attribute"),
            "expected 'at least one attribute' in: {msg}",
        );
    }
}
