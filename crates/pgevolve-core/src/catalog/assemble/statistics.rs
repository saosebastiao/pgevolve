//! Orchestrate `pg_statistic_ext` rows into `Vec<Statistic>`.
//!
//! Pipeline:
//!   1. Bulk-decode `STATISTICS_QUERY` rows → `Vec<PartialStatistic>`.
//!   2. Bulk-decode `STATISTIC_ATTRIBUTES_QUERY` rows → attnum→name maps per
//!      target table OID.
//!   3. Bulk-decode `STATISTIC_EXPRESSIONS_QUERY` rows → expression text lists
//!      per statistic OID, canonicalized via `reparse_expression_text`.
//!   4. Build `Vec<Statistic>`: column entries from `keys → Identifier` via the
//!      attribute map, expression entries appended after in stxexprs order.

// `CatalogError` embeds `IrError` and `ParseError`, both of which are large.
// Cold-path catalog reads; boxing adds noise without benefit.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use crate::catalog::CatalogQuery;
use crate::catalog::assemble::reparse_expression_text;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::catalog::statistics::{
    decode_statistic_attribute_row, decode_statistic_expression_row, decode_statistic_row,
};
use crate::identifier::Identifier;
use crate::ir::statistic::{Statistic, StatisticColumn};

/// Assemble all statistics rows into `Vec<Statistic>`.
///
/// `stat_rows`        — rows from `STATISTICS_QUERY`.
/// `attr_rows`        — rows from `STATISTIC_ATTRIBUTES_QUERY`.
/// `expr_rows`        — rows from `STATISTIC_EXPRESSIONS_QUERY` (bulk; empty
///                      slice when no statistics have expressions).
pub(in crate::catalog) fn assemble_statistics(
    stat_rows: &[Row],
    attr_rows: &[Row],
    expr_rows: &[Row],
) -> Result<Vec<Statistic>, CatalogError> {
    // Build attname_by_attnum per target_oid from the attribute rows.
    let mut attnames_by_rel: BTreeMap<i64, BTreeMap<i64, String>> = BTreeMap::new();
    for row in attr_rows {
        let attr = decode_statistic_attribute_row(row)?;
        attnames_by_rel
            .entry(attr.target_oid)
            .or_default()
            .insert(attr.attnum, attr.attname);
    }

    // Build expression lists per stat_oid from the bulk expression rows.
    // Each entry is sorted by expr_index (the SQL guarantees this, but we sort
    // defensively in case the test data is unordered).
    //
    // Stored as `(expr_index, expr_sql)` to allow post-sort by index.
    let mut exprs_by_oid: BTreeMap<i64, Vec<(i64, String)>> = BTreeMap::new();
    for row in expr_rows {
        let e = decode_statistic_expression_row(row)?;
        exprs_by_oid
            .entry(e.stat_oid)
            .or_default()
            .push((e.expr_index, e.expr_sql));
    }
    // Sort each list by expr_index for deterministic column ordering.
    for entries in exprs_by_oid.values_mut() {
        entries.sort_by_key(|(idx, _)| *idx);
    }

    let mut statistics = Vec::with_capacity(stat_rows.len());
    for row in stat_rows {
        let p = decode_statistic_row(row)?;

        // Resolve key attnums → column identifiers.
        let attmap =
            attnames_by_rel
                .get(&p.target_oid)
                .ok_or_else(|| CatalogError::DanglingReference {
                    kind: "statistic target table attributes",
                    what: format!(
                        "no attribute map for target_oid {} (statistic {})",
                        p.target_oid, p.qname,
                    ),
                })?;

        let mut columns: Vec<StatisticColumn> = Vec::with_capacity(p.keys.len());
        for attnum in &p.keys {
            let attname = attmap
                .get(attnum)
                .ok_or_else(|| CatalogError::DanglingReference {
                    kind: "statistic column attnum",
                    what: format!(
                        "attnum {} not in pg_attribute for target {} (statistic {})",
                        attnum, p.target, p.qname,
                    ),
                })?;
            let col_ident =
                Identifier::from_unquoted(attname).map_err(|e| CatalogError::BadColumnType {
                    query: CatalogQuery::StatisticAttributes,
                    column: "attname".to_string(),
                    message: format!("invalid column name {attname:?}: {e}"),
                })?;
            columns.push(StatisticColumn::Column(col_ident));
        }

        // Append expression entries in stxexprs order.
        if p.has_expressions
            && let Some(exprs) = exprs_by_oid.get(&p.oid)
        {
            for (_idx, sql) in exprs {
                let normalized = reparse_expression_text(sql)?;
                columns.push(StatisticColumn::Expression(normalized));
            }
        }

        // Derive Option<Identifier> for owner (empty string → None).
        let owner = if p.owner_str.is_empty() {
            None
        } else {
            Some(Identifier::from_unquoted(&p.owner_str).map_err(|e| {
                CatalogError::BadColumnType {
                    query: CatalogQuery::Statistics,
                    column: "owner".to_string(),
                    message: format!("invalid owner name {:?}: {e}", p.owner_str),
                }
            })?)
        };

        // -1 means "use PG default" (not managed).
        #[allow(clippy::cast_possible_truncation)]
        let statistics_target = if p.stat_target == -1 {
            None
        } else {
            Some(p.stat_target as i32)
        };

        let comment = if p.comment_str.is_empty() {
            None
        } else {
            Some(p.comment_str)
        };

        statistics.push(Statistic {
            qname: p.qname,
            target: p.target,
            kinds: p.kinds,
            columns,
            statistics_target,
            owner,
            comment,
        });
    }

    Ok(statistics)
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;
    use crate::ir::statistic::StatisticKinds;

    fn stat_row(oid: i64, schema: &str, name: &str, target_oid: i64, kinds: &[&str]) -> Row {
        Row::new()
            .with("oid", Value::Integer(oid))
            .with("schema", Value::Text(schema.to_string()))
            .with("name", Value::Text(name.to_string()))
            .with("target_schema", Value::Text("app".to_string()))
            .with("target_name", Value::Text("t".to_string()))
            .with(
                "kinds",
                Value::TextArray(kinds.iter().map(|s| (*s).to_string()).collect()),
            )
            .with("keys", Value::IntegerArray(vec![1, 2]))
            .with("target_oid", Value::Integer(target_oid))
            .with("stat_target", Value::Integer(-1))
            .with("owner", Value::Text("pg_user".to_string()))
            .with("comment", Value::Text(String::new()))
            .with("has_expressions", Value::Bool(false))
    }

    fn attr_row(target_oid: i64, attnum: i64, attname: &str) -> Row {
        Row::new()
            .with("target_oid", Value::Integer(target_oid))
            .with("attnum", Value::Integer(attnum))
            .with("attname", Value::Text(attname.to_string()))
    }

    fn expr_row(stat_oid: i64, expr_index: i64, expr_sql: &str) -> Row {
        Row::new()
            .with("stat_oid", Value::Integer(stat_oid))
            .with("expr_index", Value::Integer(expr_index))
            .with("expr_sql", Value::Text(expr_sql.to_string()))
    }

    #[test]
    fn empty_rows_returns_empty_vec() {
        let result = assemble_statistics(&[], &[], &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn single_statistic_two_columns() {
        let stat_rows = vec![stat_row(1, "app", "my_stat", 99, &["d", "f"])];
        let attr_rows = vec![attr_row(99, 1, "col_a"), attr_row(99, 2, "col_b")];

        let result = assemble_statistics(&stat_rows, &attr_rows, &[]).unwrap();
        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert_eq!(s.qname.name.as_str(), "my_stat");
        assert_eq!(
            s.kinds,
            StatisticKinds {
                ndistinct: true,
                dependencies: true,
                mcv: false
            }
        );
        assert_eq!(s.columns.len(), 2);
        assert!(matches!(&s.columns[0], StatisticColumn::Column(id) if id.as_str() == "col_a"));
        assert!(matches!(&s.columns[1], StatisticColumn::Column(id) if id.as_str() == "col_b"));
        assert!(s.statistics_target.is_none());
        assert!(s.comment.is_none());
    }

    #[test]
    fn stat_target_positive_becomes_some() {
        let mut row = stat_row(1, "app", "s", 99, &["d"]);
        row.insert("stat_target", Value::Integer(150));
        let attr_rows = vec![attr_row(99, 1, "a"), attr_row(99, 2, "b")];
        let result = assemble_statistics(&[row], &attr_rows, &[]).unwrap();
        assert_eq!(result[0].statistics_target, Some(150));
    }

    #[test]
    fn owner_empty_becomes_none() {
        let mut row = stat_row(1, "app", "s", 99, &["d"]);
        row.insert("owner", Value::Text(String::new()));
        let attr_rows = vec![attr_row(99, 1, "a"), attr_row(99, 2, "b")];
        let result = assemble_statistics(&[row], &attr_rows, &[]).unwrap();
        assert!(result[0].owner.is_none());
    }

    #[test]
    fn comment_non_empty_becomes_some() {
        let mut row = stat_row(1, "app", "s", 99, &["d"]);
        row.insert("comment", Value::Text("my comment".to_string()));
        let attr_rows = vec![attr_row(99, 1, "a"), attr_row(99, 2, "b")];
        let result = assemble_statistics(&[row], &attr_rows, &[]).unwrap();
        assert_eq!(result[0].comment.as_deref(), Some("my comment"));
    }

    #[test]
    fn dangling_target_oid_errors() {
        let stat_rows = vec![stat_row(1, "app", "s", 999, &["d"])];
        // No attr rows for oid 999.
        let err = assemble_statistics(&stat_rows, &[], &[]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("999") || msg.contains("attribute map"),
            "error should mention the missing oid: {msg}"
        );
    }

    #[test]
    fn expression_column_decoded_when_has_expressions_true() {
        let mut row = stat_row(1, "app", "s", 99, &["d"]);
        row.insert("has_expressions", Value::Bool(true));
        row.insert("keys", Value::IntegerArray(vec![]));
        let attr_rows = vec![attr_row(99, 1, "a"), attr_row(99, 2, "b")];
        let expr_rows = vec![expr_row(1, 0, "lower(name)")];

        let result = assemble_statistics(&[row], &attr_rows, &expr_rows).unwrap();
        assert_eq!(result[0].columns.len(), 1);
        assert!(
            matches!(&result[0].columns[0], StatisticColumn::Expression(_)),
            "expected an Expression column"
        );
    }
}
