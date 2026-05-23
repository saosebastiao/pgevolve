//! `CREATE INDEX` → [`crate::ir::index::Index`].

use pg_query::NodeEnum;
use pg_query::protobuf::{self, IndexElem, IndexStmt, SortByDir, SortByNulls};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::index::{
    Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_expr;

/// Build an [`Index`] from a `CREATE INDEX` AST.
pub fn build_index(
    stmt: &IndexStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Index, ParseError> {
    let relation = stmt
        .relation
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CREATE INDEX missing target table".into(),
        })?;
    let table = shared::resolve_qname(relation, default_schema, location)?;

    // Indexes share the table's schema; the optional `idxname` is unqualified.
    let idx_name = if stmt.idxname.is_empty() {
        // Postgres autogenerates a name; we fall back to the relation name +
        // "_idx" — phase-3 catalog read carries the real name. For source DDL,
        // omitting the name yields a deterministic synthetic name that the
        // user can override.
        format!("{}_idx", relation.relname)
    } else {
        stmt.idxname.clone()
    };
    let qname = QualifiedName::new(table.schema.clone(), shared::ident(&idx_name, location)?);

    let method = parse_method(&stmt.access_method);

    let columns = stmt
        .index_params
        .iter()
        .map(|n| build_index_column(n, default_schema, location))
        .collect::<Result<Vec<_>, _>>()?;

    let include = stmt
        .index_including_params
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(NodeEnum::IndexElem(elem)) => shared::ident(&elem.name, location),
            _ => Err(ParseError::Structural {
                location: location.clone(),
                message: "INCLUDE element must be a bare column name".into(),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let predicate = stmt
        .where_clause
        .as_ref()
        .and_then(|w| w.node.as_ref())
        .map(|n| normalize_expr::from_pg_node(n, None, location))
        .transpose()?;

    let tablespace = if stmt.table_space.is_empty() {
        None
    } else {
        Some(shared::ident(&stmt.table_space, location)?)
    };

    Ok(Index {
        qname,
        on: IndexParent::Table(table),
        method,
        columns,
        include,
        unique: stmt.unique,
        nulls_not_distinct: stmt.nulls_not_distinct,
        predicate,
        tablespace,
        comment: None,
        storage: crate::ir::reloptions::IndexStorageOptions::default(),
    })
}

fn build_index_column(
    node: &protobuf::Node,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<IndexColumn, ParseError> {
    let Some(NodeEnum::IndexElem(elem)) = node.node.as_ref() else {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "expected IndexElem in index column list".into(),
        });
    };
    let expr = build_index_column_expr(elem, location)?;
    // Built-in collations and operator classes live in `pg_catalog`; treat
    // that as the default schema for these reference lookups.
    let pg_catalog = Identifier::from_unquoted("pg_catalog").map_err(|e| ParseError::Ir {
        location: location.clone(),
        source: crate::ir::IrError::InvalidIdentifier(e.to_string()),
    })?;
    let collation = build_qname_from_strings(&elem.collation, Some(&pg_catalog), location)?;
    let opclass = build_qname_from_strings(&elem.opclass, Some(&pg_catalog), location)?;
    let _ = default_schema;

    Ok(IndexColumn {
        expr,
        collation,
        opclass,
        sort_order: parse_sort_order(elem.ordering),
        nulls_order: parse_nulls_order(elem.nulls_ordering, elem.ordering),
    })
}

fn build_index_column_expr(
    elem: &IndexElem,
    location: &SourceLocation,
) -> Result<IndexColumnExpr, ParseError> {
    if !elem.name.is_empty() {
        return Ok(IndexColumnExpr::Column(shared::ident(
            &elem.name, location,
        )?));
    }
    let raw = elem
        .expr
        .as_ref()
        .and_then(|e| e.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "index element missing both name and expression".into(),
        })?;
    Ok(IndexColumnExpr::Expression(normalize_expr::from_pg_node(
        raw, None, location,
    )?))
}

fn build_qname_from_strings(
    nodes: &[protobuf::Node],
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Option<QualifiedName>, ParseError> {
    if nodes.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.as_str()),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        [name] => {
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "qualified name needs schema or directive".into(),
                })?;
            Ok(Some(QualifiedName::new(
                schema,
                shared::ident(name, location)?,
            )))
        }
        [schema, name] => Ok(Some(QualifiedName::new(
            shared::ident(schema, location)?,
            shared::ident(name, location)?,
        ))),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "qualified name must have one or two components".into(),
        }),
    }
}

fn parse_method(s: &str) -> IndexMethod {
    // Default to BTree both when explicitly named and when unknown — v0.1
    // corpus only exercises the listed kinds.
    match s.to_ascii_lowercase().as_str() {
        "hash" => IndexMethod::Hash,
        "gin" => IndexMethod::Gin,
        "gist" => IndexMethod::Gist,
        "brin" => IndexMethod::Brin,
        "spgist" => IndexMethod::Spgist,
        _ => IndexMethod::BTree,
    }
}

fn parse_sort_order(ordering: i32) -> SortOrder {
    match SortByDir::try_from(ordering).unwrap_or(SortByDir::Undefined) {
        SortByDir::SortbyDesc => SortOrder::Desc,
        _ => SortOrder::Asc,
    }
}

fn parse_nulls_order(nulls: i32, ordering: i32) -> NullsOrder {
    let parsed = SortByNulls::try_from(nulls).unwrap_or(SortByNulls::Undefined);
    match parsed {
        SortByNulls::SortbyNullsFirst => NullsOrder::NullsFirst,
        SortByNulls::SortbyNullsLast => NullsOrder::NullsLast,
        _ => {
            // Postgres default depends on direction: ASC → NULLS LAST,
            // DESC → NULLS FIRST.
            let dir = SortByDir::try_from(ordering).unwrap_or(SortByDir::Undefined);
            if matches!(dir, SortByDir::SortbyDesc) {
                NullsOrder::NullsFirst
            } else {
                NullsOrder::NullsLast
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn build(sql: &str) -> Index {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::IndexStmt(idx) = stmt else {
            panic!("not IndexStmt")
        };
        build_index(&idx, None, &loc()).expect("builds")
    }

    #[test]
    fn bare_btree_index() {
        let i = build("CREATE INDEX users_email_idx ON app.users (email);");
        assert_eq!(i.qname.to_string(), "app.users_email_idx");
        assert_eq!(i.on.qname().to_string(), "app.users");
        assert!(matches!(i.method, IndexMethod::BTree));
        assert_eq!(i.columns.len(), 1);
        assert!(matches!(
            &i.columns[0].expr,
            IndexColumnExpr::Column(c) if c.as_str() == "email"
        ));
        assert!(!i.unique);
    }

    #[test]
    fn unique_index() {
        let i = build("CREATE UNIQUE INDEX u ON app.t (a);");
        assert!(i.unique);
    }

    #[test]
    fn partial_index_predicate() {
        let i = build("CREATE INDEX i ON app.t (a) WHERE deleted_at IS NULL;");
        assert!(i.predicate.is_some());
    }

    #[test]
    fn include_columns_extracted() {
        let i = build("CREATE UNIQUE INDEX i ON app.t (a) INCLUDE (b, c);");
        assert_eq!(i.include.len(), 2);
        assert_eq!(i.include[0].as_str(), "b");
        assert_eq!(i.include[1].as_str(), "c");
    }

    #[test]
    fn expression_index() {
        let i = build("CREATE INDEX i ON app.t (lower(email));");
        assert!(matches!(&i.columns[0].expr, IndexColumnExpr::Expression(_)));
    }

    #[test]
    fn opclass_attached() {
        let i = build("CREATE INDEX i ON app.t (email text_pattern_ops);");
        assert!(i.columns[0].opclass.is_some());
    }

    #[test]
    fn nulls_not_distinct_unique() {
        let i = build("CREATE UNIQUE INDEX i ON app.t (a) NULLS NOT DISTINCT;");
        assert!(i.unique);
        assert!(i.nulls_not_distinct);
    }

    #[test]
    fn desc_default_nulls_first() {
        let i = build("CREATE INDEX i ON app.t (a DESC);");
        assert!(matches!(i.columns[0].sort_order, SortOrder::Desc));
        assert!(matches!(i.columns[0].nulls_order, NullsOrder::NullsFirst));
    }
}
