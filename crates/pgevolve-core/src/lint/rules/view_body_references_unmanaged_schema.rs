//! `view-body-references-unmanaged-schema` lint rule.

use crate::lint::ManagedConfig;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;
use crate::plan::edges::NodeId;

use super::BUILTIN_SCHEMAS;

/// `view-body-references-unmanaged-schema` — fires when any dependency edge in
/// a view's `body_dependencies` targets a schema that is neither in
/// `[managed].schemas` nor a `PostgreSQL` built-in schema (`pg_catalog`,
/// `information_schema`).
pub fn check(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    // If the user has not populated [managed].schemas, we can't meaningfully
    // determine what is "unmanaged" — mirror managed_schemas_match's behaviour.
    if managed.schemas.is_empty() {
        return Vec::new();
    }

    let managed_set: std::collections::HashSet<&str> = managed
        .schemas
        .iter()
        .map(crate::identifier::Identifier::as_str)
        .collect();

    let mut out = Vec::new();

    let check_deps = |view_qname: &crate::identifier::QualifiedName,
                      deps: &[crate::plan::edges::DepEdge],
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
                "view-body-references-unmanaged-schema",
                format!(
                    "view `{view_qname}` body depends on schema `{target_schema}` which is not \
                     in [managed].schemas",
                ),
            ));
        }
    };

    for v in &tree.catalog.views {
        check_deps(&v.qname, &v.body_dependencies, &mut out);
    }
    for mv in &tree.catalog.materialized_views {
        check_deps(&mv.qname, &mv.body_dependencies, &mut out);
    }

    out
}
