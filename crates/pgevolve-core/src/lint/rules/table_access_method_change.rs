//! Advisory: an existing table's access method differs between source and live.
//! pgevolve does not rewrite a table's access method (heavy full-table rewrite;
//! `ALTER TABLE … SET ACCESS METHOD` is PG 15+). The operator runs it manually.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

/// Rule ID emitted on the finding; matches the file name.
pub const RULE_ID: &str = "table-access-method-change";

/// Compares table access methods between `source` (desired) and `target` (live).
///
/// Fires an advisory [`Severity::Warning`] for every table that exists in both
/// catalogs whose source access method is set and differs from the live one.
/// `access_method` is post-canon (`heap` → `None`), so a source `None`
/// (unspecified / heap) never fires — the rule is lenient. pgevolve does not
/// rewrite a table's access method, so the finding is purely advisory: the
/// operator must run `ALTER TABLE … SET ACCESS METHOD` manually (PG 15+) if the
/// change was intentional.
pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut out = Vec::new();
    for s in &source.tables {
        if let Some(t) = target.tables.iter().find(|t| t.qname == s.qname)
            && s.access_method.is_some()
            && s.access_method != t.access_method
        {
            out.push(Finding {
                severity: Severity::Warning,
                rule: RULE_ID,
                message: format!(
                    "table {} access method differs: live={:?}, source={:?} \
                     — pgevolve does not rewrite a table's access method; run \
                     ALTER TABLE … SET ACCESS METHOD manually (PG 15+) if intended",
                    s.qname,
                    t.access_method
                        .as_ref()
                        .map(crate::identifier::Identifier::as_str),
                    s.access_method
                        .as_ref()
                        .map(crate::identifier::Identifier::as_str),
                ),
                location: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::table::Table;
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::{id, qn};

    fn table(schema: &str, name: &str, am: Option<&str>) -> Table {
        Table {
            qname: qn(schema, name),
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
            access_method: am.map(id),
            tablespace: None,
        }
    }

    fn catalog_with(tables: Vec<Table>) -> Catalog {
        let mut c = Catalog::empty();
        c.tables = tables;
        c
    }

    #[test]
    fn differing_access_method_fires_one_finding() {
        let source = catalog_with(vec![table("app", "events", Some("columnar"))]);
        let target = catalog_with(vec![table("app", "events", None)]);
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("columnar"),
            "message should mention the source access method: {}",
            findings[0].message
        );
    }

    #[test]
    fn same_access_method_no_finding() {
        let source = catalog_with(vec![table("app", "events", Some("columnar"))]);
        let target = catalog_with(vec![table("app", "events", Some("columnar"))]);
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn table_only_in_source_no_finding() {
        let source = catalog_with(vec![table("app", "events", Some("columnar"))]);
        let target = catalog_with(vec![]);
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn source_access_method_none_no_finding() {
        // Lenient: source None (heap / unspecified) never fires, even when the
        // live table reports a non-default access method.
        let source = catalog_with(vec![table("app", "events", None)]);
        let target = catalog_with(vec![table("app", "events", Some("columnar"))]);
        assert!(check(&source, &target).is_empty());
    }
}
