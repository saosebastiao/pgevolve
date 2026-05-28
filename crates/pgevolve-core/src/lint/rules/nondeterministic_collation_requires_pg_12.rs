//! Errors when source declares a nondeterministic collation but
//! `[managed].min_pg_version < 12`.
//!
//! Nondeterministic collations are a Postgres 12 feature. Declaring one on
//! a project targeting PG 11 or earlier would fail at apply time; surface
//! the misconfiguration at plan time with a clear remediation message.
//!
//! Plan-time gate — registered via
//! [`crate::lint::universal::check_plan_time_catalog`].

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "nondeterministic-collation-requires-pg-12";

pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    if min_pg_version >= 12 {
        return Vec::new();
    }
    source
        .collations
        .iter()
        .filter(|c| !c.deterministic)
        .map(|c| {
            Finding::error(
                RULE_ID,
                format!(
                    "collation {}: nondeterministic collations require Postgres 12 or later \
                     (min_pg_version = {min_pg_version}); raise [managed].min_pg_version to 12 \
                     or remove the `deterministic = false` option",
                    c.qname,
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::QualifiedName;
    use crate::ir::collation::{Collation, CollationProvider};
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::qn;

    fn make_collation(qname: QualifiedName, deterministic: bool) -> Collation {
        Collation {
            qname,
            provider: CollationProvider::Icu,
            lc_collate: "und".into(),
            lc_ctype: "und".into(),
            deterministic,
            version: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn empty_catalog_silent() {
        let cat = Catalog::empty();
        assert!(check(&cat, 11).is_empty());
    }

    #[test]
    fn deterministic_collation_silent_on_pg11() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_collation(qn("app", "ok"), true));
        assert!(check(&cat, 11).is_empty());
    }

    #[test]
    fn nondeterministic_silent_on_pg12_or_higher() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_collation(qn("app", "ci"), false));
        assert!(check(&cat, 12).is_empty());
        assert!(check(&cat, 17).is_empty());
    }

    #[test]
    fn nondeterministic_fires_on_pg11() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_collation(qn("app", "ci"), false));
        let findings = check(&cat, 11);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("app.ci"));
        assert!(findings[0].message.contains("Postgres 12"));
    }

    #[test]
    fn multiple_nondeterministic_each_fire() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_collation(qn("app", "a"), false));
        cat.collations.push(make_collation(qn("app", "b"), false));
        let findings = check(&cat, 11);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.severity == Severity::Error));
    }
}
