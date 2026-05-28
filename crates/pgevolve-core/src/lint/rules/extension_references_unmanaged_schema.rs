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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::extension::Extension;
    use crate::ir::schema::Schema;
    use crate::lint::test_helpers::{empty_tree, id};

    #[test]
    fn extension_references_unmanaged_schema_fires() {
        let mut c = Catalog::empty();
        c.extensions.push(Extension {
            name: id("pg_trgm"),
            schema: Some(id("missing")),
            version: None,
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-references-unmanaged-schema")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn extension_references_managed_schema_silent() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.extensions.push(Extension {
            name: id("pg_trgm"),
            schema: Some(id("app")),
            version: None,
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-references-unmanaged-schema")
            .count();
        assert_eq!(count, 0);
    }
}
