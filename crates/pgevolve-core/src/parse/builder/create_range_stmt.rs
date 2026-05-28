//! Source-side parser for `CREATE TYPE … AS RANGE (…)`.
//!
//! Produces a [`UserType`] with `kind = UserTypeKind::Range { … }`.
//! The `pg_query` AST for this statement is [`pg_query::protobuf::CreateRangeStmt`]
//! with a `type_name: Vec<Node>` (String nodes) and a `params: Vec<Node>` list of
//! `DefElem` entries — one per `option = value` clause.

use pg_query::NodeEnum;
use pg_query::protobuf::{CreateRangeStmt, DefElem, Node, TypeName};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::user_type::{UserType, UserTypeKind};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`UserType`] from a `CREATE TYPE … AS RANGE` AST node.
///
/// * `default_schema` — filled in when the source omits the schema prefix.
/// * Requires `subtype = …`; rejects unknown option names with a clear
///   error naming the bad key.
/// * Accepts (all optional): `subtype_opclass`, `collation`, `canonical`,
///   `subtype_diff`, `multirange_type_name`.
#[allow(clippy::too_many_lines)] // option-list dispatch — splitting would scatter per-option decoding.
pub(crate) fn build_range(
    stmt: &CreateRangeStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<UserType, ParseError> {
    let qname = shared::qname_from_string_list(&stmt.type_name, default_schema, location)?;

    let mut subtype: Option<QualifiedName> = None;
    let mut subtype_opclass: Option<QualifiedName> = None;
    let mut collation: Option<QualifiedName> = None;
    let mut canonical: Option<QualifiedName> = None;
    let mut subtype_diff: Option<QualifiedName> = None;
    let mut multirange_type_name: Option<Identifier> = None;

    for node in &stmt.params {
        let Some(NodeEnum::DefElem(de)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TYPE {qname} AS RANGE: unexpected non-DefElem in option list",
                ),
            });
        };
        match de.defname.as_str() {
            "subtype" => {
                if subtype.is_some() {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "CREATE TYPE {qname} AS RANGE: duplicate `subtype` option",
                        ),
                    });
                }
                subtype = Some(qualified_name_from_defelem_typename(
                    de,
                    "subtype",
                    &qname,
                    default_schema,
                    location,
                )?);
            }
            "subtype_opclass" => {
                subtype_opclass = Some(qualified_name_from_defelem_typename(
                    de,
                    "subtype_opclass",
                    &qname,
                    default_schema,
                    location,
                )?);
            }
            "collation" => {
                collation = Some(qualified_name_from_defelem_typename(
                    de,
                    "collation",
                    &qname,
                    default_schema,
                    location,
                )?);
            }
            "canonical" => {
                canonical = Some(qualified_name_from_defelem_typename(
                    de,
                    "canonical",
                    &qname,
                    default_schema,
                    location,
                )?);
            }
            "subtype_diff" => {
                subtype_diff = Some(qualified_name_from_defelem_typename(
                    de,
                    "subtype_diff",
                    &qname,
                    default_schema,
                    location,
                )?);
            }
            "multirange_type_name" => {
                // PG syntax allows `multirange_type_name = schema.name`. The IR
                // stores the bare identifier; the schema (when present) is
                // forced to match the range type's schema by PG itself, so we
                // reject mismatches at parse time and store just the name.
                let qn = qualified_name_from_defelem_typename(
                    de,
                    "multirange_type_name",
                    &qname,
                    default_schema,
                    location,
                )?;
                if qn.schema != qname.schema {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "CREATE TYPE {qname} AS RANGE: multirange_type_name must live in \
                             the same schema as the range type ({})",
                            qname.schema.as_str(),
                        ),
                    });
                }
                multirange_type_name = Some(qn.name);
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("CREATE TYPE {qname} AS RANGE: unknown option `{other}`"),
                });
            }
        }
    }

    let subtype = subtype.ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: format!("CREATE TYPE {qname} AS RANGE: missing required `subtype` option"),
    })?;

    Ok(UserType {
        qname,
        kind: UserTypeKind::Range {
            subtype,
            subtype_opclass,
            collation,
            canonical,
            subtype_diff,
            multirange_type_name,
        },
        comment: None,
        owner: None,
        grants: vec![],
    })
}

/// Decode a `DefElem` whose argument is a `TypeName` (a list of String nodes)
/// into a [`QualifiedName`].
///
/// `option_name` is for error messages; `range_qname` and `default_schema` are
/// forwarded to provide context for unqualified references.
fn qualified_name_from_defelem_typename(
    de: &DefElem,
    option_name: &str,
    range_qname: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let arg_node =
        de.arg.as_ref().ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE TYPE {range_qname} AS RANGE: option `{option_name}` has no value",
            ),
        })?;
    let inner = arg_node
        .node
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE TYPE {range_qname} AS RANGE: option `{option_name}` value node empty",
            ),
        })?;
    let typename: &TypeName = match inner {
        NodeEnum::TypeName(t) => t,
        _ => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "CREATE TYPE {range_qname} AS RANGE: option `{option_name}` value must be a \
                     type/function name, not {kind}",
                    kind = node_kind_label(inner),
                ),
            });
        }
    };
    qname_from_typename_strings(&typename.names, default_schema, location)
}

/// Decode a sequence of `String` nodes (a `TypeName.names` list) into a
/// [`QualifiedName`]. Built-in names that arrive unqualified (e.g. `int4`)
/// are placed in the `pg_catalog` schema, mirroring PG's own resolution.
fn qname_from_typename_strings(
    nodes: &[Node],
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let parts: Vec<&str> = nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.as_str()),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        [name] => {
            // Unqualified: route built-in scalar types to pg_catalog; everything
            // else uses default_schema (or fails if missing).
            if is_builtin_scalar(name) {
                Ok(QualifiedName::new(
                    shared::ident("pg_catalog", location)?,
                    shared::ident(name, location)?,
                ))
            } else {
                let schema =
                    default_schema
                        .cloned()
                        .ok_or_else(|| ParseError::UnqualifiedName {
                            location: location.clone(),
                        })?;
                Ok(QualifiedName::new(schema, shared::ident(name, location)?))
            }
        }
        [schema, name] => Ok(QualifiedName::new(
            shared::ident(schema, location)?,
            shared::ident(name, location)?,
        )),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "expected 1 or 2 String nodes in qualified name, got {}",
                nodes.len(),
            ),
        }),
    }
}

/// PG built-in scalar type / opclass / collation names that are unambiguous
/// at parse time. Used only to qualify them under `pg_catalog` so the dep
/// graph knows they are not user-managed.
fn is_builtin_scalar(name: &str) -> bool {
    matches!(
        name,
        "int2"
            | "int4"
            | "int8"
            | "smallint"
            | "integer"
            | "bigint"
            | "numeric"
            | "decimal"
            | "real"
            | "double precision"
            | "float4"
            | "float8"
            | "text"
            | "varchar"
            | "char"
            | "bpchar"
            | "bytea"
            | "date"
            | "time"
            | "timestamp"
            | "timestamptz"
            | "interval"
            | "uuid"
            | "bool"
            | "boolean"
            | "jsonb"
            | "json"
            | "int4_ops"
            | "int8_ops"
            | "text_ops"
            | "timestamp_ops"
            | "timestamptz_ops"
            | "date_ops"
            | "numeric_ops"
            | "C"
            | "POSIX"
            | "default"
    )
}

const fn node_kind_label(node: &NodeEnum) -> &'static str {
    match node {
        NodeEnum::String(_) => "String",
        NodeEnum::TypeName(_) => "TypeName",
        NodeEnum::Integer(_) => "Integer",
        NodeEnum::Float(_) => "Float",
        NodeEnum::AConst(_) => "AConst",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_range(sql: &str) -> CreateRangeStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateRangeStmt(s) = node else {
            panic!("expected CreateRangeStmt, got: {node:?}");
        };
        s
    }

    fn build(sql: &str) -> UserType {
        let stmt = parse_range(sql);
        build_range(&stmt, None, &loc()).expect("build_range")
    }

    fn try_build(sql: &str) -> Result<UserType, ParseError> {
        let stmt = parse_range(sql);
        build_range(&stmt, None, &loc())
    }

    #[test]
    fn parse_range_with_subtype_only() {
        let ut = build("CREATE TYPE app.tsrange_co AS RANGE (subtype = timestamptz);");
        assert_eq!(ut.qname.to_string(), "app.tsrange_co");
        let UserTypeKind::Range {
            subtype,
            subtype_opclass,
            collation,
            canonical,
            subtype_diff,
            multirange_type_name,
        } = &ut.kind
        else {
            panic!("not range kind");
        };
        assert_eq!(subtype.to_string(), "pg_catalog.timestamptz");
        assert!(subtype_opclass.is_none());
        assert!(collation.is_none());
        assert!(canonical.is_none());
        assert!(subtype_diff.is_none());
        assert!(multirange_type_name.is_none());
    }

    #[test]
    fn parse_range_subtype_in_pg_catalog() {
        let ut = build("CREATE TYPE app.r AS RANGE (subtype = int4);");
        let UserTypeKind::Range { subtype, .. } = &ut.kind else {
            panic!("not range");
        };
        assert_eq!(subtype.schema.as_str(), "pg_catalog");
        assert_eq!(subtype.name.as_str(), "int4");
    }

    #[test]
    fn parse_range_with_opclass_and_multirange_name() {
        let ut = build(
            "CREATE TYPE app.r AS RANGE (subtype = int4, subtype_opclass = int4_ops, multirange_type_name = app.r_mr);",
        );
        let UserTypeKind::Range {
            subtype_opclass,
            multirange_type_name,
            ..
        } = &ut.kind
        else {
            panic!("not range");
        };
        assert_eq!(
            subtype_opclass.as_ref().map(ToString::to_string),
            Some("pg_catalog.int4_ops".into())
        );
        assert_eq!(
            multirange_type_name
                .as_ref()
                .map(|i| i.as_str().to_string()),
            Some("r_mr".into()),
        );
    }

    #[test]
    fn parse_range_rejects_unknown_option() {
        let err =
            try_build("CREATE TYPE app.bad AS RANGE (subtype = int4, bogus = 1);").unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("bogus"), "expected bad key in: {msg}");
    }

    #[test]
    fn parse_range_rejects_missing_subtype() {
        let err = try_build("CREATE TYPE app.bad AS RANGE (multirange_type_name = app.bad_mr);")
            .unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("subtype"), "expected subtype in: {msg}");
    }

    #[test]
    fn parse_range_rejects_duplicate_subtype() {
        let err = try_build("CREATE TYPE app.bad AS RANGE (subtype = int4, subtype = int8);")
            .unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn parse_range_rejects_multirange_in_different_schema() {
        let err = try_build(
            "CREATE TYPE app.r AS RANGE (subtype = int4, multirange_type_name = other.r_mr);",
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn parse_range_unqualified_with_default_schema() {
        let stmt = parse_range("CREATE TYPE r AS RANGE (subtype = int4);");
        let app = Identifier::from_unquoted("app").unwrap();
        let ut = build_range(&stmt, Some(&app), &loc()).unwrap();
        assert_eq!(ut.qname.to_string(), "app.r");
    }

    #[test]
    fn parse_range_unqualified_without_schema_errors() {
        let stmt = parse_range("CREATE TYPE r AS RANGE (subtype = int4);");
        let err = build_range(&stmt, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }
}
