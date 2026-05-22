//! `extension-references-unmanaged-schema` lint rule.

use std::collections::BTreeSet;

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `extension-references-unmanaged-schema` — fires when a CREATE EXTENSION
/// references a schema not in the source catalog. Without the schema
/// being managed, the planner can't guarantee ordering.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let managed_schemas: BTreeSet<&str> = tree
        .catalog
        .schemas
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let mut out = Vec::new();
    for e in &tree.catalog.extensions {
        if let Some(schema) = &e.schema
            && !managed_schemas.contains(schema.as_str())
        {
            out.push(Finding::error(
                "extension-references-unmanaged-schema",
                format!(
                    "{}: WITH SCHEMA {} references a schema not declared in source. \
                     Add a CREATE SCHEMA {} to the source or remove the WITH SCHEMA clause.",
                    e.name, schema, schema,
                ),
            ));
        }
    }
    out
}
