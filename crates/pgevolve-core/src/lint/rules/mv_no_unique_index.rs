//! `mv-no-unique-index` lint rule.

use crate::ir::index::IndexParent;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `mv-no-unique-index` — fires when a materialized view has no unique index,
/// making `REFRESH MATERIALIZED VIEW CONCURRENTLY` unavailable. Plain `REFRESH`
/// blocks reads for the duration of the refresh.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for mv in &tree.catalog.materialized_views {
        let has_unique = tree
            .catalog
            .indexes
            .iter()
            .any(|idx| idx.unique && matches!(&idx.on, IndexParent::Mv(q) if q == &mv.qname));

        if !has_unique {
            out.push(Finding::warning(
                "mv-no-unique-index",
                format!(
                    "MV `{q}` has no unique index — REFRESH MATERIALIZED VIEW CONCURRENTLY is \
                     unavailable; plain REFRESH will block reads",
                    q = mv.qname,
                ),
            ));
        }
    }

    out
}
