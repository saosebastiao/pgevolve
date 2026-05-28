//! `domain-check-references-unmanaged-type` lint rule.

use std::collections::HashSet;

use crate::ir::user_type::UserTypeKind;
use crate::lint::ManagedConfig;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

use super::{BUILTIN_SCHEMAS, extract_qualified_refs};

/// `domain-check-references-unmanaged-type` — fires (Warning) when a domain's
/// CHECK constraint expression text contains a `schema.name` reference where the
/// schema is neither in `[managed].schemas` nor a `PostgreSQL` built-in schema.
///
/// This is a forward-looking check (full resolution lands in v0.3 when
/// functions are supported), using simple text-based extraction of qualified
/// identifiers from the canonical expression text.
pub fn check(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    // If [managed].schemas is empty we cannot determine what is "unmanaged".
    if managed.schemas.is_empty() {
        return Vec::new();
    }

    let managed_set: HashSet<&str> = managed
        .schemas
        .iter()
        .map(crate::identifier::Identifier::as_str)
        .collect();

    let mut out = Vec::new();

    for ty in &tree.catalog.types {
        let UserTypeKind::Domain {
            check_constraints, ..
        } = &ty.kind
        else {
            continue;
        };

        for check in check_constraints {
            let refs = extract_qualified_refs(&check.expression.canonical_text);
            for (schema, _name) in refs {
                if BUILTIN_SCHEMAS.contains(&schema.as_str()) {
                    continue;
                }
                if managed_set.contains(schema.as_str()) {
                    continue;
                }
                out.push(Finding::warning(
                    "domain-check-references-unmanaged-type",
                    format!(
                        "domain `{q}` CHECK constraint `{chk}` references schema `{schema}` \
                         which is not in [managed].schemas",
                        q = ty.qname,
                        chk = check.name.as_str(),
                    ),
                ));
                // One warning per check constraint per unmanaged schema is
                // sufficient — break after the first unmanaged reference.
                break;
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
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::schema::Schema;
    use crate::ir::user_type::{DomainCheck, UserType, UserTypeKind};
    use crate::lint::test_helpers::{empty_tree, id, qn};

    #[test]
    fn domain_check_references_unmanaged_type_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "positive_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: false,
                default: None,
                check_constraints: vec![DomainCheck {
                    name: id("positive_int_check"),
                    // references external.validate_int — schema "external" is not managed
                    expression: NormalizedExpr::from_text(
                        "value > 0 and external.validate_int(value)",
                    ),
                }],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "domain-check-references-unmanaged-type")
            .count();
        assert_eq!(
            count, 1,
            "expected one domain-check-references-unmanaged-type warning"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "domain-check-references-unmanaged-type")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn domain_check_references_unmanaged_type_silent_on_managed_schema() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "positive_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: false,
                default: None,
                check_constraints: vec![DomainCheck {
                    name: id("positive_int_check"),
                    // references app.validate_int — "app" is managed
                    expression: NormalizedExpr::from_text("app.validate_int(value)"),
                }],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "domain-check-references-unmanaged-type"),
            "rule must not fire when referenced schema is managed",
        );
    }

    #[test]
    fn domain_check_references_unmanaged_type_silent_for_pg_catalog() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "text_domain"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Text,
                nullable: false,
                default: None,
                check_constraints: vec![DomainCheck {
                    name: id("not_empty"),
                    // references pg_catalog — built-in, always exempt
                    expression: NormalizedExpr::from_text("pg_catalog.char_length(value) > 0"),
                }],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "domain-check-references-unmanaged-type"),
            "rule must not fire for pg_catalog references",
        );
    }
}
