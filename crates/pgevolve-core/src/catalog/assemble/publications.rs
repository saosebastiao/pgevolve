//! Assemble `pg_publication`, `pg_publication_rel`, `pg_publication_namespace`,
//! and column-attnum rows into `Vec<Publication>`.

// `CatalogError` embeds `IrError` and `ParseError`, both of which carry
// location info and are large. Cold-path catalog reads; boxing adds noise
// without benefit.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use crate::catalog::error::CatalogError;
use crate::catalog::publications::{
    PartialPublication, PartialPublicationNamespace, PartialPublicationRel, build_scope,
    decode_publication_attribute_row, decode_publication_namespace_row, decode_publication_rel_row,
    decode_publication_row,
};
use crate::catalog::rows::Row;
use crate::identifier::Identifier;
use crate::ir::publication::Publication;

/// Assemble all publication rows into `Vec<Publication>`.
///
/// Accepts the pre-fetched rows from the four publication queries.
/// Called by the top-level `assemble()` orchestrator.
pub(super) fn assemble_publications(
    pub_rows: &[Row],
    rel_rows: &[Row],
    ns_rows: &[Row],
    attr_rows: &[Row],
) -> Result<Vec<Publication>, CatalogError> {
    // Build attname_by_attnum per rel_oid from the attribute rows.
    let mut attnames_by_rel: BTreeMap<i64, BTreeMap<i64, String>> = BTreeMap::new();
    for row in attr_rows {
        let attr = decode_publication_attribute_row(row)?;
        attnames_by_rel
            .entry(attr.rel_oid)
            .or_default()
            .insert(attr.attnum, attr.attname);
    }

    // Decode and group rel rows by pub_oid.
    let mut rels_by_oid: BTreeMap<i64, Vec<PartialPublicationRel>> = BTreeMap::new();
    for row in rel_rows {
        let pr = decode_publication_rel_row(row)?;
        rels_by_oid.entry(pr.pub_oid).or_default().push(pr);
    }

    // Decode and group namespace rows by pub_oid.
    let mut ns_by_oid: BTreeMap<i64, Vec<PartialPublicationNamespace>> = BTreeMap::new();
    for row in ns_rows {
        let pn = decode_publication_namespace_row(row)?;
        ns_by_oid.entry(pn.pub_oid).or_default().push(pn);
    }

    // Build each Publication.
    let mut publications = Vec::with_capacity(pub_rows.len());
    for row in pub_rows {
        let pp: PartialPublication = decode_publication_row(row)?;
        let rels = rels_by_oid.remove(&pp.oid).unwrap_or_default();
        let nss = ns_by_oid.remove(&pp.oid).unwrap_or_default();

        let scope = build_scope(
            pp.all_tables,
            rels,
            |r| -> Result<Option<Vec<Identifier>>, CatalogError> {
                let Some(attnums) = &r.col_attnums else {
                    return Ok(None);
                };
                let empty_map = BTreeMap::new();
                let attname_map = attnames_by_rel.get(&r.rel_oid).unwrap_or(&empty_map);
                let names =
                    crate::catalog::publications::resolve_column_names(attnums, attname_map)?;
                Ok(Some(names))
            },
            nss,
        )?;

        publications.push(Publication {
            name: pp.name,
            scope,
            publish: pp.publish,
            publish_via_partition_root: pp.publish_via_partition_root,
            owner: pp.owner,
            comment: pp.comment,
        });
    }

    Ok(publications)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;
    use crate::ir::publication::PublicationScope;

    fn pub_row(oid: i64, name: &str, all_tables: bool) -> Row {
        Row::new()
            .with("oid", Value::Integer(oid))
            .with("name", Value::Text(name.to_string()))
            .with("owner", Value::Text("pg_user".to_string()))
            .with("all_tables", Value::Bool(all_tables))
            .with("pub_insert", Value::Bool(true))
            .with("pub_update", Value::Bool(true))
            .with("pub_delete", Value::Bool(true))
            .with("pub_truncate", Value::Bool(true))
            .with("publish_via_partition_root", Value::Bool(false))
            .with("comment", Value::Text(String::new()))
    }

    fn rel_row(pub_oid: i64, schema: &str, table: &str, rel_oid: i64) -> Row {
        Row::new()
            .with("pub_oid", Value::Integer(pub_oid))
            .with("schema", Value::Text(schema.to_string()))
            .with("table_name", Value::Text(table.to_string()))
            .with("row_filter", Value::Null)
            .with("col_attnums", Value::Null)
            .with("rel_oid", Value::Integer(rel_oid))
    }

    #[test]
    fn all_tables_publication_assembles_correctly() {
        let pubs = assemble_publications(&[pub_row(1, "my_pub", true)], &[], &[], &[]).unwrap();
        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].name.as_str(), "my_pub");
        assert!(matches!(pubs[0].scope, PublicationScope::AllTables));
        assert!(pubs[0].publish.insert);
    }

    #[test]
    fn selective_publication_with_table() {
        let pub_rows = vec![pub_row(42, "p", false)];
        let rel_rows = vec![rel_row(42, "app", "orders", 99)];
        let pubs = assemble_publications(&pub_rows, &rel_rows, &[], &[]).unwrap();
        assert_eq!(pubs.len(), 1);
        let PublicationScope::Selective { tables, schemas } = &pubs[0].scope else {
            panic!("expected Selective");
        };
        assert!(schemas.is_empty());
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].qname.name.as_str(), "orders");
        assert!(tables[0].columns.is_none());
        assert!(tables[0].row_filter.is_none());
    }

    #[test]
    fn selective_publication_with_column_list() {
        let pub_rows = vec![pub_row(10, "p", false)];
        let rel_rows = vec![{
            let mut r = rel_row(10, "app", "orders", 55);
            r.insert("col_attnums", Value::IntegerArray(vec![1, 2]));
            r
        }];
        let attr_rows = vec![
            Row::new()
                .with("rel_oid", Value::Integer(55))
                .with("attnum", Value::Integer(1))
                .with("attname", Value::Text("id".to_string())),
            Row::new()
                .with("rel_oid", Value::Integer(55))
                .with("attnum", Value::Integer(2))
                .with("attname", Value::Text("status".to_string())),
        ];
        let pubs = assemble_publications(&pub_rows, &rel_rows, &[], &attr_rows).unwrap();
        let PublicationScope::Selective { tables, .. } = &pubs[0].scope else {
            panic!("expected Selective");
        };
        let cols = tables[0].columns.as_ref().unwrap();
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].as_str(), "id");
        assert_eq!(cols[1].as_str(), "status");
    }

    #[test]
    fn empty_pub_rows_returns_empty_vec() {
        let pubs = assemble_publications(&[], &[], &[], &[]).unwrap();
        assert!(pubs.is_empty());
    }
}
