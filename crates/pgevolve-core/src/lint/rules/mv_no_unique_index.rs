//! `mv-no-unique-index` lint rule.

use crate::ir::index::IndexParent;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `mv-no-unique-index` — fires when a materialized view has no unique index,
/// making `REFRESH MATERIALIZED VIEW CONCURRENTLY` unavailable. Plain `REFRESH`
/// blocks reads for the duration of the refresh.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for mv in &tree.catalog.materialized_views {
        let has_unique = tree
            .catalog
            .indexes
            .iter()
            .any(|idx| idx.unique && matches!(&idx.on, IndexParent::Mv(q) if q == &mv.qname));

        if !has_unique {
            out.push(Finding::warning(
                "mv-no-unique-index",
                format!(
                    "MV `{q}` has no unique index — REFRESH MATERIALIZED VIEW CONCURRENTLY is \
                     unavailable; plain REFRESH will block reads",
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
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;
    use crate::ir::view::MaterializedView;
    use crate::lint::test_helpers::{empty_tree, id, qn};
    use crate::parse::normalize_body::NormalizedBody;

    #[test]
    fn mv_no_unique_index_fires_when_mv_has_no_unique_index() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        // Non-unique index on the MV — should still trigger the warning.
        c.indexes.push(Index {
            qname: qn("app", "summary_idx"),
            on: IndexParent::Mv(qn("app", "summary")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "mv-no-unique-index")
            .count();
        assert_eq!(count, 1, "expected one mv-no-unique-index warning");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "mv-no-unique-index")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn mv_no_unique_index_passes_when_unique_index_present() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        // Unique index on the MV — rule must NOT fire.
        c.indexes.push(Index {
            qname: qn("app", "summary_id_uidx"),
            on: IndexParent::Mv(qn("app", "summary")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "mv-no-unique-index")
            .count();
        assert_eq!(
            count, 0,
            "expected no mv-no-unique-index findings when unique index present"
        );
    }

    #[test]
    fn mv_no_unique_index_clean_catalog_no_mvs() {
        // Catalog with only tables — rule must stay silent.
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
            tablespace: None,
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert!(
            findings.iter().all(|f| f.rule != "mv-no-unique-index"),
            "mv-no-unique-index must not fire on a catalog with no MVs",
        );
    }
}
