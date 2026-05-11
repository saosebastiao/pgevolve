//! `free-form` layout profile — no path constraints, only universal rules.

use std::path::Path;

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// No-op; free-form layout enforces no path-shape rules.
pub const fn check(_tree: &SourceTree, _schema_dir: &Path) -> Vec<Finding> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::lint::source_tree::SourceTree;

    #[test]
    fn always_empty() {
        let tree = SourceTree::new(Catalog::empty(), std::collections::HashMap::new());
        assert!(check(&tree, Path::new("schema")).is_empty());
    }
}
