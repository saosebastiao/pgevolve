//! Errors when source uses a PG-version-gated subscription option but
//! `min_pg_version` is below the required version.
//!
//! Gated features and their minimum PG versions:
//! - `streaming = Parallel` — requires PG 16+
//! - `disable_on_error` — requires PG 15+
//! - `password_required` — requires PG 16+
//! - `run_as_owner` — requires PG 16+
//! - `origin` — requires PG 16+
//! - `failover` — requires PG 17+
//!
//! Note: `two_phase` is PG 14+, and pgevolve's minimum is PG 14, so that
//! option never fires in practice.
//!
//! Source-only rule; fires `Severity::Error` (not waivable — using PG-gated
//! syntax on a project declaring a lower min version is genuine misconfiguration).

use crate::ir::catalog::Catalog;
use crate::ir::subscription::StreamingMode;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "subscription-feature-requires-pg-version";

/// Check source subscriptions against `min_pg_version`.
pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    let mut findings = Vec::new();
    for s in &source.subscriptions {
        if matches!(s.options.streaming, Some(StreamingMode::Parallel)) && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "streaming = parallel", 16));
        }
        if s.options.disable_on_error.is_some() && min_pg_version < 15 {
            findings.push(fire(s.name.as_str(), "disable_on_error", 15));
        }
        if s.options.password_required.is_some() && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "password_required", 16));
        }
        if s.options.run_as_owner.is_some() && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "run_as_owner", 16));
        }
        if s.options.origin.is_some() && min_pg_version < 16 {
            findings.push(fire(s.name.as_str(), "origin", 16));
        }
        if s.options.failover.is_some() && min_pg_version < 17 {
            findings.push(fire(s.name.as_str(), "failover", 17));
        }
    }
    findings
}

fn fire(sub_name: &str, feature: &str, required: u32) -> Finding {
    Finding {
        rule: RULE_ID,
        severity: Severity::Error,
        message: format!(
            "subscription {sub_name}: option `{feature}` requires PG {required}+; \
             raise [managed].min_pg_version to {required} or remove the option",
        ),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::subscription::{OriginMode, StreamingMode, Subscription, SubscriptionOptions};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn make_sub(name: &str, options: SubscriptionOptions) -> Subscription {
        Subscription {
            name: id(name),
            connection: "host=x dbname=app".into(),
            publications: vec![id("p")],
            options,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn no_subscriptions_always_silent() {
        assert!(check(&Catalog::empty(), 14).is_empty());
    }

    #[test]
    fn streaming_parallel_fires_below_pg16() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                streaming: Some(StreamingMode::Parallel),
                ..Default::default()
            },
        ));
        // PG 14 — should fire.
        let findings = check(&source, 14);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("streaming = parallel"));
        // PG 15 — still below 16, should fire.
        assert_eq!(check(&source, 15).len(), 1);
        // PG 16 — at minimum, should not fire.
        assert!(check(&source, 16).is_empty());
        // PG 17 — above minimum, should not fire.
        assert!(check(&source, 17).is_empty());
    }

    #[test]
    fn streaming_off_or_on_silent_on_pg14() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                streaming: Some(StreamingMode::Off),
                ..Default::default()
            },
        ));
        assert!(check(&source, 14).is_empty());
        source.subscriptions[0].options.streaming = Some(StreamingMode::On);
        assert!(check(&source, 14).is_empty());
    }

    #[test]
    fn disable_on_error_fires_below_pg15() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                disable_on_error: Some(true),
                ..Default::default()
            },
        ));
        assert_eq!(check(&source, 14).len(), 1);
        assert!(
            check(&source, 14)
                .iter()
                .any(|f| f.message.contains("disable_on_error"))
        );
        // PG 15 — at minimum, silent.
        assert!(check(&source, 15).is_empty());
    }

    #[test]
    fn password_required_fires_below_pg16() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                password_required: Some(true),
                ..Default::default()
            },
        ));
        assert_eq!(check(&source, 15).len(), 1);
        assert!(check(&source, 16).is_empty());
    }

    #[test]
    fn run_as_owner_fires_below_pg16() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                run_as_owner: Some(false),
                ..Default::default()
            },
        ));
        assert_eq!(check(&source, 15).len(), 1);
        assert!(check(&source, 16).is_empty());
    }

    #[test]
    fn origin_fires_below_pg16() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                origin: Some(OriginMode::Any),
                ..Default::default()
            },
        ));
        assert_eq!(check(&source, 15).len(), 1);
        assert!(check(&source, 16).is_empty());
    }

    #[test]
    fn failover_fires_below_pg17() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                failover: Some(true),
                ..Default::default()
            },
        ));
        assert_eq!(check(&source, 16).len(), 1);
        assert!(
            check(&source, 16)
                .iter()
                .any(|f| f.message.contains("failover"))
        );
        assert!(check(&source, 17).is_empty());
    }

    #[test]
    fn multiple_gated_options_each_fire() {
        let mut source = Catalog::empty();
        // Use options from PG15, PG16, and PG17 simultaneously, with min=14.
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                disable_on_error: Some(true),  // requires 15
                password_required: Some(true), // requires 16
                failover: Some(true),          // requires 17
                ..Default::default()
            },
        ));
        let findings = check(&source, 14);
        assert_eq!(findings.len(), 3);
        assert!(findings.iter().all(|f| f.severity == Severity::Error));
    }

    #[test]
    fn all_silent_at_pg17() {
        let mut source = Catalog::empty();
        source.subscriptions.push(make_sub(
            "s",
            SubscriptionOptions {
                streaming: Some(StreamingMode::Parallel),
                disable_on_error: Some(true),
                password_required: Some(true),
                run_as_owner: Some(true),
                origin: Some(OriginMode::None),
                failover: Some(true),
                ..Default::default()
            },
        ));
        // At PG17, nothing should fire.
        assert!(check(&source, 17).is_empty());
    }
}
