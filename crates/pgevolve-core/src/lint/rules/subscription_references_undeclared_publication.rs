//! Warns when a source subscription's PUBLICATION list references a name
//! with no matching `Publication` in source.
//!
//! Subscriptions are cross-cluster by nature — the publications they reference
//! live on a remote publisher. This rule fires on local-source typos where a
//! subscription names a publication that isn't declared in the same source tree.
//! Operators publishing from this same cluster will benefit from the check;
//! operators subscribing to an external cluster may waive this rule.
//!
//! Source-only rule (no catalog / target needed).

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "subscription-references-undeclared-publication";

/// For each source subscription, checks that every name in its `publications`
/// list exists as a `Publication.name` in source.
pub fn check(source: &Catalog) -> Vec<Finding> {
    let pub_names: std::collections::BTreeSet<_> =
        source.publications.iter().map(|p| &p.name).collect();
    source
        .subscriptions
        .iter()
        .flat_map(|s| {
            s.publications
                .iter()
                .filter(|p| !pub_names.contains(p))
                .map(move |p| Finding {
                    rule: RULE_ID,
                    severity: Severity::Warning,
                    message: format!(
                        "subscription {} references undeclared publication {}",
                        s.name, p,
                    ),
                    location: None,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::publication::{Publication, PublicationScope, PublishKinds};
    use crate::ir::subscription::{Subscription, SubscriptionOptions};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn make_subscription(name: &str, pubs: &[&str]) -> Subscription {
        Subscription {
            name: id(name),
            connection: "host=x dbname=app".into(),
            publications: pubs.iter().map(|p| id(p)).collect(),
            options: SubscriptionOptions::default(),
            owner: None,
            comment: None,
        }
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
    fn no_subscriptions_silent() {
        let source = Catalog::empty();
        assert!(check(&source).is_empty());
    }

    #[test]
    fn subscription_references_declared_publication_silent() {
        let mut source = Catalog::empty();
        source.publications.push(make_publication("my_pub"));
        source
            .subscriptions
            .push(make_subscription("sub", &["my_pub"]));
        assert!(check(&source).is_empty());
    }

    #[test]
    fn subscription_references_undeclared_publication_fires() {
        let mut source = Catalog::empty();
        // No publications declared.
        source
            .subscriptions
            .push(make_subscription("sub", &["typo_pub"]));
        let findings = check(&source);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("typo_pub"),
            "message should name the undeclared publication: {}",
            findings[0].message
        );
        assert!(
            findings[0].message.contains("sub"),
            "message should name the subscription: {}",
            findings[0].message
        );
    }

    #[test]
    fn partial_overlap_fires_only_for_undeclared() {
        let mut source = Catalog::empty();
        source.publications.push(make_publication("pub_good"));
        // pub_missing is not declared.
        source
            .subscriptions
            .push(make_subscription("sub", &["pub_good", "pub_missing"]));
        let findings = check(&source);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("pub_missing"));
    }

    #[test]
    fn multiple_subscriptions_with_undeclared_each_fire() {
        let mut source = Catalog::empty();
        source
            .subscriptions
            .push(make_subscription("sub_a", &["unknown_pub_a"]));
        source
            .subscriptions
            .push(make_subscription("sub_b", &["unknown_pub_b"]));
        let findings = check(&source);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.rule == RULE_ID));
    }

    #[test]
    fn multiple_missing_pubs_in_one_subscription_each_fire() {
        let mut source = Catalog::empty();
        source
            .subscriptions
            .push(make_subscription("sub", &["p1", "p2", "p3"]));
        let findings = check(&source);
        assert_eq!(findings.len(), 3);
    }
}
