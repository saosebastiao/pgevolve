//! Partition metadata assembly from catalog rows.
//!
//! Called from [`super::assemble`] to populate [`crate::ir::partition::PartitionBy`]
//! and [`crate::ir::partition::PartitionOf`] on [`crate::ir::table::Table`] entries
//! that were already loaded by the main table query.

use std::path::PathBuf;

use pg_query::NodeEnum;

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::ir::catalog::Catalog;
use crate::parse::error::SourceLocation;

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
    use crate::parse::builder::create_stmt::build_partition_by;

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

        // Wrap the raw partkey_def back into a synthetic CREATE TABLE so
        // pg_query can parse it, then extract the PartitionSpec node.
        let synthetic = format!(
            "CREATE TABLE _pgevolve_synth () PARTITION BY {};",
            partkey_def.trim()
        );
        let parsed = pg_query::parse(&synthetic).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "could not re-parse partkey_def {partkey_def:?}: {e}"
            )))
        })?;
        let Some(raw_stmt) = parsed.protobuf.stmts.into_iter().next() else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "synthetic partkey CREATE TABLE yielded no statement".into(),
            )));
        };
        let Some(node) = raw_stmt.stmt.and_then(|n| n.node) else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "synthetic partkey CREATE TABLE node was empty".into(),
            )));
        };
        let NodeEnum::CreateStmt(create_stmt) = node else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "expected CreateStmt for partkey re-parse".into(),
            )));
        };
        let spec = create_stmt.partspec.as_ref().ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "synthetic CREATE TABLE lost partspec".into(),
            ))
        })?;

        table.partition_by = Some(
            build_partition_by(spec, loc).map_err(|e| CatalogError::ReparseFailed(Box::new(e)))?,
        );
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
    use crate::parse::builder::create_stmt::build_partition_bounds;

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

        // Wrap the raw partbound_def in a synthetic ATTACH PARTITION so
        // pg_query can parse it, then extract the PartitionBoundSpec node.
        let synthetic = format!(
            "ALTER TABLE _pgevolve_synth ATTACH PARTITION _pgevolve_synth_child {};",
            partbound_def.trim()
        );
        let parsed = pg_query::parse(&synthetic).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "could not re-parse partbound_def {partbound_def:?}: {e}"
            )))
        })?;
        let bound_spec = extract_partition_bound_spec(parsed)?;
        let bounds = build_partition_bounds(&bound_spec, loc)
            .map_err(|e| CatalogError::ReparseFailed(Box::new(e)))?;
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

/// Walk the protobuf AST of a synthetic `ALTER TABLE … ATTACH PARTITION … <bound>`
/// statement and return the [`pg_query::protobuf::PartitionBoundSpec`] node.
fn extract_partition_bound_spec(
    parsed: pg_query::ParseResult,
) -> Result<pg_query::protobuf::PartitionBoundSpec, CatalogError> {
    let ir_err =
        |msg: &str| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(msg.to_string()));

    let Some(raw_stmt) = parsed.protobuf.stmts.into_iter().next() else {
        return Err(ir_err(
            "synthetic partbound ALTER TABLE yielded no statement",
        ));
    };
    let Some(node) = raw_stmt.stmt.and_then(|n| n.node) else {
        return Err(ir_err("synthetic partbound ALTER TABLE node was empty"));
    };
    let NodeEnum::AlterTableStmt(alter_stmt) = node else {
        return Err(ir_err("expected AlterTableStmt for partbound re-parse"));
    };
    let cmd_node = alter_stmt
        .cmds
        .into_iter()
        .next()
        .and_then(|n| n.node)
        .ok_or_else(|| ir_err("AlterTableStmt had no commands for partbound re-parse"))?;
    let NodeEnum::AlterTableCmd(cmd) = cmd_node else {
        return Err(ir_err("expected AlterTableCmd in partbound re-parse"));
    };
    let part_cmd_node = cmd
        .def
        .and_then(|n| n.node)
        .ok_or_else(|| ir_err("AlterTableCmd had no def node for partbound re-parse"))?;
    let NodeEnum::PartitionCmd(part_cmd) = part_cmd_node else {
        return Err(ir_err("expected PartitionCmd in partbound re-parse"));
    };
    part_cmd
        .bound
        .ok_or_else(|| ir_err("PartitionCmd missing bound spec"))
}
