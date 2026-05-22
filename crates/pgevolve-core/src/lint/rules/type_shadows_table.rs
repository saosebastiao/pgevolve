//! `type-shadows-table` lint rule.

use std::collections::HashSet;

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `type-shadows-table` — fires when a user-defined type's qname collides with
/// a table, view, or materialized-view qname. `PostgreSQL` uses one namespace for
/// relations and types, so the conflict would be rejected at apply time.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    // Build a set of all relation qnames (table + view + MV).
    let mut relation_names: HashSet<crate::identifier::QualifiedName> = HashSet::new();
    for t in &tree.catalog.tables {
        relation_names.insert(t.qname.clone());
    }
    for v in &tree.catalog.views {
        relation_names.insert(v.qname.clone());
    }
    for mv in &tree.catalog.materialized_views {
        relation_names.insert(mv.qname.clone());
    }

    for ty in &tree.catalog.types {
        if relation_names.contains(&ty.qname) {
            out.push(Finding::error(
                "type-shadows-table",
                format!(
                    "type `{q}` has the same qualified name as an existing relation \
                     (table, view, or materialized view) — PostgreSQL would reject this",
                    q = ty.qname,
                ),
            ));
        }
    }

    out
}
