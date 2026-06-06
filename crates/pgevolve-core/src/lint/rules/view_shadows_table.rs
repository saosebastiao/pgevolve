//! `view-shadows-table` lint rule.

use std::collections::HashSet;

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `view-shadows-table` — fires when a view or materialized view shares a
/// qname with a table in the same catalog. `PostgreSQL` itself would reject the
/// conflict at apply time; the lint catches it earlier.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let table_names: HashSet<_> = tree
        .catalog
        .tables
        .iter()
        .map(|t| t.qname.clone())
        .collect();

    for v in &tree.catalog.views {
        if table_names.contains(&v.qname) {
            out.push(Finding::error(
                "view-shadows-table",
                format!(
                    "view `{q}` has the same name as a table — PostgreSQL would reject this",
                    q = v.qname,
                ),
            ));
        }
    }

    for mv in &tree.catalog.materialized_views {
        if table_names.contains(&mv.qname) {
            out.push(Finding::error(
                "view-shadows-table",
                format!(
                    "materialized view `{q}` has the same name as a table — PostgreSQL would reject this",
                    q = mv.qname,
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
    use crate::ir::table::Table;
    use crate::ir::view::{MaterializedView, View};
    use crate::lint::test_helpers::{empty_tree, id, qn};
    use crate::parse::normalize_body::NormalizedBody;

    #[test]
    fn view_shadows_table_fires_when_view_collides_with_table() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        });
        // View with the same qname as the table.
        c.views.push(View {
            qname: qn("app", "users"),
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
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-shadows-table")
            .count();
        assert_eq!(count, 1, "expected exactly one view-shadows-table finding");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "view-shadows-table")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn view_shadows_table_fires_when_mv_collides_with_table() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "orders"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        });
        // MV with the same qname as the table.
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "orders"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-shadows-table")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one view-shadows-table finding for MV"
        );
    }

    #[test]
    fn view_shadows_table_clean_catalog_passes() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        });
        c.views.push(View {
            qname: qn("app", "active_users"),
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
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "user_summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-shadows-table")
            .count();
        assert_eq!(
            count, 0,
            "expected no view-shadows-table findings on clean catalog"
        );
    }
}
