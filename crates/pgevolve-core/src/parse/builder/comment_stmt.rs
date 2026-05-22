//! `COMMENT ON ...` — apply a comment to an existing IR object in a partial
//! [`Catalog`].
//!
//! The COMMENT statement is order-dependent: it must come *after* the object's
//! `CREATE`. We resolve targets by walking the partial catalog and setting
//! `comment` on the matching object. If no target is found, we emit a
//! [`ParseError::Structural`] with the missing qname.

use pg_query::NodeEnum;
use pg_query::protobuf::{CommentStmt, ObjectType};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::function::{ArgMode, FunctionArg, NormalizedArgTypes};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply a `COMMENT ON ...` to a partial catalog.
pub fn apply_comment(
    stmt: &CommentStmt,
    catalog: &mut Catalog,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(), ParseError> {
    let kind = ObjectType::try_from(stmt.objtype).unwrap_or(ObjectType::Undefined);
    let comment = if stmt.comment.is_empty() {
        None
    } else {
        Some(stmt.comment.clone())
    };
    apply_comment_inner(stmt, catalog, default_schema, location, kind, comment)
}

#[allow(clippy::too_many_lines)]
fn apply_comment_inner(
    stmt: &CommentStmt,
    catalog: &mut Catalog,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
    kind: ObjectType,
    comment: Option<String>,
) -> Result<(), ParseError> {
    match kind {
        ObjectType::ObjectSchema => {
            let name = single_string(stmt, location)?;
            let schema_name = shared::ident(&name, location)?;
            let schema = catalog
                .schemas
                .iter_mut()
                .find(|s| s.name == schema_name)
                .ok_or_else(|| missing(location, "schema", &schema_name.to_string()))?;
            schema.comment = comment;
        }
        ObjectType::ObjectTable => {
            let qname = qualified_name(stmt, default_schema, location)?;
            let table = catalog
                .tables
                .iter_mut()
                .find(|t| t.qname == qname)
                .ok_or_else(|| missing(location, "table", &qname.to_string()))?;
            table.comment = comment;
        }
        ObjectType::ObjectIndex => {
            let qname = qualified_name(stmt, default_schema, location)?;
            let index = catalog
                .indexes
                .iter_mut()
                .find(|i| i.qname == qname)
                .ok_or_else(|| missing(location, "index", &qname.to_string()))?;
            index.comment = comment;
        }
        ObjectType::ObjectSequence => {
            let qname = qualified_name(stmt, default_schema, location)?;
            let seq = catalog
                .sequences
                .iter_mut()
                .find(|s| s.qname == qname)
                .ok_or_else(|| missing(location, "sequence", &qname.to_string()))?;
            seq.comment = comment;
        }
        ObjectType::ObjectTabconstraint => {
            let parts = string_parts(stmt, location)?;
            let (table_qname, con_name) = split_column_target(&parts, default_schema, location)?;
            let table = catalog
                .tables
                .iter_mut()
                .find(|t| t.qname == table_qname)
                .ok_or_else(|| missing(location, "table", &table_qname.to_string()))?;
            let con = table
                .constraints
                .iter_mut()
                .find(|c| c.qname.name == con_name)
                .ok_or_else(|| {
                    missing(location, "constraint", &format!("{table_qname}.{con_name}"))
                })?;
            con.comment = comment;
        }
        ObjectType::ObjectView => {
            let qname = qualified_name(stmt, default_schema, location)?;
            let view = catalog
                .views
                .iter_mut()
                .find(|v| v.qname == qname)
                .ok_or_else(|| missing(location, "view", &qname.to_string()))?;
            view.comment = comment;
        }
        ObjectType::ObjectMatview => {
            let qname = qualified_name(stmt, default_schema, location)?;
            let mv = catalog
                .materialized_views
                .iter_mut()
                .find(|m| m.qname == qname)
                .ok_or_else(|| missing(location, "materialized view", &qname.to_string()))?;
            mv.comment = comment;
        }
        ObjectType::ObjectColumn => {
            let parts = string_parts(stmt, location)?;
            let (obj_qname, col_name) = split_column_target(&parts, default_schema, location)?;
            apply_column_comment(catalog, location, &obj_qname, &col_name, comment)?;
        }
        ObjectType::ObjectType | ObjectType::ObjectDomain => {
            // `COMMENT ON TYPE app.foo IS '...'`
            // `COMMENT ON DOMAIN app.foo IS '...'`
            // pg_query encodes these with a TypeName node (not a List of strings),
            // so we need a different extraction path.
            let qname = qualified_name_from_type_name(stmt, default_schema, location)?;
            let ty = catalog
                .types
                .iter_mut()
                .find(|t| t.qname == qname)
                .ok_or_else(|| missing(location, "type", &qname.to_string()))?;
            ty.comment = comment;
        }
        ObjectType::ObjectFunction => {
            let (qname, arg_types_normalized) =
                qname_and_args_from_object_with_args(stmt, default_schema, location)?;
            let func = catalog
                .functions
                .iter_mut()
                .find(|f| f.qname == qname && f.arg_types_normalized == arg_types_normalized)
                .ok_or_else(|| missing(location, "function", &qname.to_string()))?;
            func.comment = comment;
        }
        ObjectType::ObjectProcedure => {
            let qname = qname_from_object_with_args(stmt, default_schema, location)?;
            let proc = catalog
                .procedures
                .iter_mut()
                .find(|p| p.qname == qname)
                .ok_or_else(|| missing(location, "procedure", &qname.to_string()))?;
            proc.comment = comment;
        }
        ObjectType::ObjectExtension => {
            // `COMMENT ON EXTENSION name IS '...'`
            // pg_query encodes the extension name as a bare String node.
            let name_str = single_string(stmt, location)?;
            let ext_name = shared::ident(&name_str, location)?;
            let ext = catalog
                .extensions
                .iter_mut()
                .find(|e| e.name == ext_name)
                .ok_or_else(|| missing(location, "extension", &ext_name.to_string()))?;
            ext.comment = comment;
        }
        ObjectType::ObjectTrigger => {
            // `COMMENT ON TRIGGER trigger_name ON schema.table IS '...'`
            // pg_query encodes this as a List of string parts:
            //   [schema, table, trigger_name]   (3 parts, schema-qualified table)
            //   [table, trigger_name]           (2 parts, unqualified table — handled
            //                                    by inferring the default schema)
            let parts = string_parts(stmt, location)?;
            let (table_qn, ident) = split_column_target(&parts, default_schema, location)?;
            // The trigger qname mirrors the owning table's schema.
            let qname = crate::identifier::QualifiedName::new(table_qn.schema, ident);
            let trg = catalog
                .triggers
                .iter_mut()
                .find(|t| t.qname == qname)
                .ok_or_else(|| missing(location, "trigger", &qname.to_string()))?;
            trg.comment = comment;
        }
        other => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("unsupported COMMENT target kind: {other:?}"),
            });
        }
    }
    Ok(())
}

/// Apply a column comment, trying tables then views then MVs.
fn apply_column_comment(
    catalog: &mut Catalog,
    location: &SourceLocation,
    obj_qname: &QualifiedName,
    col_name: &Identifier,
    comment: Option<String>,
) -> Result<(), ParseError> {
    if let Some(table) = catalog.tables.iter_mut().find(|t| t.qname == *obj_qname) {
        let col = table
            .columns
            .iter_mut()
            .find(|c| c.name == *col_name)
            .ok_or_else(|| missing(location, "column", &format!("{obj_qname}.{col_name}")))?;
        col.comment = comment;
    } else if let Some(view) = catalog.views.iter_mut().find(|v| v.qname == *obj_qname) {
        let col = view
            .columns
            .iter_mut()
            .find(|c| c.name == *col_name)
            .ok_or_else(|| missing(location, "view column", &format!("{obj_qname}.{col_name}")))?;
        col.comment = comment;
    } else if let Some(mv) = catalog
        .materialized_views
        .iter_mut()
        .find(|m| m.qname == *obj_qname)
    {
        let col = mv
            .columns
            .iter_mut()
            .find(|c| c.name == *col_name)
            .ok_or_else(|| {
                missing(
                    location,
                    "materialized view column",
                    &format!("{obj_qname}.{col_name}"),
                )
            })?;
        col.comment = comment;
    } else {
        return Err(missing(location, "table/view/mv", &obj_qname.to_string()));
    }
    Ok(())
}

/// Extract the qname from an `ObjectWithArgs` node (used by `COMMENT ON
/// FUNCTION` and `COMMENT ON PROCEDURE`). The `objname` field is a list of
/// String nodes identical in structure to a `CreateFunctionStmt.funcname` list.
fn qname_from_object_with_args(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let obj = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT missing object reference".into(),
        })?;
    let NodeEnum::ObjectWithArgs(owa) = obj else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON FUNCTION/PROCEDURE expected ObjectWithArgs, got {:?}",
                std::mem::discriminant(obj)
            ),
        });
    };
    shared::qname_from_string_list(&owa.objname, default_schema, location)
}

/// Extract both qname and a [`NormalizedArgTypes`] from an `ObjectWithArgs`
/// node (used by `COMMENT ON FUNCTION`). The `objargs` field is a list of
/// `TypeName` nodes, one per declared argument type.
fn qname_and_args_from_object_with_args(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(QualifiedName, NormalizedArgTypes), ParseError> {
    let obj = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT missing object reference".into(),
        })?;
    let NodeEnum::ObjectWithArgs(owa) = obj else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON FUNCTION expected ObjectWithArgs, got {:?}",
                std::mem::discriminant(obj)
            ),
        });
    };
    let qname = shared::qname_from_string_list(&owa.objname, default_schema, location)?;
    // Parse arg types from `objargs` (list of TypeName nodes).
    let mut args: Vec<FunctionArg> = Vec::with_capacity(owa.objargs.len());
    for node in &owa.objargs {
        let Some(NodeEnum::TypeName(tn)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!(
                    "COMMENT ON FUNCTION {qname}: expected TypeName in objargs, got {:?}",
                    node.node.as_ref().map(std::mem::discriminant)
                ),
            });
        };
        let ty = shared::type_name_to_column_type(tn, location)?;
        args.push(FunctionArg {
            name: None,
            mode: ArgMode::In,
            ty,
            default: None,
        });
    }
    let arg_types_normalized = NormalizedArgTypes::from_args(&args);
    Ok((qname, arg_types_normalized))
}

fn missing(location: &SourceLocation, kind: &'static str, name: &str) -> ParseError {
    ParseError::Structural {
        location: location.clone(),
        message: format!("COMMENT references {kind} {name} which is not defined in source"),
    }
}

/// Extract a `String` payload from `stmt.object` (for SCHEMA targets).
fn single_string(stmt: &CommentStmt, location: &SourceLocation) -> Result<String, ParseError> {
    let obj = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT missing object reference".into(),
        })?;
    if let NodeEnum::String(s) = obj {
        return Ok(s.sval.clone());
    }
    if let NodeEnum::List(list) = obj
        && let Some(NodeEnum::String(s)) = list.items.first().and_then(|n| n.node.as_ref())
    {
        return Ok(s.sval.clone());
    }
    Err(ParseError::Structural {
        location: location.clone(),
        message: "COMMENT object expected a string identifier".into(),
    })
}

/// Extract a list of String parts from `stmt.object` (for List targets).
fn string_parts(stmt: &CommentStmt, location: &SourceLocation) -> Result<Vec<String>, ParseError> {
    let obj = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT missing object reference".into(),
        })?;
    let NodeEnum::List(list) = obj else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "COMMENT object expected a qualified-name list".into(),
        });
    };
    let mut out = Vec::with_capacity(list.items.len());
    for n in &list.items {
        let Some(NodeEnum::String(s)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: "COMMENT qualified-name parts must be string identifiers".into(),
            });
        };
        out.push(s.sval.clone());
    }
    Ok(out)
}

/// Resolve a possibly-qualified table/index/sequence name from `stmt.object`.
fn qualified_name(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let parts = string_parts(stmt, location)?;
    match parts.as_slice() {
        [name] => {
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::UnqualifiedName {
                    location: location.clone(),
                })?;
            Ok(QualifiedName::new(schema, shared::ident(name, location)?))
        }
        [schema, name] => Ok(QualifiedName::new(
            shared::ident(schema, location)?,
            shared::ident(name, location)?,
        )),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "COMMENT object name must have one or two qualified components".into(),
        }),
    }
}

/// Resolve a qualified type/domain name from `stmt.object` for `COMMENT ON TYPE`
/// and `COMMENT ON DOMAIN`. `pg_query` encodes these as a `TypeName` node (unlike
/// tables/views which use a `List` of String nodes).
fn qualified_name_from_type_name(
    stmt: &CommentStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    use pg_query::NodeEnum;
    let obj = stmt
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "COMMENT missing object reference".into(),
        })?;
    // pg_query represents COMMENT ON TYPE / DOMAIN via a TypeName node whose
    // `names` field is a list of String nodes (schema, name).
    let NodeEnum::TypeName(type_name) = obj else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: format!("COMMENT ON TYPE expected TypeName node, got {obj:?}"),
        });
    };
    let parts: Vec<String> = type_name
        .names
        .iter()
        .filter_map(|n| n.node.as_ref())
        .filter_map(|n| {
            if let NodeEnum::String(s) = n {
                Some(s.sval.clone())
            } else {
                None
            }
        })
        .collect();
    match parts.as_slice() {
        [name] => {
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::UnqualifiedName {
                    location: location.clone(),
                })?;
            Ok(QualifiedName::new(schema, shared::ident(name, location)?))
        }
        [schema, name] => Ok(QualifiedName::new(
            shared::ident(schema, location)?,
            shared::ident(name, location)?,
        )),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!(
                "COMMENT ON TYPE object name must have 1-2 qualified components; got {parts:?}"
            ),
        }),
    }
}

/// Split a 2- or 3-part list into `(table_qname, last_component)`.
/// Used for COLUMN (`schema.table.col` or `table.col`) and TABLE CONSTRAINT
/// (`schema.table.con` or `table.con`).
fn split_column_target(
    parts: &[String],
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(QualifiedName, Identifier), ParseError> {
    match parts {
        [table, last] => {
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::UnqualifiedName {
                    location: location.clone(),
                })?;
            Ok((
                QualifiedName::new(schema, shared::ident(table, location)?),
                shared::ident(last, location)?,
            ))
        }
        [schema, table, last] => Ok((
            QualifiedName::new(
                shared::ident(schema, location)?,
                shared::ident(table, location)?,
            ),
            shared::ident(last, location)?,
        )),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "expected `[schema.]table.<column-or-constraint>` for this COMMENT target"
                .into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{Constraint as IrConstraint, ConstraintKind, Deferrable};
    use crate::ir::index::{
        Index as IrIndex, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder,
        SortOrder,
    };
    use crate::ir::schema::Schema;
    use crate::ir::sequence::Sequence;
    use crate::ir::table::Table;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn seed_catalog() -> Catalog {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("users"),
            columns: vec![Column {
                name: id("email"),
                ty: ColumnType::Text,
                nullable: true,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                storage: None,
                compression: None,
                comment: None,
            }],
            constraints: vec![IrConstraint {
                qname: qn("users_pkey"),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![id("email")],
                    include: vec![],
                },
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        c.indexes.push(IrIndex {
            qname: qn("users_email_idx"),
            on: IndexParent::Table(qn("users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("email")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        });
        c.sequences.push(Sequence {
            qname: qn("seq1"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        });
        c
    }

    fn parse_first(sql: &str) -> CommentStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CommentStmt(c) = stmt else {
            panic!("not CommentStmt")
        };
        *c
    }

    #[test]
    fn comment_on_schema() {
        let mut c = seed_catalog();
        let stmt = parse_first("COMMENT ON SCHEMA app IS 'application';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(c.schemas[0].comment.as_deref(), Some("application"));
    }

    #[test]
    fn comment_on_table() {
        let mut c = seed_catalog();
        let stmt = parse_first("COMMENT ON TABLE app.users IS 'people';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(c.tables[0].comment.as_deref(), Some("people"));
    }

    #[test]
    fn comment_on_column() {
        let mut c = seed_catalog();
        let stmt = parse_first("COMMENT ON COLUMN app.users.email IS 'their email';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(
            c.tables[0].columns[0].comment.as_deref(),
            Some("their email")
        );
    }

    #[test]
    fn comment_on_index() {
        let mut c = seed_catalog();
        let stmt = parse_first("COMMENT ON INDEX app.users_email_idx IS 'lookup';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(c.indexes[0].comment.as_deref(), Some("lookup"));
    }

    #[test]
    fn comment_on_sequence() {
        let mut c = seed_catalog();
        let stmt = parse_first("COMMENT ON SEQUENCE app.seq1 IS 'counter';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(c.sequences[0].comment.as_deref(), Some("counter"));
    }

    #[test]
    fn comment_on_constraint() {
        let mut c = seed_catalog();
        let stmt = parse_first("COMMENT ON CONSTRAINT users_pkey ON app.users IS 'primary';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(
            c.tables[0].constraints[0].comment.as_deref(),
            Some("primary")
        );
    }

    #[test]
    fn comment_on_missing_table_errors() {
        let mut c = seed_catalog();
        let stmt = parse_first("COMMENT ON TABLE app.does_not_exist IS 'x';");
        let err = apply_comment(&stmt, &mut c, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn empty_comment_clears_comment_field() {
        let mut c = seed_catalog();
        c.tables[0].comment = Some("old".into());
        let stmt = parse_first("COMMENT ON TABLE app.users IS NULL;");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert!(c.tables[0].comment.is_none());
    }

    #[test]
    fn comment_on_type_sets_comment() {
        use crate::ir::user_type::{UserType, UserTypeKind};
        let mut c = seed_catalog();
        let qname = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("mytype").unwrap(),
        );
        c.types.push(UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Enum { values: vec![] },
            comment: None,
        });
        let stmt = parse_first("COMMENT ON TYPE app.mytype IS 'a type';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        let ty = c.types.iter().find(|t| t.qname == qname).unwrap();
        assert_eq!(ty.comment.as_deref(), Some("a type"));
    }

    #[test]
    fn comment_on_domain_sets_comment() {
        use crate::ir::column_type::ColumnType;
        use crate::ir::user_type::{UserType, UserTypeKind};
        let mut c = seed_catalog();
        let qname = QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("email").unwrap(),
        );
        c.types.push(UserType {
            qname: qname.clone(),
            kind: UserTypeKind::Domain {
                base: ColumnType::Text,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
        });
        let stmt = parse_first("COMMENT ON DOMAIN app.email IS 'email domain';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        let ty = c.types.iter().find(|t| t.qname == qname).unwrap();
        assert_eq!(ty.comment.as_deref(), Some("email domain"));
    }

    #[test]
    fn comment_on_function_sets_comment() {
        use crate::ir::function::{
            ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
            ReturnType, SecurityMode, Volatility,
        };
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = seed_catalog();
        let args = vec![FunctionArg {
            name: Some(id("x")),
            mode: ArgMode::In,
            ty: ColumnType::Integer,
            default: None,
        }];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        c.functions.push(Function {
            qname: qn("double"),
            args,
            arg_types_normalized,
            return_type: ReturnType::Scalar {
                ty: ColumnType::Integer,
            },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT $1 * 2").unwrap(),
            body_dependencies: vec![],
            volatility: Volatility::Immutable,
            strict: true,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Safe,
            leakproof: false,
            cost: Some(1.0),
            rows: None,
            comment: None,
        });
        let stmt = parse_first("COMMENT ON FUNCTION app.double(integer) IS 'doubles the value';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(c.functions[0].comment.as_deref(), Some("doubles the value"));
    }

    #[test]
    fn comment_on_procedure_sets_comment() {
        use crate::ir::function::{FunctionLanguage, SecurityMode};
        use crate::ir::procedure::Procedure;
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = seed_catalog();
        c.procedures.push(Procedure {
            qname: qn("greet"),
            args: vec![],
            language: FunctionLanguage::PlPgSql,
            body: NormalizedBody::empty(),
            body_dependencies: vec![],
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
        });
        let stmt = parse_first("COMMENT ON PROCEDURE app.greet IS 'greeting procedure';");
        apply_comment(&stmt, &mut c, None, &loc()).unwrap();
        assert_eq!(
            c.procedures[0].comment.as_deref(),
            Some("greeting procedure")
        );
    }
}
