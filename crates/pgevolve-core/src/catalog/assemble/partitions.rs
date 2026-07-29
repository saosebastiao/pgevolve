//! Partition metadata assembly from catalog rows.
//!
//! Called from [`super::assemble`] to populate [`crate::ir::partition::PartitionBy`]
//! and [`crate::ir::partition::PartitionOf`] on [`crate::ir::table::Table`] entries
//! that were already loaded by the main table query.

use std::path::PathBuf;

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::ir::catalog::Catalog;
use crate::parse::error::SourceLocation;
use crate::parse::from_catalog;

use super::qname_from_strings;

/// Re-parse `pg_get_partkeydef` and `pg_get_expr(relpartbound)` output and
/// merge the resulting [`crate::ir::partition::PartitionBy`] / [`crate::ir::partition::PartitionOf`] onto the matching
/// [`crate::ir::table::Table`] entries that were already loaded by the main table query.
pub(super) fn merge_partition_metadata(
    catalog: &mut Catalog,
    partitioned_rows: &[Row],
    partition_rows: &[Row],
) -> Result<(), CatalogError> {
    let loc = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
    apply_partitioned_parents(catalog, partitioned_rows, &loc)?;
    apply_partition_children(catalog, partition_rows, &loc)?;
    Ok(())
}

/// Apply `PARTITION BY` metadata to partitioned-table parents.
fn apply_partitioned_parents(
    catalog: &mut Catalog,
    rows: &[Row],
    loc: &SourceLocation,
) -> Result<(), CatalogError> {
    for r in rows {
        let schema_name = r.get_text(CatalogQuery::PartitionedTables, "schema_name")?;
        let table_name = r.get_text(CatalogQuery::PartitionedTables, "table_name")?;
        let partkey_def = r.get_text(CatalogQuery::PartitionedTables, "partkey_def")?;

        let qname = qname_from_strings(&schema_name, &table_name)?;
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == qname)
            .ok_or_else(|| CatalogError::DanglingReference {
                kind: "partitioned-table parent",
                what: qname.to_string(),
            })?;

        table.partition_by =
            Some(from_catalog::partition_by(&partkey_def, loc).map_err(CatalogError::from)?);
    }
    Ok(())
}

/// Apply `PARTITION OF` / bound metadata to child-partition tables.
fn apply_partition_children(
    catalog: &mut Catalog,
    rows: &[Row],
    loc: &SourceLocation,
) -> Result<(), CatalogError> {
    use crate::ir::partition::PartitionOf;

    for r in rows {
        let schema_name = r.get_text(CatalogQuery::Partitions, "schema_name")?;
        let table_name = r.get_text(CatalogQuery::Partitions, "table_name")?;
        let parent_schema = r.get_text(CatalogQuery::Partitions, "parent_schema")?;
        let parent_name = r.get_text(CatalogQuery::Partitions, "parent_name")?;
        let partbound_def = r.get_text(CatalogQuery::Partitions, "partbound_def")?;

        let qname = qname_from_strings(&schema_name, &table_name)?;
        let parent = qname_from_strings(&parent_schema, &parent_name)?;
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == qname)
            .ok_or_else(|| CatalogError::DanglingReference {
                kind: "child partition",
                what: qname.to_string(),
            })?;

        let bounds =
            from_catalog::partition_bounds(&partbound_def, loc).map_err(CatalogError::from)?;
        table.partition_of = Some(PartitionOf { parent, bounds });
        // Clear inherited columns: a partition child's canonical source form
        // uses `PARTITION OF parent FOR VALUES …` with no column list.
        // Keeping the inherited columns would cause spurious diff against a
        // source that omits them, so we drop them here to match the source IR.
        table.columns.clear();
        table.constraints.clear();
    }
    Ok(())
}
