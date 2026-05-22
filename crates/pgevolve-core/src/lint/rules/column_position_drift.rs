//! `column-position-drift` lint rule.

use std::collections::{BTreeMap, BTreeSet};

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub fn check(source: &Catalog, target: &Catalog, out: &mut Vec<Finding>) {
    let target_tables: BTreeMap<_, _> =
        target.tables.iter().map(|t| (t.qname.clone(), t)).collect();

    for source_table in &source.tables {
        let Some(target_table) = target_tables.get(&source_table.qname) else {
            continue;
        };
        let source_names: Vec<_> = source_table
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let target_names: Vec<_> = target_table
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect();

        // Only compare columns that exist in both catalogs. Added or removed
        // columns do not constitute position drift — those are handled by the
        // planner.
        let source_set: BTreeSet<_> = source_names.iter().cloned().collect();
        let target_set: BTreeSet<_> = target_names.iter().cloned().collect();
        let common: BTreeSet<_> = source_set.intersection(&target_set).cloned().collect();

        let source_order: Vec<_> = source_names.iter().filter(|n| common.contains(n)).collect();
        let target_order: Vec<_> = target_names.iter().filter(|n| common.contains(n)).collect();

        if source_order != target_order {
            out.push(Finding::lint_at_plan(
                "column-position-drift",
                format!(
                    "{}: column position drift. source order [{}] vs catalog order [{}]",
                    source_table.qname,
                    source_order
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    target_order
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ));
        }
    }
}
