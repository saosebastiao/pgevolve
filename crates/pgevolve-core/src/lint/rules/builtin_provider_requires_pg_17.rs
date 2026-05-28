//! Errors when source declares a collation with `provider = builtin` but
//! `[managed].min_pg_version < 17`.
//!
//! The `builtin` collation provider was added in Postgres 17. Declaring one
//! on a project targeting an earlier release would fail at apply time;
//! surface the misconfiguration at plan time with a clear remediation
//! message.
//!
//! Plan-time gate — registered via
//! [`crate::lint::universal::check_plan_time_catalog`].

use crate::ir::catalog::Catalog;
use crate::ir::collation::CollationProvider;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "builtin-provider-requires-pg-17";

pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    if min_pg_version >= 17 {
        return Vec::new();
    }
    source
        .collations
        .iter()
        .filter(|c| c.provider == CollationProvider::Builtin)
        .map(|c| {
            Finding::error(
                RULE_ID,
                format!(
                    "collation {}: provider = builtin requires Postgres 17 or later \
                     (min_pg_version = {min_pg_version}); raise [managed].min_pg_version to 17 \
                     or choose a different provider",
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

    fn make_collation(qname: QualifiedName, provider: CollationProvider) -> Collation {
        Collation {
            qname,
            provider,
            lc_collate: "C".into(),
            lc_ctype: "C".into(),
            deterministic: true,
            version: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn empty_catalog_silent() {
        let cat = Catalog::empty();
        assert!(check(&cat, 14).is_empty());
    }

    #[test]
    fn libc_silent_on_any_version() {
        let mut cat = Catalog::empty();
        cat.collations
            .push(make_collation(qn("app", "ok"), CollationProvider::Libc));
        assert!(check(&cat, 14).is_empty());
        assert!(check(&cat, 17).is_empty());
    }

    #[test]
    fn icu_silent_on_any_version() {
        let mut cat = Catalog::empty();
        cat.collations
            .push(make_collation(qn("app", "ok"), CollationProvider::Icu));
        assert!(check(&cat, 14).is_empty());
        assert!(check(&cat, 17).is_empty());
    }

    #[test]
    fn builtin_silent_on_pg17_or_higher() {
        let mut cat = Catalog::empty();
        cat.collations
            .push(make_collation(qn("app", "bi"), CollationProvider::Builtin));
        assert!(check(&cat, 17).is_empty());
        assert!(check(&cat, 18).is_empty());
    }

    #[test]
    fn builtin_fires_on_pg16() {
        let mut cat = Catalog::empty();
        cat.collations
            .push(make_collation(qn("app", "bi"), CollationProvider::Builtin));
        let findings = check(&cat, 16);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("app.bi"));
        assert!(findings[0].message.contains("Postgres 17"));
    }

    #[test]
    fn multiple_builtins_each_fire() {
        let mut cat = Catalog::empty();
        cat.collations
            .push(make_collation(qn("app", "a"), CollationProvider::Builtin));
        cat.collations
            .push(make_collation(qn("app", "b"), CollationProvider::Builtin));
        let findings = check(&cat, 14);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.severity == Severity::Error));
    }
}
