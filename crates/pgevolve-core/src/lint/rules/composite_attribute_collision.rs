//! `composite-attribute-collision` lint rule.

use std::collections::HashSet;

use crate::ir::user_type::UserTypeKind;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `composite-attribute-collision` — fires when a composite type has duplicate
/// attribute names.
///
/// The source parser rejects duplicates at parse time, so this is a
/// defense-in-depth check for catalogs constructed programmatically.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for ty in &tree.catalog.types {
        if let UserTypeKind::Composite { attributes } = &ty.kind {
            let mut seen: HashSet<&str> = HashSet::new();
            for attr in attributes {
                if !seen.insert(attr.name.as_str()) {
                    out.push(Finding::error(
                        "composite-attribute-collision",
                        format!(
                            "composite type `{q}` has duplicate attribute `{attr}`",
                            q = ty.qname,
                            attr = attr.name.as_str(),
                        ),
                    ));
                }
            }
        }
    }

    out
}
