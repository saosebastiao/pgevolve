//! `partition-references-unmanaged-parent` lint rule.

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `partition-references-unmanaged-parent` — fires (Error) when a partition
/// table's PARTITION OF target parent is not declared in the source catalog
/// as a table. The parent must be a managed source object.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for table in &tree.catalog.tables {
        let Some(po) = &table.partition_of else {
            continue;
        };
        let found = tree
            .catalog
            .tables
            .iter()
            .any(|other| other.qname == po.parent);

        if !found {
            out.push(Finding::error(
                "partition-references-unmanaged-parent",
                format!(
                    "partition `{}` references parent `{}`, which is not declared in this \
                     project's managed schema",
                    table.qname, po.parent,
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
    use crate::ir::partition::{
        PartitionBounds, PartitionBy, PartitionColumn, PartitionColumnKind, PartitionOf,
        PartitionStrategy,
    };
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;
    use crate::lint::test_helpers::{empty_tree, id, qn};

    #[test]
    fn partition_with_managed_parent_no_finding() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));

        // Parent table with PARTITION BY clause.
        c.tables.push(Table {
            qname: qn("app", "orders"),
            columns: vec![],
            constraints: vec![],
            partition_by: Some(PartitionBy {
                strategy: PartitionStrategy::Range,
                columns: vec![PartitionColumn {
                    kind: PartitionColumnKind::Column(id("id")),
                    collation: None,
                    opclass: None,
                }],
            }),
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

        // Child partition referencing the parent.
        c.tables.push(Table {
            qname: qn("app", "orders_p1"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: Some(PartitionOf {
                parent: qn("app", "orders"),
                bounds: PartitionBounds::Default,
            }),
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        });

        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "partition-references-unmanaged-parent")
            .count();
        assert_eq!(
            count, 0,
            "expected no partition-references-unmanaged-parent finding when parent is managed"
        );
    }

    #[test]
    fn partition_with_unmanaged_parent_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));

        // Child partition references a parent NOT in the catalog.
        c.tables.push(Table {
            qname: qn("app", "orders_p1"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: Some(PartitionOf {
                parent: qn("app", "ghost_orders"),
                bounds: PartitionBounds::Default,
            }),
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        });

        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "partition-references-unmanaged-parent")
            .count();
        assert_eq!(
            count, 1,
            "expected one partition-references-unmanaged-parent finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "partition-references-unmanaged-parent")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn non_partition_tables_ignored() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));

        // Regular table without partition_of.
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

        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "partition-references-unmanaged-parent")
            .count();
        assert_eq!(
            count, 0,
            "partition-references-unmanaged-parent must not fire for regular tables"
        );
    }
}
