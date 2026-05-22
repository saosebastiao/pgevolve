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
