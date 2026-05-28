//! `managed_schemas_match` lint rule.

use std::collections::HashSet;

use crate::lint::ManagedConfig;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

pub fn check(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    let in_source: HashSet<_> = tree
        .catalog
        .schemas
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let in_config: HashSet<_> = managed.schemas.iter().cloned().collect();

    // managed.schemas may be empty (some projects don't list them and rely
    // on filter at apply time). Only flag mismatches when the user has
    // explicitly populated the list.
    if in_config.is_empty() {
        return out;
    }
    for s in &in_source {
        if !in_config.contains(s) {
            out.push(Finding::error(
                "managed_schemas_match",
                format!(
                    "schema `{s}` is declared in source but not listed in `[managed].schemas`",
                ),
            ));
        }
    }
    for s in &in_config {
        if !in_source.contains(s) {
            out.push(Finding::error(
                "managed_schemas_match",
                format!(
                    "schema `{s}` is listed in `[managed].schemas` but not declared in source",
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
    use crate::ir::schema::Schema;
    use crate::lint::ManagedConfig;
    use crate::lint::test_helpers::{empty_tree, id};

    #[test]
    fn managed_schemas_match_passes_when_lists_align() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        let tree = empty_tree(c);
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check(&tree, &managed);
        assert!(f.is_empty(), "got findings: {f:?}");
    }

    #[test]
    fn managed_schemas_match_flags_missing_source_schema() {
        let tree = empty_tree(Catalog::empty());
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check(&tree, &managed);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "managed_schemas_match");
    }

    #[test]
    fn managed_schemas_match_flags_unlisted_source_schema() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("audit")));
        let tree = empty_tree(c);
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check(&tree, &managed);
        // Two findings: app missing in source, audit not listed in managed.
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn managed_schemas_match_skips_when_managed_is_empty() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("any")));
        let tree = empty_tree(c);
        let managed = ManagedConfig::default();
        let f = check(&tree, &managed);
        // Other universal rules may still fire, but managed_schemas_match
        // must be silent.
        assert!(f.iter().all(|x| x.rule != "managed_schemas_match"));
    }
}
