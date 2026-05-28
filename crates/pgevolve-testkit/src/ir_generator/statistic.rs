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
    // Tables with fewer than 2 columns cannot have statistics.
    let eligible: Vec<(QualifiedName, Vec<Identifier>)> = tables
        .iter()
        .filter_map(|t| {
            let non_pk_cols: Vec<Identifier> = t.columns.iter().map(|c| c.name.clone()).collect();
            if non_pk_cols.len() >= 2 {
                Some((t.qname.clone(), non_pk_cols))
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
