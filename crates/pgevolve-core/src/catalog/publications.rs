//! Decode `pg_catalog` publication rows into `Publication` IR.
//!
//! Three queries:
//!   - `pg_publication`           → name + owner + scope flag + publish + comment
//!   - `pg_publication_rel`       → per-table membership (+ row filter PG15+, + column list PG15+)
//!   - `pg_publication_namespace` → per-schema membership (PG15+)
//!
//! Row filter text is fed through `reparse_expression_text` so source-side
//! and catalog-side canonical forms compare equal (same canon as CHECK / USING).

// `CatalogError` embeds `IrError` and `ParseError`, both of which are large.
// Boxing them would add indirection noise without measurable benefit —
// errors here are cold-path (catalog reads, not hot loops).
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use crate::catalog::CatalogQuery;
use crate::catalog::assemble::reparse_expression_text;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::publication::{PublicationScope, PublishKinds, PublishedTable};

// ---- constant query tags used in error context ----------------------------

const Q_PUB: CatalogQuery = CatalogQuery::Publications;
const Q_REL: CatalogQuery = CatalogQuery::PublicationRel;
const Q_NS: CatalogQuery = CatalogQuery::PublicationNamespace;
const Q_ATTR: CatalogQuery = CatalogQuery::PublicationAttributes;

// ---- PartialPublication ---------------------------------------------------

/// Decoded `pg_publication` row; scope inputs not yet assembled.
pub struct PartialPublication {
    pub oid: i64,
    pub name: Identifier,
    pub owner: Option<Identifier>,
    pub all_tables: bool,
    pub publish: PublishKinds,
    pub publish_via_partition_root: bool,
    pub comment: Option<String>,
}

pub fn decode_publication_row(row: &Row) -> Result<PartialPublication, CatalogError> {
    let name_str = row.get_text(Q_PUB, "name")?;
    let owner_str = row.get_text(Q_PUB, "owner")?;
    let comment_str = row.get_text(Q_PUB, "comment")?;

    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: Q_PUB,
        column: "name".to_string(),
        message: format!("invalid publication name {name_str:?}: {e}"),
    })?;

    let owner = if owner_str.is_empty() {
        None
    } else {
        Some(
            Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::BadColumnType {
                query: Q_PUB,
                column: "owner".to_string(),
                message: format!("invalid owner name {owner_str:?}: {e}"),
            })?,
        )
    };

    let comment = if comment_str.is_empty() {
        None
    } else {
        Some(comment_str)
    };

    Ok(PartialPublication {
        oid: row.get_int(Q_PUB, "oid")?,
        name,
        owner,
        all_tables: row.get_bool(Q_PUB, "all_tables")?,
        publish: PublishKinds {
            insert: row.get_bool(Q_PUB, "pub_insert")?,
            update: row.get_bool(Q_PUB, "pub_update")?,
            delete: row.get_bool(Q_PUB, "pub_delete")?,
            truncate: row.get_bool(Q_PUB, "pub_truncate")?,
        },
        publish_via_partition_root: row.get_bool(Q_PUB, "publish_via_partition_root")?,
        comment,
    })
}

// ---- PartialPublicationRel ------------------------------------------------

/// One `pg_publication_rel` row decoded but not yet attached to its parent
/// publication (caller groups by `pub_oid`).
pub struct PartialPublicationRel {
    pub pub_oid: i64,
    pub qname: QualifiedName,
    /// Raw `pg_get_expr` text for the row filter, or `None` if absent.
    pub row_filter_sql: Option<String>,
    /// Column attnums from `prattrs`, or `None` if absent / PG14.
    pub col_attnums: Option<Vec<i64>>,
    pub rel_oid: i64,
}

pub fn decode_publication_rel_row(row: &Row) -> Result<PartialPublicationRel, CatalogError> {
    let schema_str = row.get_text(Q_REL, "schema")?;
    let table_str = row.get_text(Q_REL, "table_name")?;

    let schema =
        Identifier::from_unquoted(&schema_str).map_err(|e| CatalogError::BadColumnType {
            query: Q_REL,
            column: "schema".to_string(),
            message: format!("invalid schema {schema_str:?}: {e}"),
        })?;
    let table = Identifier::from_unquoted(&table_str).map_err(|e| CatalogError::BadColumnType {
        query: Q_REL,
        column: "table_name".to_string(),
        message: format!("invalid table {table_str:?}: {e}"),
    })?;

    let col_attnums = if row.is_null("col_attnums") {
        None
    } else {
        Some(row.get_int_array(Q_REL, "col_attnums")?)
    };

    Ok(PartialPublicationRel {
        pub_oid: row.get_int(Q_REL, "pub_oid")?,
        qname: QualifiedName::new(schema, table),
        row_filter_sql: row.get_opt_text(Q_REL, "row_filter")?,
        col_attnums,
        rel_oid: row.get_int(Q_REL, "rel_oid")?,
    })
}

// ---- PartialPublicationNamespace ------------------------------------------

pub struct PartialPublicationNamespace {
    pub pub_oid: i64,
    pub schema: Identifier,
}

pub fn decode_publication_namespace_row(
    row: &Row,
) -> Result<PartialPublicationNamespace, CatalogError> {
    let schema_str = row.get_text(Q_NS, "schema")?;
    let schema =
        Identifier::from_unquoted(&schema_str).map_err(|e| CatalogError::BadColumnType {
            query: Q_NS,
            column: "schema".to_string(),
            message: format!("invalid schema {schema_str:?}: {e}"),
        })?;
    Ok(PartialPublicationNamespace {
        pub_oid: row.get_int(Q_NS, "pub_oid")?,
        schema,
    })
}

// ---- PublicationAttribute (attnum resolver) --------------------------------

/// One row from `PUBLICATION_ATTRIBUTES_QUERY`.
pub struct PublicationAttribute {
    pub rel_oid: i64,
    pub attnum: i64,
    pub attname: String,
}

pub fn decode_publication_attribute_row(row: &Row) -> Result<PublicationAttribute, CatalogError> {
    Ok(PublicationAttribute {
        rel_oid: row.get_int(Q_ATTR, "rel_oid")?,
        attnum: row.get_int(Q_ATTR, "attnum")?,
        attname: row.get_text(Q_ATTR, "attname")?,
    })
}

// ---- Helpers ---------------------------------------------------------------

/// Convert a slice of attnum values into resolved column-name identifiers.
///
/// `attname_by_attnum` maps `attnum → name` for the table's columns and is
/// pre-built by the assembler from the `PublicationAttributes` query.
pub fn resolve_column_names(
    attnums: &[i64],
    attname_by_attnum: &BTreeMap<i64, String>,
) -> Result<Vec<Identifier>, CatalogError> {
    attnums
        .iter()
        .map(|n| {
            let name = attname_by_attnum
                .get(n)
                .ok_or_else(|| CatalogError::DanglingReference {
                    kind: "publication column attnum",
                    what: format!("attnum {n}"),
                })?;
            Identifier::from_unquoted(name).map_err(|e| CatalogError::BadColumnType {
                query: Q_ATTR,
                column: "attname".to_string(),
                message: format!("invalid column name {name:?}: {e}"),
            })
        })
        .collect()
}

/// Build a `PublishedTable` from a decoded rel row and resolved column names.
///
/// Row filter SQL is fed through `reparse_expression_text` so it normalizes
/// the same way as CHECK / USING expressions on the source side.
pub fn assemble_published_table(
    rel: PartialPublicationRel,
    columns: Option<Vec<Identifier>>,
) -> Result<PublishedTable, CatalogError> {
    let row_filter = rel
        .row_filter_sql
        .map(|sql| reparse_expression_text(&sql))
        .transpose()?;

    Ok(PublishedTable {
        qname: rel.qname,
        row_filter,
        columns,
    })
}

/// Construct `PublicationScope` from grouped rel/namespace rows.
pub fn build_scope(
    all_tables: bool,
    rels: Vec<PartialPublicationRel>,
    column_resolver: impl Fn(&PartialPublicationRel) -> Result<Option<Vec<Identifier>>, CatalogError>,
    namespaces: Vec<PartialPublicationNamespace>,
) -> Result<PublicationScope, CatalogError> {
    if all_tables {
        return Ok(PublicationScope::AllTables);
    }

    let mut tables = Vec::with_capacity(rels.len());
    for r in rels {
        let cols = column_resolver(&r)?;
        tables.push(assemble_published_table(r, cols)?);
    }
    let schemas = namespaces.into_iter().map(|n| n.schema).collect();
    Ok(PublicationScope::Selective { schemas, tables })
}

// ---- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_columns_handles_simple_case() {
        let map = BTreeMap::from([(1_i64, "id".to_string()), (2_i64, "name".to_string())]);
        let attnums = vec![2, 1];
        let cols = resolve_column_names(&attnums, &map).unwrap();
        assert_eq!(
            cols.iter().map(Identifier::as_str).collect::<Vec<_>>(),
            vec!["name", "id"]
        );
    }

    #[test]
    fn resolve_columns_fails_on_missing_attnum() {
        let map = BTreeMap::from([(1_i64, "id".to_string())]);
        let err = resolve_column_names(&[3], &map).unwrap_err();
        assert!(
            format!("{err}").contains("attnum 3"),
            "error should mention attnum 3: {err}"
        );
    }

    #[test]
    fn resolve_columns_empty_input_returns_empty() {
        let map = BTreeMap::from([(1_i64, "id".to_string())]);
        let cols = resolve_column_names(&[], &map).unwrap();
        assert!(cols.is_empty());
    }

    #[test]
    fn build_scope_all_tables_ignores_rels_and_namespaces() {
        let scope = build_scope(true, vec![], |_| Ok(None), vec![]).unwrap();
        assert!(matches!(scope, PublicationScope::AllTables));
    }
}
