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
        ObjectType::ObjectColumn => {
            let parts = string_parts(stmt, location)?;
            let (table_qname, col_name) = split_column_target(&parts, default_schema, location)?;
            let table = catalog
                .tables
                .iter_mut()
                .find(|t| t.qname == table_qname)
                .ok_or_else(|| missing(location, "table", &table_qname.to_string()))?;
            let col = table
                .columns
                .iter_mut()
                .find(|c| c.name == col_name)
                .ok_or_else(|| missing(location, "column", &format!("{table_qname}.{col_name}")))?;
            col.comment = comment;
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
        other => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("unsupported COMMENT target kind: {other:?}"),
            });
        }
    }

    Ok(())
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
}
