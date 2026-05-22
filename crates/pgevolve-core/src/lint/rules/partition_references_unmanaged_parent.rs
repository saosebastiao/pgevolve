//! `partition-references-unmanaged-parent` lint rule.

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `partition-references-unmanaged-parent` — fires (Error) when a partition
/// table's PARTITION OF target parent is not declared in the source catalog
/// as a table. The parent must be a managed source object.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for table in &tree.catalog.tables {
        let Some(po) = &table.partition_of else {
            continue;
        };
        let found = tree
            .catalog
            .tables
            .iter()
            .any(|other| other.qname == po.parent);

        if !found {
            out.push(Finding::error(
                "partition-references-unmanaged-parent",
                format!(
                    "partition `{}` references parent `{}`, which is not declared in this \
                     project's managed schema",
                    table.qname, po.parent,
                ),
            ));
        }
    }

    out
}
