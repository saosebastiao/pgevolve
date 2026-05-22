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
