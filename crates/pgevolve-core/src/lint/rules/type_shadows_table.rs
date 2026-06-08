//! `type-shadows-table` lint rule.

use std::collections::HashSet;

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `type-shadows-table` — fires when a user-defined type's qname collides with
/// a table, view, or materialized-view qname. `PostgreSQL` uses one namespace for
/// relations and types, so the conflict would be rejected at apply time.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    // Build a set of all relation qnames (table + view + MV).
    let mut relation_names: HashSet<crate::identifier::QualifiedName> = HashSet::new();
    for t in &tree.catalog.tables {
        relation_names.insert(t.qname.clone());
    }
    for v in &tree.catalog.views {
        relation_names.insert(v.qname.clone());
    }
    for mv in &tree.catalog.materialized_views {
        relation_names.insert(mv.qname.clone());
    }

    for ty in &tree.catalog.types {
        if relation_names.contains(&ty.qname) {
            out.push(Finding::error(
                "type-shadows-table",
                format!(
                    "type `{q}` has the same qualified name as an existing relation \
                     (table, view, or materialized view) — PostgreSQL would reject this",
                    q = ty.qname,
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
    use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};
    use crate::lint::test_helpers::{empty_tree, id, qn};

    #[test]
    fn type_shadows_table_fires_on_collision() {
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
        // An enum type that collides with the table.
        c.types.push(UserType {
            qname: qn("app", "users"),
            kind: UserTypeKind::Enum {
                values: vec![EnumValue {
                    name: "active".into(),
                    sort_order: 1.0,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "type-shadows-table")
            .count();
        assert_eq!(count, 1, "expected exactly one type-shadows-table finding");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "type-shadows-table")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn type_shadows_table_silent_when_no_collision() {
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
        c.types.push(UserType {
            qname: qn("app", "user_status"),
            kind: UserTypeKind::Enum {
                values: vec![EnumValue {
                    name: "active".into(),
                    sort_order: 1.0,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert!(
            findings.iter().all(|f| f.rule != "type-shadows-table"),
            "type-shadows-table must not fire when names are distinct",
        );
    }
}
