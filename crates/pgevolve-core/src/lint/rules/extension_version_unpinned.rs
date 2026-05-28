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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::extension::Extension;
    use crate::lint::test_helpers::{empty_tree, id};

    #[test]
    fn extension_version_unpinned_fires_on_unpinned() {
        let mut c = Catalog::empty();
        c.extensions.push(Extension {
            name: id("pgcrypto"),
            schema: None,
            version: None,
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-version-unpinned")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn extension_version_unpinned_silent_when_pinned() {
        let mut c = Catalog::empty();
        c.extensions.push(Extension {
            name: id("pgcrypto"),
            schema: None,
            version: Some("1.3".into()),
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-version-unpinned")
            .count();
        assert_eq!(count, 0);
    }
}
