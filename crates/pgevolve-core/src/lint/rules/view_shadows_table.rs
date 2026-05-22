//! `view-shadows-table` lint rule.

use std::collections::HashSet;

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `view-shadows-table` — fires when a view or materialized view shares a
/// qname with a table in the same catalog. `PostgreSQL` itself would reject the
/// conflict at apply time; the lint catches it earlier.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let table_names: HashSet<_> = tree
        .catalog
        .tables
        .iter()
        .map(|t| t.qname.clone())
        .collect();

    for v in &tree.catalog.views {
        if table_names.contains(&v.qname) {
            out.push(Finding::error(
                "view-shadows-table",
                format!(
                    "view `{q}` has the same name as a table — PostgreSQL would reject this",
                    q = v.qname,
                ),
            ));
        }
    }

    for mv in &tree.catalog.materialized_views {
        if table_names.contains(&mv.qname) {
            out.push(Finding::error(
                "view-shadows-table",
                format!(
                    "materialized view `{q}` has the same name as a table — PostgreSQL would reject this",
                    q = mv.qname,
                ),
            ));
        }
    }

    out
}
