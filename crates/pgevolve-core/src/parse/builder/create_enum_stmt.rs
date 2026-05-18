//! Source-side parser for `CREATE TYPE x AS ENUM (...)`.
//!
//! Produces a [`UserType`] with `kind = UserTypeKind::Enum { values }`.
//! Enum values receive `sort_order` 1.0, 2.0, 3.0, … matching PG's
//! `pg_enum.enumsortorder` semantics.

use std::collections::BTreeSet;

use pg_query::NodeEnum;
use pg_query::protobuf::CreateEnumStmt;

use crate::identifier::Identifier;
use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`UserType`] with `kind = Enum` from a `CREATE TYPE … AS ENUM`
/// AST node.
///
/// * `default_schema` — filled in when the source omits the schema prefix
///   (e.g. a `-- @pgevolve schema=app` directive is in effect).
/// * Duplicate labels within the same enum are rejected (PG does the same).
/// * An empty value list is rejected.
pub(crate) fn build_enum(
    stmt: &CreateEnumStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<UserType, ParseError> {
    let qname = shared::qname_from_string_list(&stmt.type_name, default_schema, location)?;

    if stmt.vals.is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE TYPE {qname} AS ENUM requires at least one value"),
        });
    }

    let mut values: Vec<EnumValue> = Vec::with_capacity(stmt.vals.len());
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (idx, node) in stmt.vals.iter().enumerate() {
        let label = string_val_from_node(node, location)?;
        if !seen.insert(label.clone()) {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE TYPE {qname} AS ENUM: duplicate label {label:?}"),
            });
        }
        #[allow(clippy::cast_precision_loss)]
        let sort_order = (idx as f32) + 1.0;
        values.push(EnumValue {
            name: label,
            sort_order,
        });
    }

    Ok(UserType {
        qname,
        kind: UserTypeKind::Enum { values },
        comment: None,
    })
}

/// Extract a string value from a node that must be `NodeEnum::String`.
fn string_val_from_node(
    node: &pg_query::protobuf::Node,
    location: &SourceLocation,
) -> Result<String, ParseError> {
    match node.node.as_ref() {
        Some(NodeEnum::String(s)) if !s.sval.is_empty() => Ok(s.sval.clone()),
        Some(NodeEnum::AConst(c)) => {
            // pg_query sometimes wraps enum labels in AConst Sval nodes.
            use pg_query::protobuf::a_const;
            match c.val.as_ref() {
                Some(a_const::Val::Sval(s)) if !s.sval.is_empty() => Ok(s.sval.clone()),
                _ => Err(ParseError::Structural {
                    location: location.clone(),
                    message: "expected non-empty string AConst for enum label".into(),
                }),
            }
        }
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "expected string node for enum label".into(),
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

    fn parse_enum(sql: &str) -> CreateEnumStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateEnumStmt(s) = node else {
            panic!("expected CreateEnumStmt, got: {node:?}");
        };
        s
    }

    fn build(sql: &str) -> UserType {
        let stmt = parse_enum(sql);
        build_enum(&stmt, None, &loc()).expect("build_enum")
    }

    #[test]
    fn qualified_enum_three_values() {
        let ut = build("CREATE TYPE app.order_status AS ENUM ('pending', 'shipped', 'delivered');");
        assert_eq!(ut.qname.to_string(), "app.order_status");
        let crate::ir::user_type::UserTypeKind::Enum { values } = ut.kind else {
            panic!("not enum kind");
        };
        assert_eq!(values.len(), 3);
        assert_eq!(values[0].name, "pending");
        assert_eq!(values[0].sort_order.to_bits(), 1.0_f32.to_bits());
        assert_eq!(values[1].name, "shipped");
        assert_eq!(values[1].sort_order.to_bits(), 2.0_f32.to_bits());
        assert_eq!(values[2].name, "delivered");
        assert_eq!(values[2].sort_order.to_bits(), 3.0_f32.to_bits());
        assert!(ut.comment.is_none());
    }

    #[test]
    fn unqualified_enum_uses_default_schema() {
        let stmt = parse_enum("CREATE TYPE order_status AS ENUM ('a');");
        let app = Identifier::from_unquoted("app").unwrap();
        let ut = build_enum(&stmt, Some(&app), &loc()).unwrap();
        assert_eq!(ut.qname.to_string(), "app.order_status");
    }

    #[test]
    fn unqualified_without_schema_errors() {
        let stmt = parse_enum("CREATE TYPE order_status AS ENUM ('a');");
        let err = build_enum(&stmt, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }

    #[test]
    fn single_value_enum() {
        let ut = build("CREATE TYPE app.singleton AS ENUM ('only');");
        let crate::ir::user_type::UserTypeKind::Enum { values } = ut.kind else {
            panic!("not enum kind");
        };
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].name, "only");
        assert_eq!(values[0].sort_order.to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn duplicate_label_rejected() {
        let stmt = parse_enum("CREATE TYPE app.bad AS ENUM ('x', 'y', 'x');");
        let err = build_enum(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("duplicate"), "expected duplicate in: {msg}");
        assert!(msg.contains("\"x\""), "expected label in: {msg}");
    }

    #[test]
    fn empty_enum_rejected() {
        // pg_query itself rejects empty ENUM lists at parse time, so this
        // tests that our empty-check path is defensive. We simulate by
        // constructing a synthetic statement.
        let synthetic = CreateEnumStmt {
            type_name: {
                use pg_query::protobuf::{Node, String as PgString};
                vec![
                    Node {
                        node: Some(NodeEnum::String(PgString { sval: "app".into() })),
                    },
                    Node {
                        node: Some(NodeEnum::String(PgString {
                            sval: "myenum".into(),
                        })),
                    },
                ]
            },
            vals: vec![],
        };
        let err = build_enum(&synthetic, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("at least one value"), "unexpected: {msg}");
    }
}
