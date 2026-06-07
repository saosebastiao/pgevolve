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
                // Publications, subscriptions, and event triggers are not
                // schema-qualified and cannot appear as view body dependency
                // targets; skip. Statistics and collations are schema-qualified
                // but not referenced by view bodies.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;
    use crate::ir::view::View;
    use crate::lint::test_helpers::{empty_tree, id, qn};
    use crate::parse::normalize_body::NormalizedBody;
    use crate::plan::edges::{DepEdge, DepSource, NodeId};

    #[test]
    fn view_body_references_unmanaged_schema_fires_when_dep_in_unmanaged_schema() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::View(qn("app", "my_view")),
                // references table in "external" schema — not managed
                to: NodeId::Table(qn("external", "data")),
                source: DepSource::AstExtracted,
            }],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        // managed only has "app" — "external" is unmanaged
        let findings = check(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-body-references-unmanaged-schema")
            .count();
        assert_eq!(
            count, 1,
            "expected one view-body-references-unmanaged-schema warning"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "view-body-references-unmanaged-schema")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn view_body_references_unmanaged_schema_silent_on_managed_dep() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::View(qn("app", "my_view")),
                // references table in the managed "app" schema — fine
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstExtracted,
            }],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
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
                .all(|f| f.rule != "view-body-references-unmanaged-schema"),
            "rule must not fire when dep is in a managed schema",
        );
    }

    #[test]
    fn view_body_references_unmanaged_schema_silent_on_builtin_schemas() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![
                DepEdge {
                    from: NodeId::View(qn("app", "my_view")),
                    to: NodeId::Table(qn("pg_catalog", "pg_type")),
                    source: DepSource::AstExtracted,
                },
                DepEdge {
                    from: NodeId::View(qn("app", "my_view")),
                    to: NodeId::Table(qn("information_schema", "columns")),
                    source: DepSource::AstExtracted,
                },
            ],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
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
                .all(|f| f.rule != "view-body-references-unmanaged-schema"),
            "rule must not fire for pg_catalog / information_schema references",
        );
    }

    #[test]
    fn view_body_references_unmanaged_schema_silent_when_managed_is_empty() {
        let mut c = Catalog::empty();
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::View(qn("app", "my_view")),
                to: NodeId::Table(qn("anywhere", "stuff")),
                source: DepSource::AstExtracted,
            }],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        // Empty managed config — rule must stay silent (mirrors managed_schemas_match).
        let findings = check(&tree, &ManagedConfig::default());
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "view-body-references-unmanaged-schema"),
            "rule must be silent when [managed].schemas is empty",
        );
    }
}
