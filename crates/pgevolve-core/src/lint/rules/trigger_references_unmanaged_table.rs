//! `trigger-references-unmanaged-table` lint rule.

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `trigger-references-unmanaged-table` — fires (Error) when a trigger's
/// target table is not declared in the source catalog as a table, view, or
/// materialized view. Triggers can fire on plain tables, views (`INSTEAD OF`),
/// and materialized views, so all three collections are checked.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for trigger in &tree.catalog.triggers {
        let managed = tree.catalog.tables.iter().any(|t| t.qname == trigger.table)
            || tree.catalog.views.iter().any(|v| v.qname == trigger.table)
            || tree
                .catalog
                .materialized_views
                .iter()
                .any(|mv| mv.qname == trigger.table);

        if !managed {
            out.push(Finding::error(
                "trigger-references-unmanaged-table",
                format!(
                    "trigger `{qname}` fires on `{table}`, which is not declared in this \
                     project's managed schema",
                    qname = trigger.qname,
                    table = trigger.table,
                ),
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;
    use crate::ir::view::{MaterializedView, View};
    use crate::lint::test_helpers::{empty_tree, id, make_function_bare, make_trigger, qn};
    use crate::parse::normalize_body::NormalizedBody;

    #[test]
    fn trigger_references_unmanaged_table_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Function IS managed but the table is not in the catalog.
        c.functions.push(make_function_bare("app", "audit_fn"));
        c.triggers.push(make_trigger(
            "app",
            "trg_missing",
            "app",
            "ghost_table", // not in catalog
            "app",
            "audit_fn",
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-table")
            .count();
        assert_eq!(
            count, 1,
            "expected one trigger-references-unmanaged-table finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "trigger-references-unmanaged-table")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn trigger_on_managed_view_no_finding() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // INSTEAD OF triggers fire on views.
        c.views.push(View {
            qname: qn("app", "editable_orders"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        c.functions.push(make_function_bare("app", "audit_fn"));
        c.triggers.push(make_trigger(
            "app",
            "trg_view",
            "app",
            "editable_orders", // view, not a table
            "app",
            "audit_fn",
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-table")
            .count();
        assert_eq!(
            count, 0,
            "trigger-references-unmanaged-table must not fire when target is a managed view",
        );
    }

    #[test]
    fn trigger_on_managed_mv_no_finding() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "order_summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        c.functions.push(make_function_bare("app", "audit_fn"));
        c.triggers.push(make_trigger(
            "app",
            "trg_mv",
            "app",
            "order_summary", // materialized view
            "app",
            "audit_fn",
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-table")
            .count();
        assert_eq!(
            count, 0,
            "trigger-references-unmanaged-table must not fire when target is a managed MV",
        );
    }
}
