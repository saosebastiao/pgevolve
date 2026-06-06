//! `trigger-references-unmanaged-function` lint rule.

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `trigger-references-unmanaged-function` — fires (Error) when a trigger's
/// execute function is not declared in the source catalog. The function must be
/// a managed source object, not just present in the live database via an
/// extension or external schema.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for trigger in &tree.catalog.triggers {
        let managed = tree
            .catalog
            .functions
            .iter()
            .any(|f| f.qname == trigger.function_qname);

        if !managed {
            out.push(Finding::error(
                "trigger-references-unmanaged-function",
                format!(
                    "trigger `{qname}` executes function `{func}`, which is not declared in \
                     this project's managed schema",
                    qname = trigger.qname,
                    func = trigger.function_qname,
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
    use crate::ir::table::Table;
    use crate::lint::test_helpers::{empty_tree, id, make_trigger, qn};

    #[test]
    fn trigger_references_unmanaged_function_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Table IS managed but the function is not.
        c.tables.push(Table {
            qname: qn("app", "orders"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        });
        c.triggers.push(make_trigger(
            "app",
            "trg_no_fn",
            "app",
            "orders",
            "app",
            "missing_fn", // not in catalog
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-function")
            .count();
        assert_eq!(
            count, 1,
            "expected one trigger-references-unmanaged-function finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "trigger-references-unmanaged-function")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }
}
