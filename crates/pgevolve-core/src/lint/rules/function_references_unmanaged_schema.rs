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
                // Publications, subscriptions, and event triggers are not
                // schema-qualified and cannot appear as function body dependency
                // targets; skip. Statistics and collations are schema-qualified
                // but not referenced by function bodies.
                NodeId::Publication(_)
                | NodeId::Subscription(_)
                | NodeId::EventTrigger(_)
                | NodeId::Statistic(_)
                | NodeId::Collation(_)
                | NodeId::Aggregate(..) => continue,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;
    use crate::lint::test_helpers::{empty_arg_types, empty_tree, id, make_plpgsql_function, qn};
    use crate::plan::edges::{DepEdge, DepSource, NodeId};

    #[test]
    fn function_references_unmanaged_schema_fires_on_cross_schema_dep() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.functions.push(make_plpgsql_function(
            "app",
            "cross_fn",
            "BEGIN RETURN external.helper(); END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "cross_fn"), empty_arg_types()),
                to: NodeId::Function(qn("external", "helper"), empty_arg_types()),
                source: DepSource::AstExtracted,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "function-references-unmanaged-schema")
            .count();
        assert_eq!(
            count, 1,
            "expected one function-references-unmanaged-schema warning"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "function-references-unmanaged-schema")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn function_references_unmanaged_schema_silent_on_managed_dep() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.functions.push(make_plpgsql_function(
            "app",
            "managed_fn",
            "BEGIN RETURN app.helper(); END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "managed_fn"), empty_arg_types()),
                to: NodeId::Function(qn("app", "helper"), empty_arg_types()),
                source: DepSource::AstExtracted,
            }],
        ));
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
                .all(|f| f.rule != "function-references-unmanaged-schema"),
            "function-references-unmanaged-schema must not fire when dep is in managed schema",
        );
    }

    #[test]
    fn function_references_unmanaged_schema_silent_on_builtin_schema() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.functions.push(make_plpgsql_function(
            "app",
            "catalog_fn",
            "BEGIN RETURN pg_catalog.now(); END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "catalog_fn"), empty_arg_types()),
                to: NodeId::Function(qn("pg_catalog", "now"), empty_arg_types()),
                source: DepSource::AstExtracted,
            }],
        ));
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
                .all(|f| f.rule != "function-references-unmanaged-schema"),
            "function-references-unmanaged-schema must not fire for pg_catalog",
        );
    }

    #[test]
    fn function_references_unmanaged_schema_silent_when_managed_is_empty() {
        let mut c = Catalog::empty();
        c.functions.push(make_plpgsql_function(
            "app",
            "any_fn",
            "BEGIN NULL; END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "any_fn"), empty_arg_types()),
                to: NodeId::Table(qn("external", "data")),
                source: DepSource::AstExtracted,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check(&tree, &ManagedConfig::default());
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "function-references-unmanaged-schema"),
            "function-references-unmanaged-schema must be silent when managed.schemas is empty",
        );
    }
}
