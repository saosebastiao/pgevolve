//! Decode `pg_statistic_ext` rows into `PartialStatistic` intermediate values.
//!
//! The assembler (`catalog/assemble/statistics.rs`) drives the full pipeline:
//! bulk-fetch rows, resolve column attnums, decode expression entries, and
//! construct `Vec<Statistic>` IR.
//!
//! Three queries are involved:
//!   - `STATISTICS_QUERY`   — per-statistic base row (oid, name, kinds, keys…)
//!   - `STATISTIC_ATTRIBUTES_QUERY` — attnum → name for all target tables
//!   - `STATISTIC_EXPRESSIONS_QUERY` — one row per expression entry (per oid)

// `CatalogError` embeds `IrError` and `ParseError`, both of which are large.
// Cold-path catalog reads; boxing adds noise without benefit.
#![allow(clippy::result_large_err)]

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::statistic::StatisticKinds;

// ---- query constants ----------------------------------------------------------

const Q: CatalogQuery = CatalogQuery::Statistics;
const Q_ATTR: CatalogQuery = CatalogQuery::StatisticAttributes;
const Q_EXPR: CatalogQuery = CatalogQuery::StatisticExpressions;

// ---- PartialStatistic --------------------------------------------------------

/// Decoded `pg_statistic_ext` row; column names and expression SQL not yet
/// resolved — the assembler completes those steps.
pub struct PartialStatistic {
    /// `pg_statistic_ext.oid`.
    pub oid: i64,
    /// Schema-qualified statistic name.
    pub qname: QualifiedName,
    /// Schema-qualified target table.
    pub target: QualifiedName,
    /// OID of the target table (used to look up `pg_attribute`).
    pub target_oid: i64,
    /// Decoded `stxkind`: `'d'` → ndistinct, `'f'` → dependencies, `'m'` → mcv.
    /// `'e'` is the internal expression marker — ignored here.
    pub kinds: StatisticKinds,
    /// `stxkeys` cast to `int8[]`: ordered list of attribute numbers.
    pub keys: Vec<i64>,
    /// True when `stxexprs IS NOT NULL` — one or more expression-form entries.
    pub has_expressions: bool,
    /// `stxstattarget` COALESCED to -1 when NULL. The assembler converts -1 to
    /// `None` and any positive value to `Some(n)`.
    pub stat_target: i64,
    /// Role name; empty string when the connection returned none.
    pub owner_str: String,
    /// Description; empty string when no comment exists.
    pub comment_str: String,
}

/// Decode one `STATISTICS_QUERY` row into a [`PartialStatistic`].
pub fn decode_statistic_row(row: &Row) -> Result<PartialStatistic, CatalogError> {
    let schema_str = row.get_text(Q, "schema")?;
    let name_str = row.get_text(Q, "name")?;
    let target_schema_str = row.get_text(Q, "target_schema")?;
    let target_name_str = row.get_text(Q, "target_name")?;

    let schema =
        Identifier::from_unquoted(&schema_str).map_err(|e| CatalogError::BadColumnType {
            query: Q,
            column: "schema".to_string(),
            message: format!("invalid schema {schema_str:?}: {e}"),
        })?;
    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: Q,
        column: "name".to_string(),
        message: format!("invalid statistic name {name_str:?}: {e}"),
    })?;
    let target_schema =
        Identifier::from_unquoted(&target_schema_str).map_err(|e| CatalogError::BadColumnType {
            query: Q,
            column: "target_schema".to_string(),
            message: format!("invalid target schema {target_schema_str:?}: {e}"),
        })?;
    let target_name =
        Identifier::from_unquoted(&target_name_str).map_err(|e| CatalogError::BadColumnType {
            query: Q,
            column: "target_name".to_string(),
            message: format!("invalid target name {target_name_str:?}: {e}"),
        })?;

    let kinds_raw = row.get_text_array(Q, "kinds")?;
    let kinds = StatisticKinds {
        // 'd' = ndistinct
        ndistinct: kinds_raw.iter().any(|k| k == "d"),
        // 'f' = functional dependencies
        dependencies: kinds_raw.iter().any(|k| k == "f"),
        // 'm' = most-common-value lists
        mcv: kinds_raw.iter().any(|k| k == "m"),
        // 'e' is an internal marker for "has expression entries"; derived from
        // the separate `has_expressions` column instead.
    };

    Ok(PartialStatistic {
        oid: row.get_int(Q, "oid")?,
        qname: QualifiedName::new(schema, name),
        target: QualifiedName::new(target_schema, target_name),
        target_oid: row.get_int(Q, "target_oid")?,
        kinds,
        keys: row.get_int_array(Q, "keys")?,
        has_expressions: row.get_bool(Q, "has_expressions")?,
        stat_target: row.get_int(Q, "stat_target")?,
        owner_str: row.get_text(Q, "owner")?,
        comment_str: row.get_text(Q, "comment")?,
    })
}

// ---- StatisticAttribute ------------------------------------------------------

/// One row from `STATISTIC_ATTRIBUTES_QUERY`.
pub struct StatisticAttribute {
    pub target_oid: i64,
    pub attnum: i64,
    pub attname: String,
}

/// Decode one `STATISTIC_ATTRIBUTES_QUERY` row.
pub fn decode_statistic_attribute_row(row: &Row) -> Result<StatisticAttribute, CatalogError> {
    Ok(StatisticAttribute {
        target_oid: row.get_int(Q_ATTR, "target_oid")?,
        attnum: row.get_int(Q_ATTR, "attnum")?,
        attname: row.get_text(Q_ATTR, "attname")?,
    })
}

// ---- StatisticExpression -----------------------------------------------------

/// One row from `STATISTIC_EXPRESSIONS_QUERY` (bulk query).
pub struct StatisticExpressionRow {
    /// OID of the owning `pg_statistic_ext` row.
    pub stat_oid: i64,
    /// Zero-based ordinal within this statistic's expression list.
    pub expr_index: i64,
    /// SQL text returned by `pg_get_statisticsobjdef_expressions`.
    pub expr_sql: String,
}

/// Decode one `STATISTIC_EXPRESSIONS_QUERY` row.
pub fn decode_statistic_expression_row(row: &Row) -> Result<StatisticExpressionRow, CatalogError> {
    Ok(StatisticExpressionRow {
        stat_oid: row.get_int(Q_EXPR, "stat_oid")?,
        expr_index: row.get_int(Q_EXPR, "expr_index")?,
        expr_sql: row.get_text(Q_EXPR, "expr_sql")?,
    })
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    fn make_stat_row(kinds: Vec<&str>, has_expr: bool, stat_target: i64) -> Row {
        Row::new()
            .with("oid", Value::Integer(1))
            .with("schema", Value::Text("app".to_string()))
            .with("name", Value::Text("my_stat".to_string()))
            .with("target_schema", Value::Text("app".to_string()))
            .with("target_name", Value::Text("t".to_string()))
            .with(
                "kinds",
                Value::TextArray(kinds.into_iter().map(ToString::to_string).collect()),
            )
            .with("keys", Value::IntegerArray(vec![1, 2]))
            .with("target_oid", Value::Integer(99))
            .with("stat_target", Value::Integer(stat_target))
            .with("owner", Value::Text("app_owner".to_string()))
            .with("comment", Value::Text(String::new()))
            .with("has_expressions", Value::Bool(has_expr))
    }

    #[test]
    fn kinds_decode_ndistinct_only() {
        let row = make_stat_row(vec!["d"], false, -1);
        let p = decode_statistic_row(&row).unwrap();
        assert!(p.kinds.ndistinct);
        assert!(!p.kinds.dependencies);
        assert!(!p.kinds.mcv);
    }

    #[test]
    fn kinds_decode_all_three() {
        let row = make_stat_row(vec!["d", "f", "m"], false, -1);
        let p = decode_statistic_row(&row).unwrap();
        assert!(p.kinds.ndistinct && p.kinds.dependencies && p.kinds.mcv);
    }

    #[test]
    fn kinds_decode_expression_marker_ignored() {
        // 'e' is the internal expression marker; it should not set any kind flag.
        let row = make_stat_row(vec!["d", "e"], true, -1);
        let p = decode_statistic_row(&row).unwrap();
        assert!(p.kinds.ndistinct);
        assert!(!p.kinds.dependencies);
        assert!(!p.kinds.mcv);
        // Expression flag comes from has_expressions column, not kinds.
        assert!(p.has_expressions);
    }

    #[test]
    fn kinds_decode_unknown_kind_ignored() {
        // Unknown characters should be silently ignored (just not match any flag).
        let row = make_stat_row(vec!["z", "d"], false, -1);
        let p = decode_statistic_row(&row).unwrap();
        assert!(p.kinds.ndistinct);
        assert!(!p.kinds.dependencies);
        assert!(!p.kinds.mcv);
    }

    #[test]
    fn stat_target_minus_one_preserved() {
        let row = make_stat_row(vec!["d"], false, -1);
        let p = decode_statistic_row(&row).unwrap();
        // Assembler converts -1 → None; decoder just returns the raw i64.
        assert_eq!(p.stat_target, -1);
    }

    #[test]
    fn stat_target_positive_preserved() {
        let row = make_stat_row(vec!["d"], false, 200);
        let p = decode_statistic_row(&row).unwrap();
        assert_eq!(p.stat_target, 200);
    }

    #[test]
    fn happy_path_basic_row() {
        let row = make_stat_row(vec!["d", "f"], false, -1);
        let p = decode_statistic_row(&row).unwrap();
        assert_eq!(p.qname.schema.as_str(), "app");
        assert_eq!(p.qname.name.as_str(), "my_stat");
        assert_eq!(p.target.name.as_str(), "t");
        assert_eq!(p.keys, vec![1, 2]);
        assert!(!p.has_expressions);
        assert_eq!(p.owner_str, "app_owner");
        assert!(p.comment_str.is_empty());
    }

    #[test]
    fn attribute_row_decodes() {
        let row = Row::new()
            .with("target_oid", Value::Integer(99))
            .with("attnum", Value::Integer(3))
            .with("attname", Value::Text("col_c".to_string()));
        let attr = decode_statistic_attribute_row(&row).unwrap();
        assert_eq!(attr.target_oid, 99);
        assert_eq!(attr.attnum, 3);
        assert_eq!(attr.attname, "col_c");
    }

    #[test]
    fn expression_row_decodes() {
        let row = Row::new()
            .with("stat_oid", Value::Integer(42))
            .with("expr_index", Value::Integer(0))
            .with("expr_sql", Value::Text("lower(name)".to_string()));
        let expr = decode_statistic_expression_row(&row).unwrap();
        assert_eq!(expr.stat_oid, 42);
        assert_eq!(expr.expr_index, 0);
        assert_eq!(expr.expr_sql, "lower(name)");
    }
}
