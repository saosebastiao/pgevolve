//! `extension-version-unpinned` lint rule.

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `extension-version-unpinned` — fires when a source-declared extension
/// has no `VERSION` clause. Unpinned extensions can shift between
/// environments; pinning ensures dev and prod install the same version.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    for e in &tree.catalog.extensions {
        if e.version.is_none() {
            out.push(Finding::warning(
                "extension-version-unpinned",
                format!(
                    "{}: extension is declared without a VERSION clause. Pinning the version \
                     ensures the same version is installed across environments.",
                    e.name,
                ),
            ));
        }
    }
    out
}
