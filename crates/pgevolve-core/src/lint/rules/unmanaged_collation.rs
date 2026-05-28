//! Warns when the catalog has a collation not declared in source.
//!
//! See [`super::check_unmanaged_objects`] for the shared lenient-drift policy.

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "unmanaged-collation";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    super::check_unmanaged_objects(
        &target.collations,
        &source.collations,
        |c| &c.qname,
        RULE_ID,
        "collation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::QualifiedName;
    use crate::ir::collation::{Collation, CollationProvider};
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::qn;

    fn make_collation(qname: QualifiedName) -> Collation {
        Collation {
            qname,
            provider: CollationProvider::Libc,
            lc_collate: "C".into(),
            lc_ctype: "C".into(),
            deterministic: true,
            version: None,
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
    fn target_only_fires() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target.collations.push(make_collation(qn("app", "drift")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("app.drift"),
            "message should mention the collation: {}",
            findings[0].message,
        );
    }

    #[test]
    fn matching_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.collations.push(make_collation(qn("app", "managed")));
        target.collations.push(make_collation(qn("app", "managed")));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn source_only_silent() {
        let mut source = Catalog::empty();
        let target = Catalog::empty();
        source.collations.push(make_collation(qn("app", "managed")));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn partial_overlap_fires_only_for_extras() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.collations.push(make_collation(qn("app", "shared")));
        target.collations.push(make_collation(qn("app", "shared")));
        target.collations.push(make_collation(qn("app", "extra")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("app.extra"));
    }
}
