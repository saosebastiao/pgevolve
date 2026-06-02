//! Statistic generators (v0.3.7). Per-table strategies draw columns from the
//! table's actual column list so generated statistics always reference real
//! columns.

#![allow(clippy::needless_pass_by_value)]

use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::statistic::{Statistic, StatisticColumn, StatisticKinds};
use pgevolve_core::ir::table::Table;

/// Generate a [`StatisticKinds`] with at least one kind enabled.
fn arb_statistic_kinds() -> impl Strategy<Value = StatisticKinds> {
    (any::<bool>(), any::<bool>(), any::<bool>())
        .prop_filter("at least one kind", |(d, f, m)| *d || *f || *m)
        .prop_map(|(ndistinct, dependencies, mcv)| StatisticKinds {
            ndistinct,
            dependencies,
            mcv,
        })
}

/// Small fixed pool of statistic name suffixes.
const STAT_NAMES: &[&str] = &["s_a", "s_b", "s_c"];

/// Generate a [`Statistic`] targeting `target`.
///
/// Draws 2–N columns from `col_pool`. The statistic's schema is the same
/// as the target table's schema.
pub fn arb_statistic(
    stat_idx: usize,
    target: QualifiedName,
    col_pool: Vec<Identifier>,
) -> impl Strategy<Value = Statistic> {
    let schema = target.schema.clone();
    let target_c = target;
    // Need at least 2 columns (PG requires >= 2 for CREATE STATISTICS).
    let max_cols = col_pool.len().max(2);
    let col_count = 2usize..=max_cols;
    (arb_statistic_kinds(), col_count, any::<u64>()).prop_map(move |(kinds, n_cols, seed)| {
        // Deterministically pick `n_cols` from the pool using the seed.
        let chosen: Vec<StatisticColumn> = col_pool
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                // XOR-shift to spread bits evenly.
                let bit = (seed >> (*i % 64)) & 1;
                bit == 1
            })
            .take(n_cols)
            .map(|(_, id)| StatisticColumn::Column(id.clone()))
            .collect();
        // If the filter yielded fewer than 2, fall back to first n_cols.
        let columns: Vec<StatisticColumn> = if chosen.len() >= 2 {
            chosen
        } else {
            col_pool
                .iter()
                .take(n_cols)
                .map(|id| StatisticColumn::Column(id.clone()))
                .collect()
        };
        let stat_name = format!(
            "{}_{}",
            target_c.name.as_str(),
            STAT_NAMES[stat_idx % STAT_NAMES.len()]
        );
        Statistic {
            qname: QualifiedName::new(
                schema.clone(),
                Identifier::from_unquoted(&stat_name).unwrap(),
            ),
            target: target_c.clone(),
            kinds,
            columns,
            statistics_target: None,
            owner: None,
            comment: None,
        }
    })
}

/// Generate 0–1 statistics per table, drawing columns from the table's actual
/// column list.  Returns a flat `Vec<Statistic>` for the whole catalog.
pub(super) fn arb_statistics_for_tables(tables: &[Table]) -> BoxedStrategy<Vec<Statistic>> {
    // Tables with fewer than 2 btree-eligible columns cannot have statistics.
    // PG rejects CREATE STATISTICS on columns whose type lacks a default btree
    // opclass (json, jsonb, arrays, bit/varbit, user-defined, ...) with 0A000.
    let eligible: Vec<(QualifiedName, Vec<Identifier>)> = tables
        .iter()
        .filter_map(|t| {
            let eligible_cols: Vec<Identifier> = t
                .columns
                .iter()
                .filter(|c| c.ty.has_default_btree_opclass())
                .map(|c| c.name.clone())
                .collect();
            if eligible_cols.len() >= 2 {
                Some((t.qname.clone(), eligible_cols))
            } else {
                None
            }
        })
        .collect();

    if eligible.is_empty() {
        return Just(Vec::new()).boxed();
    }

    // For each eligible table, independently decide 0 or 1 statistic.
    let strategies: Vec<BoxedStrategy<Option<Statistic>>> = eligible
        .into_iter()
        .enumerate()
        .map(|(idx, (qname, cols))| {
            prop_oneof![Just(None), arb_statistic(idx, qname, cols).prop_map(Some),].boxed()
        })
        .collect();

    strategies
        .prop_map(|opts| opts.into_iter().flatten().collect())
        .boxed()
}

#[cfg(test)]
mod tests {
    use pgevolve_core::ir::statistic::StatisticColumn;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, TestRunner};

    use super::*;
    use crate::ir_generator::IRGeneratorConfig;
    use crate::ir_generator::schema::arbitrary_schemas;
    use crate::ir_generator::table::arbitrary_tables_for_schema;

    /// Every column referenced in a generated `Statistic` must have a type
    /// that has a default btree opclass, or PG will reject the DDL with 0A000.
    #[test]
    fn statistics_only_reference_btree_eligible_columns() {
        let mut runner = TestRunner::new(Config {
            cases: 256,
            ..Config::default()
        });
        let cfg = IRGeneratorConfig::default();

        for _ in 0..256 {
            // Build a table set then generate statistics from it.
            let schema_tree = arbitrary_schemas(&cfg).new_tree(&mut runner).unwrap();
            let schemas = schema_tree.current();
            if schemas.is_empty() {
                continue;
            }
            let schema = &schemas[0];
            let table_tree = arbitrary_tables_for_schema(schema.name.clone(), &cfg)
                .new_tree(&mut runner)
                .unwrap();
            let tables = table_tree.current();

            // Build a name→type lookup for fast column-type resolution.
            let col_type_map: std::collections::HashMap<_, _> = tables
                .iter()
                .flat_map(|t| {
                    t.columns
                        .iter()
                        .map(|c| ((t.qname.clone(), c.name.clone()), c.ty.clone()))
                })
                .collect();

            let stats_tree = arb_statistics_for_tables(&tables)
                .new_tree(&mut runner)
                .unwrap();
            let stats = stats_tree.current();

            for stat in &stats {
                for col_ref in &stat.columns {
                    if let StatisticColumn::Column(col_name) = col_ref {
                        let ty = col_type_map
                            .get(&(stat.target.clone(), col_name.clone()))
                            .unwrap_or_else(|| {
                                panic!(
                                    "statistic '{}' references column '{}' not found in table '{}'",
                                    stat.qname.render_sql(),
                                    col_name.as_str(),
                                    stat.target.render_sql()
                                )
                            });
                        assert!(
                            ty.has_default_btree_opclass(),
                            "statistic '{}' references column '{}' of ineligible type {:?}",
                            stat.qname.render_sql(),
                            col_name.as_str(),
                            ty
                        );
                    }
                }
            }
        }
    }
}
