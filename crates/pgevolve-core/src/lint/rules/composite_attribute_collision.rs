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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::column_type::ColumnType;
    use crate::ir::schema::Schema;
    use crate::ir::user_type::{CompositeAttribute, UserType, UserTypeKind};
    use crate::lint::test_helpers::{empty_tree, id, qn};

    #[test]
    fn composite_attribute_collision_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "address"),
            kind: UserTypeKind::Composite {
                attributes: vec![
                    CompositeAttribute {
                        name: id("street"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: id("street"), // duplicate attribute
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: id("city"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "composite-attribute-collision")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one composite-attribute-collision finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "composite-attribute-collision")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn composite_attribute_collision_silent_on_distinct_attributes() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "address"),
            kind: UserTypeKind::Composite {
                attributes: vec![
                    CompositeAttribute {
                        name: id("street"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: id("city"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "composite-attribute-collision"),
            "composite-attribute-collision must not fire on distinct attributes",
        );
    }
}
