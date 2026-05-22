//! `function-references-unmanaged-schema` lint rule.

use std::collections::HashSet;

use crate::lint::ManagedConfig;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;
use crate::plan::edges::{DepEdge, NodeId};

use super::BUILTIN_SCHEMAS;

/// `function-references-unmanaged-schema` — fires (Warning) when any
/// dependency edge in a function or procedure `body_dependencies` targets a
/// schema that is neither in `[managed].schemas` nor a `PostgreSQL` built-in
/// schema (`pg_catalog`, `information_schema`).
///
/// Mirrors `view-body-references-unmanaged-schema` for routines.
pub fn check(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    // If the user has not populated [managed].schemas we cannot determine
    // what is "unmanaged" — mirror managed_schemas_match's behaviour.
    if managed.schemas.is_empty() {
        return Vec::new();
    }

    let managed_set: HashSet<&str> = managed
        .schemas
        .iter()
        .map(crate::identifier::Identifier::as_str)
        .collect();

    let mut out = Vec::new();

    let check_deps = |routine_qname: &crate::identifier::QualifiedName,
                      kind: &str,
                      deps: &[DepEdge],
                      out: &mut Vec<Finding>| {
        for edge in deps {
            let target_schema = match &edge.to {
                NodeId::Table(q)
                | NodeId::View(q)
                | NodeId::Mv(q)
                | NodeId::Index(q)
                | NodeId::Sequence(q)
                | NodeId::Type(q)
                | NodeId::Procedure(q)
                | NodeId::Trigger(q)
                | NodeId::Function(q, _) => q.schema.as_str(),
                NodeId::Schema(s) | NodeId::Extension(s) => s.as_str(),
                NodeId::Constraint { table, .. } => table.schema.as_str(),
            };

            if BUILTIN_SCHEMAS.contains(&target_schema) {
                continue;
            }
            if managed_set.contains(target_schema) {
                continue;
            }

            out.push(Finding::warning(
                "function-references-unmanaged-schema",
                format!(
                    "{kind} `{routine_qname}` body depends on schema `{target_schema}` \
                     which is not in [managed].schemas",
                ),
            ));
        }
    };

    for f in &tree.catalog.functions {
        check_deps(&f.qname, "function", &f.body_dependencies, &mut out);
    }
    for p in &tree.catalog.procedures {
        check_deps(&p.qname, "procedure", &p.body_dependencies, &mut out);
    }

    out
}
