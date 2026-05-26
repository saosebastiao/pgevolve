//! Warns when the catalog has a publication not declared in source.
//!
//! Per the lenient drift policy, catalog publications that don't appear
//! in source are NOT dropped by the differ. This lint surfaces them so
//! operators can decide whether to bring under management or accept the drift.
//!
//! Mirrors `unmanaged_reloption`, `unmanaged_grant`, `unmanaged_policy`.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-publication";

/// Fires once per target publication whose name is not in source's publications list.
pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    target
        .publications
        .iter()
        .filter(|tgt_pub| {
            !source
                .publications
                .iter()
                .any(|src_pub| src_pub.name == tgt_pub.name)
        })
        .map(|tgt_pub| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!(
                "publication {}: catalog has a publication not declared in source",
                tgt_pub.name,
            ),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::publication::{Publication, PublicationScope, PublishKinds};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn make_publication(name: &str) -> Publication {
        Publication {
            name: id(name),
            scope: PublicationScope::AllTables,
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
    fn publication_in_source_and_target_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.publications.push(make_publication("my_pub"));
        target.publications.push(make_publication("my_pub"));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn publication_only_in_target_fires() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target.publications.push(make_publication("unmanaged_pub"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("unmanaged_pub"),
            "message should mention the publication name: {}",
            findings[0].message
        );
    }

    #[test]
    fn publication_only_in_source_silent() {
        let mut source = Catalog::empty();
        let target = Catalog::empty();
        source.publications.push(make_publication("managed_pub"));
        // Source-only: not-yet-created; no drift finding needed.
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn multiple_unmanaged_publications_each_fire() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target.publications.push(make_publication("pub_a"));
        target.publications.push(make_publication("pub_b"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.rule == RULE_ID));
    }

    #[test]
    fn partial_overlap_fires_only_for_unmanaged() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.publications.push(make_publication("managed_pub"));
        target.publications.push(make_publication("managed_pub"));
        target.publications.push(make_publication("unmanaged_pub"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("unmanaged_pub"));
    }
}
