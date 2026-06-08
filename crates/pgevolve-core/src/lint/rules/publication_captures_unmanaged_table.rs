//! Warns when a publication's implicit scope captures tables not declared in source.
//!
//! A `FOR ALL TABLES` or `FOR TABLES IN SCHEMA s` publication implicitly
//! captures every current and future table in scope. This lint surfaces every
//! catalog-reported table that falls outside the source IR's managed set,
//! helping catch "I added a table and it silently started replicating."
//!
//! Firing conditions:
//! - `AllTables`: for each target table whose qname is not in source's tables list.
//! - `Selective { schemas, .. }` with non-empty `schemas`: for each schema in
//!   the publication's schema list that is not declared in source → schema-level
//!   finding. Then for each target table in that schema not in source → table-level
//!   finding.
//! - `Selective` with only an explicit table list (no schemas) → no findings.

use crate::ir::catalog::Catalog;
use crate::ir::publication::PublicationScope;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "publication-captures-unmanaged-table";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Build a set of table qnames present in source for O(1) lookup.
    let source_table_qnames: std::collections::BTreeSet<_> =
        source.tables.iter().map(|t| &t.qname).collect();

    // Build a set of schema names present in source for O(1) lookup.
    let source_schema_names: std::collections::BTreeSet<_> =
        source.schemas.iter().map(|s| &s.name).collect();

    for pub_ in &source.publications {
        match &pub_.scope {
            PublicationScope::AllTables => {
                // FOR ALL TABLES: every target table not in source is implicitly captured.
                for tgt_table in &target.tables {
                    if !source_table_qnames.contains(&tgt_table.qname) {
                        findings.push(Finding {
                            rule: RULE_ID,
                            severity: Severity::Warning,
                            message: format!(
                                "publication {}: FOR ALL TABLES captures table {} which is not declared in source",
                                pub_.name, tgt_table.qname,
                            ),
                            location: None,
                        });
                    }
                }
            }
            PublicationScope::Selective { schemas, .. } if !schemas.is_empty() => {
                // FOR TABLES IN SCHEMA: check each schema and tables in it.
                for schema in schemas {
                    if !source_schema_names.contains(schema) {
                        findings.push(Finding {
                            rule: RULE_ID,
                            severity: Severity::Warning,
                            message: format!(
                                "publication {}: FOR TABLES IN SCHEMA captures schema {} which is not declared in source",
                                pub_.name, schema,
                            ),
                            location: None,
                        });
                    }
                    // Also check each target table in this schema.
                    for tgt_table in target.tables.iter().filter(|t| &t.qname.schema == schema) {
                        if !source_table_qnames.contains(&tgt_table.qname) {
                            findings.push(Finding {
                                rule: RULE_ID,
                                severity: Severity::Warning,
                                message: format!(
                                    "publication {}: FOR TABLES IN SCHEMA captures table {} which is not declared in source",
                                    pub_.name, tgt_table.qname,
                                ),
                                location: None,
                            });
                        }
                    }
                }
            }
            // Selective with only explicit table list (no schemas) — no implicit capture.
            PublicationScope::Selective { .. } => {}
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};
    use crate::ir::reloptions::TableStorageOptions;
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;
    use std::collections::BTreeSet;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
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
            storage: TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
        }
    }

    fn all_tables_pub(name: &str) -> Publication {
        Publication {
            name: id(name),
            scope: PublicationScope::AllTables,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    fn schema_pub(name: &str, schemas: &[&str]) -> Publication {
        Publication {
            name: id(name),
            scope: PublicationScope::Selective {
                schemas: schemas.iter().map(|s| id(s)).collect(),
                tables: vec![],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    fn table_list_pub(name: &str, tables: &[(&str, &str)]) -> Publication {
        Publication {
            name: id(name),
            scope: PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: tables
                    .iter()
                    .map(|(s, t)| PublishedTable {
                        qname: qn(s, t),
                        row_filter: None,
                        columns: None,
                    })
                    .collect(),
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn empty_catalogs_silent() {
        let source = Catalog::empty();
        let target = Catalog::empty();
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn all_tables_with_managed_table_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.publications.push(all_tables_pub("p"));
        source.tables.push(empty_table(qn("app", "orders")));
        target.tables.push(empty_table(qn("app", "orders")));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn all_tables_fires_for_unmanaged_target_table() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.publications.push(all_tables_pub("p"));
        // Target has a table that source doesn't declare.
        target.tables.push(empty_table(qn("app", "untracked")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("untracked"));
    }

    #[test]
    fn schema_scope_fires_for_unmanaged_schema() {
        let mut source = Catalog::empty();
        let target = Catalog::empty();
        // Publication captures schema "legacy" but it's not in source.
        source.publications.push(schema_pub("p", &["legacy"]));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert!(findings[0].message.contains("legacy"));
    }

    #[test]
    fn schema_scope_fires_for_unmanaged_table_in_schema() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        // Publication captures schema "app" which is declared in source.
        source.schemas.push(Schema::new(id("app")));
        source.publications.push(schema_pub("p", &["app"]));
        // Target has a table in "app" that source doesn't declare.
        target.tables.push(empty_table(qn("app", "untracked")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("untracked"));
    }

    #[test]
    fn explicit_table_list_only_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        // Selective with only explicit table list (no schemas) — no implicit capture.
        source
            .publications
            .push(table_list_pub("p", &[("app", "orders")]));
        // Target has unmanaged tables — rule should NOT fire for explicit table lists.
        target.tables.push(empty_table(qn("app", "untracked")));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn no_publications_in_source_silent() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target.tables.push(empty_table(qn("app", "t")));
        // No publications in source → rule doesn't fire.
        assert!(check(&source, &target).is_empty());
    }
}
