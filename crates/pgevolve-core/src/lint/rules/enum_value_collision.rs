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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;
    use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};
    use crate::lint::test_helpers::{empty_tree, id, qn};

    #[test]
    fn enum_value_collision_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "status"),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue {
                        name: "active".into(),
                        sort_order: 1.0,
                    },
                    EnumValue {
                        name: "active".into(), // duplicate label
                        sort_order: 2.0,
                    },
                    EnumValue {
                        name: "inactive".into(),
                        sort_order: 3.0,
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
            .filter(|f| f.rule == "enum-value-collision")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one enum-value-collision finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "enum-value-collision")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn enum_value_collision_silent_on_distinct_values() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "status"),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue {
                        name: "pending".into(),
                        sort_order: 1.0,
                    },
                    EnumValue {
                        name: "active".into(),
                        sort_order: 2.0,
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
            findings.iter().all(|f| f.rule != "enum-value-collision"),
            "enum-value-collision must not fire on distinct values",
        );
    }
}
