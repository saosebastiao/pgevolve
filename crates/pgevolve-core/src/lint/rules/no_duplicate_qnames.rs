//! `no_duplicate_qnames` lint rule.

use std::collections::HashSet;

use crate::lint::finding::Finding;
use crate::lint::source_tree::{ObjectKey, SourceTree};

/// Duplicate qnames are already rejected by `parse_directory`; this is a
/// belt-and-suspenders check that confirms the same invariant on any
/// `SourceTree` regardless of how it was constructed.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen: HashSet<&ObjectKey> = HashSet::new();
    for key in tree.objects() {
        if !seen.insert(key) {
            out.push(Finding::error(
                "no_duplicate_qnames",
                format!("duplicate object: {key}"),
            ));
        }
    }
    out
}
