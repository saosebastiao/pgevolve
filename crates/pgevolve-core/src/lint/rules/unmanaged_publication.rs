//! Warns when the catalog has a publication not declared in source.
//!
//! See [`super::check_unmanaged_objects`] for the shared lenient-drift policy.

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "unmanaged-publication";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    super::check_unmanaged_objects(
        &target.publications,
        &source.publications,
        |p| &p.name,
        RULE_ID,
        "publication",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::publication::{Publication, PublicationScope, PublishKinds};
    use crate::lint::finding::Severity;

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
        assert!(check(&source, &target).is_empty());
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
