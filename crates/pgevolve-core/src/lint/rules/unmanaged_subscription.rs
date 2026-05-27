//! Warns when the catalog has a subscription not declared in source.
//!
//! Per the lenient drift policy, catalog subscriptions that don't appear
//! in source are NOT dropped by the differ. This lint surfaces them so
//! operators can decide whether to bring under management or accept the drift.
//!
//! Mirrors `unmanaged_publication`.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-subscription";

/// Fires once per target subscription whose name is not in source's subscriptions list.
pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    target
        .subscriptions
        .iter()
        .filter(|tgt_sub| {
            !source
                .subscriptions
                .iter()
                .any(|src_sub| src_sub.name == tgt_sub.name)
        })
        .map(|tgt_sub| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!(
                "subscription {}: catalog has a subscription not declared in source",
                tgt_sub.name,
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
    use crate::ir::subscription::{Subscription, SubscriptionOptions};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn make_subscription(name: &str) -> Subscription {
        Subscription {
            name: id(name),
            connection: "host=x dbname=app".into(),
            publications: vec![id("p")],
            options: SubscriptionOptions::default(),
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
    fn subscription_in_source_and_target_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.subscriptions.push(make_subscription("my_sub"));
        target.subscriptions.push(make_subscription("my_sub"));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn subscription_only_in_target_fires() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target
            .subscriptions
            .push(make_subscription("unmanaged_sub"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("unmanaged_sub"),
            "message should mention the subscription name: {}",
            findings[0].message
        );
    }

    #[test]
    fn subscription_only_in_source_silent() {
        let mut source = Catalog::empty();
        let target = Catalog::empty();
        source.subscriptions.push(make_subscription("managed_sub"));
        // Source-only: not-yet-created; no drift finding needed.
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn source_has_one_target_has_two_fires_for_extra() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.subscriptions.push(make_subscription("sub_s"));
        target.subscriptions.push(make_subscription("sub_s"));
        target.subscriptions.push(make_subscription("sub_t"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("sub_t"));
    }

    #[test]
    fn multiple_unmanaged_subscriptions_each_fire() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target.subscriptions.push(make_subscription("sub_a"));
        target.subscriptions.push(make_subscription("sub_b"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.rule == RULE_ID));
    }
}
