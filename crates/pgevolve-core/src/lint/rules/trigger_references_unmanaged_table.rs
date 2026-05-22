//! `trigger-references-unmanaged-table` lint rule.

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `trigger-references-unmanaged-table` — fires (Error) when a trigger's
/// target table is not declared in the source catalog as a table, view, or
/// materialized view. Triggers can fire on plain tables, views (`INSTEAD OF`),
/// and materialized views, so all three collections are checked.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for trigger in &tree.catalog.triggers {
        let managed = tree.catalog.tables.iter().any(|t| t.qname == trigger.table)
            || tree.catalog.views.iter().any(|v| v.qname == trigger.table)
            || tree
                .catalog
                .materialized_views
                .iter()
                .any(|mv| mv.qname == trigger.table);

        if !managed {
            out.push(Finding::error(
                "trigger-references-unmanaged-table",
                format!(
                    "trigger `{qname}` fires on `{table}`, which is not declared in this \
                     project's managed schema",
                    qname = trigger.qname,
                    table = trigger.table,
                ),
            ));
        }
    }

    out
}
