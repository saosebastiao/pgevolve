//! Subscription generators (v0.3.5).
//!
//! Publication names are drawn from the catalog's actual publications so
//! generated subscriptions always reference real publications. CREATE-only
//! fields (`connect`, `create_slot`, `copy_data`) and PG-version-gated fields
//! (`password_required`, `run_as_owner`, `disable_on_error` (PG 15+),
//! `two_phase` (PG 15+), `origin` (PG 16+), `failover` (PG 17+)) are left at
//! fixed values or `None` to keep generation simple and lint-clean.
//!
//! `connect` is always `Some(false)` so that generated subscriptions with
//! synthetic DSNs (e.g. `replica.example.com`) never trigger a network dial
//! at `CREATE SUBSCRIPTION` time (PG dials the publisher by default).

#![allow(clippy::needless_pass_by_value)]

use proptest::prelude::*;

use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::subscription::{StreamingMode, Subscription, SubscriptionOptions};

/// Small fixed pool of subscription names (SQL-safe, short, distinct).
const SUB_NAMES: &[&str] = &["sub_a", "sub_b", "sub_c"];

/// Generate a random [`StreamingMode`] accepted by all supported PG versions.
///
/// `Parallel` is deliberately excluded: the `parallel` string form was
/// introduced in PG 16; PG ≤15 only accepts `true` / `false` for the
/// `streaming` option. The testkit targets PG 14–18, so it must stay within
/// the common subset. Coverage of `Parallel` is provided by hand-crafted
/// conformance fixtures that target PG 16+ explicitly.
fn arb_streaming_mode() -> impl Strategy<Value = StreamingMode> {
    prop_oneof![Just(StreamingMode::Off), Just(StreamingMode::On),]
}

/// Generate random [`SubscriptionOptions`] with selected fields set.
///
/// `connect` is always `Some(false)` — generated subscriptions use synthetic
/// DSNs that must never be dialed at CREATE time.
///
/// Other CREATE-only fields (`create_slot`, `copy_data`) and PG-version-gated
/// fields (`password_required`, `run_as_owner`, `disable_on_error` (PG 15+),
/// `two_phase` (PG 15+), `origin` (PG 16+), `failover` (PG 17+)) are left
/// `None` to keep generation simple and lint-clean. `synchronous_commit` is
/// also left `None` (free-form string; no bounded pool to sample from).
fn arb_subscription_options() -> impl Strategy<Value = SubscriptionOptions> {
    (
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // enabled
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // binary
        prop_oneof![Just(None), arb_streaming_mode().prop_map(Some)], // streaming
        Just(None), // two_phase — PG 15+ only; leave None to stay version-agnostic
        Just(None), // disable_on_error — PG 15+ only; leave None to stay version-agnostic
        Just(None), // origin — PG 16+ only; leave None to stay version-agnostic
        Just(None), // failover — PG 17+ only; leave None to stay version-agnostic
    )
        .prop_map(
            |(enabled, binary, streaming, two_phase, disable_on_error, origin, failover)| {
                SubscriptionOptions {
                    enabled,
                    slot_name: None,
                    connect: Some(false), // always false — never dial synthetic DSNs
                    create_slot: None,
                    copy_data: None,
                    synchronous_commit: None,
                    binary,
                    streaming,
                    two_phase,
                    disable_on_error,
                    password_required: None,
                    run_as_owner: None,
                    origin,
                    failover,
                }
            },
        )
}

/// Generate a single [`Subscription`] with a name from `sub_name_idx` into
/// `SUB_NAMES`, 1–3 publications drawn from `publication_pool`, and random
/// options. Connection string uses a `${TEST_PWD}` placeholder so it lints
/// clean (`subscription-password-in-source` only fires on plaintext values).
fn arb_subscription_inner(
    sub_name_idx: usize,
    publication_pool: Vec<Identifier>,
) -> impl Strategy<Value = Subscription> {
    let name = Identifier::from_unquoted(SUB_NAMES[sub_name_idx]).unwrap();
    // Pick 1–3 publications from the pool; fall back to a synthetic name when
    // the pool is empty so the strategy always produces a valid Subscription.
    let pubs_strategy: BoxedStrategy<Vec<Identifier>> = if publication_pool.is_empty() {
        Just(vec![Identifier::from_unquoted("pub_a").unwrap()]).boxed()
    } else {
        let max_pick = 3usize.min(publication_pool.len());
        proptest::sample::subsequence(publication_pool, 1..=max_pick)
            .prop_map(|mut v| {
                v.sort();
                v.dedup();
                v
            })
            .boxed()
    };
    (pubs_strategy, arb_subscription_options()).prop_map(move |(publications, options)| {
        Subscription {
            name: name.clone(),
            // Synthetic connection string with a ${VAR} placeholder for the
            // password. The strategy doesn't vary the connection text — every
            // generated subscription uses a benign placeholder that lints clean
            // (subscription-password-in-source fires only on plaintext values).
            connection: "host=replica.example.com dbname=app user=repl password=${TEST_PWD}"
                .to_string(),
            publications,
            options,
            owner: None,
            comment: None,
        }
    })
}

/// Generate 0–1 [`Subscription`]s with names drawn from `SUB_NAMES`.
///
/// 0–1 (not 0–2+) keeps the generated catalog lightweight and avoids
/// subscription-name collisions with the small `SUB_NAMES` pool when the
/// publication pool is already non-empty.
pub(super) fn arb_subscriptions(
    publication_pool: Vec<Identifier>,
) -> BoxedStrategy<Vec<Subscription>> {
    (0usize..=1usize)
        .prop_flat_map(move |count| {
            let pp = publication_pool.clone();
            proptest::sample::subsequence((0..SUB_NAMES.len()).collect::<Vec<_>>(), count..=count)
                .prop_flat_map(move |indices| {
                    let pp = pp.clone();
                    let strategies: Vec<_> = indices
                        .into_iter()
                        .map(|idx| arb_subscription_inner(idx, pp.clone()))
                        .collect();
                    strategies
                })
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    use super::arb_subscription_options;

    use pgevolve_core::ir::subscription::StreamingMode;

    /// `connect` must always be `Some(false)` in generated subscriptions.
    /// Generated subscriptions use synthetic DSNs (e.g. `replica.example.com`)
    /// that cannot be dialed; `connect = false` prevents PG from attempting a
    /// network connection at `CREATE SUBSCRIPTION` time.
    #[test]
    fn subscription_options_connect_is_always_some_false() {
        let mut runner = TestRunner::default();
        for _ in 0..256 {
            let tree = arb_subscription_options()
                .new_tree(&mut runner)
                .expect("strategy construction failed");
            let opts = tree.current();
            assert_eq!(
                opts.connect,
                Some(false),
                "connect must always be Some(false) for synthetic-DSN safety, got {:?}",
                opts.connect,
            );
        }
    }

    /// `failover` (PG 17+), `origin` (PG 16+), `two_phase` (PG 15+), and
    /// `disable_on_error` (PG 15+) must always be `None` so that generated
    /// subscriptions are valid on every PG version the soak matrix covers
    /// (PG 14–18).
    #[test]
    fn subscription_options_version_gated_fields_are_none() {
        let mut runner = TestRunner::default();
        for _ in 0..256 {
            let tree = arb_subscription_options()
                .new_tree(&mut runner)
                .expect("strategy construction failed");
            let opts = tree.current();
            assert!(
                opts.failover.is_none(),
                "failover must be None (PG 17+ only), got {:?}",
                opts.failover,
            );
            assert!(
                opts.origin.is_none(),
                "origin must be None (PG 16+ only), got {:?}",
                opts.origin,
            );
            assert!(
                opts.two_phase.is_none(),
                "two_phase must be None (PG 15+ only), got {:?}",
                opts.two_phase,
            );
            assert!(
                opts.disable_on_error.is_none(),
                "disable_on_error must be None (PG 15+ only), got {:?}",
                opts.disable_on_error,
            );
        }
    }

    /// `streaming = parallel` is only valid on PG 16+. The testkit generator
    /// must never emit `StreamingMode::Parallel` so that generated catalogs
    /// are valid across the full PG 14–18 support window.
    ///
    /// Coverage gap deliberately accepted: `Parallel` is not exercise by the
    /// proptest soak; it is covered by hand-crafted conformance fixtures
    /// targeting PG 16+.
    #[test]
    fn streaming_is_never_parallel() {
        let mut runner = TestRunner::default();
        for _ in 0..256 {
            let tree = arb_subscription_options()
                .new_tree(&mut runner)
                .expect("strategy construction failed");
            let opts = tree.current();
            assert!(
                opts.streaming != Some(StreamingMode::Parallel),
                "streaming must never be Parallel (PG 16+ only), got {:?}",
                opts.streaming,
            );
        }
    }
}
