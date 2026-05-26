//! Errors when source uses a PG 15+ publication feature but `min_pg_version < 15`.
//!
//! Three PG 15+ features in publications:
//! - `FOR TABLES IN SCHEMA` — schema-scope publication (`Selective { schemas, .. }`
//!   with non-empty `schemas`).
//! - Row filters (`PublishedTable.row_filter.is_some()`).
//! - Column lists (`PublishedTable.columns.is_some()`).
//!
//! If any of these are used and `min_pg_version < 15`, this rule fires an Error
//! (not a Warning, not waivable — using PG15+ syntax on a project declaring PG14
//! is genuine misconfiguration).
//!
//! Source-only rule: checks the source catalog against `min_pg_version`.

use crate::ir::catalog::Catalog;
use crate::ir::publication::PublicationScope;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "publication-feature-requires-pg-version";

/// Check source publications against `min_pg_version`.
///
/// Fires `Severity::Error` when a PG 15+ publication feature is used and
/// `min_pg_version < 15`.
pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    if min_pg_version >= 15 {
        return Vec::new();
    }

    let mut findings = Vec::new();

    for pub_ in &source.publications {
        let PublicationScope::Selective { schemas, tables } = &pub_.scope else {
            continue;
        };

        if !schemas.is_empty() {
            findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Error,
                message: format!(
                    "publication {}: FOR TABLES IN SCHEMA requires Postgres 15 or later (min_pg_version = {}); raise [managed].min_pg_version to 15 or remove the schema scope",
                    pub_.name, min_pg_version,
                ),
                location: None,
            });
        }

        for t in tables {
            if t.row_filter.is_some() {
                findings.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Error,
                    message: format!(
                        "publication {}: row filter on table {} requires Postgres 15 or later (min_pg_version = {}); raise [managed].min_pg_version to 15 or remove the row filter",
                        pub_.name, t.qname, min_pg_version,
                    ),
                    location: None,
                });
            }
            if t.columns.is_some() {
                findings.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Error,
                    message: format!(
                        "publication {}: column list on table {} requires Postgres 15 or later (min_pg_version = {}); raise [managed].min_pg_version to 15 or remove the column list",
                        pub_.name, t.qname, min_pg_version,
                    ),
                    location: None,
                });
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};
    use std::collections::BTreeSet;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn base_pub(name: &str, scope: PublicationScope) -> Publication {
        Publication {
            name: id(name),
            scope,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn pg15_or_higher_always_silent() {
        let mut source = Catalog::empty();
        // Publication with schema scope.
        source.publications.push(base_pub(
            "p",
            PublicationScope::Selective {
                schemas: BTreeSet::from([id("app")]),
                tables: vec![],
            },
        ));
        // min_pg_version = 15 → no findings.
        assert!(check(&source, 15).is_empty());
        // min_pg_version = 17 → no findings.
        assert!(check(&source, 17).is_empty());
    }

    #[test]
    fn schema_scope_fires_on_pg14() {
        let mut source = Catalog::empty();
        source.publications.push(base_pub(
            "p",
            PublicationScope::Selective {
                schemas: BTreeSet::from([id("app")]),
                tables: vec![],
            },
        ));
        let findings = check(&source, 14);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("FOR TABLES IN SCHEMA"));
    }

    #[test]
    fn row_filter_fires_on_pg14() {
        let mut source = Catalog::empty();
        source.publications.push(base_pub(
            "p",
            PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![PublishedTable {
                    qname: qn("app", "orders"),
                    row_filter: Some(NormalizedExpr::from_text("status = 'active'")),
                    columns: None,
                }],
            },
        ));
        let findings = check(&source, 14);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("row filter"));
    }

    #[test]
    fn column_list_fires_on_pg14() {
        let mut source = Catalog::empty();
        source.publications.push(base_pub(
            "p",
            PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![PublishedTable {
                    qname: qn("app", "orders"),
                    row_filter: None,
                    columns: Some(vec![id("id"), id("status")]),
                }],
            },
        ));
        let findings = check(&source, 14);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("column list"));
    }

    #[test]
    fn all_tables_scope_silent_on_pg14() {
        let mut source = Catalog::empty();
        source
            .publications
            .push(base_pub("p", PublicationScope::AllTables));
        // AllTables has no PG15+ features.
        assert!(check(&source, 14).is_empty());
    }

    #[test]
    fn explicit_table_list_only_silent_on_pg14() {
        let mut source = Catalog::empty();
        // Selective with only explicit table list (no schemas, no row filter, no column list).
        source.publications.push(base_pub(
            "p",
            PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![PublishedTable {
                    qname: qn("app", "orders"),
                    row_filter: None,
                    columns: None,
                }],
            },
        ));
        assert!(check(&source, 14).is_empty());
    }

    #[test]
    fn multiple_pg15_features_each_fire() {
        let mut source = Catalog::empty();
        source.publications.push(base_pub(
            "p",
            PublicationScope::Selective {
                schemas: BTreeSet::from([id("app")]),
                tables: vec![
                    PublishedTable {
                        qname: qn("app", "t1"),
                        row_filter: Some(NormalizedExpr::from_text("x > 0")),
                        columns: Some(vec![id("x")]),
                    },
                    PublishedTable {
                        qname: qn("app", "t2"),
                        row_filter: None,
                        columns: Some(vec![id("id")]),
                    },
                ],
            },
        ));
        // 1 (schema) + 2 (row_filter t1) + 2 (column list t1, t2) = wait, let's count:
        // schema: 1
        // t1 row_filter: 1
        // t1 columns: 1
        // t2 columns: 1
        // Total: 4
        let findings = check(&source, 14);
        assert_eq!(findings.len(), 4, "expected 4 findings, got: {findings:?}");
        assert!(findings.iter().all(|f| f.severity == Severity::Error));
    }
}
