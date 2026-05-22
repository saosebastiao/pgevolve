//! `enum-value-collision` lint rule.

use std::collections::HashSet;

use crate::ir::user_type::UserTypeKind;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `enum-value-collision` — fires when an enum type has duplicate value labels.
///
/// The source parser rejects duplicates at parse time, so this is a
/// defense-in-depth check for catalogs constructed programmatically.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for ty in &tree.catalog.types {
        if let UserTypeKind::Enum { values } = &ty.kind {
            let mut seen: HashSet<&str> = HashSet::new();
            for v in values {
                if !seen.insert(v.name.as_str()) {
                    out.push(Finding::error(
                        "enum-value-collision",
                        format!(
                            "enum `{q}` has duplicate value `{label}`",
                            q = ty.qname,
                            label = v.name,
                        ),
                    ));
                }
            }
        }
    }

    out
}
