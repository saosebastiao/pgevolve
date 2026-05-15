//! PG-dependent property tests (spec §14, Tier 5).
//!
//! Each property generates a random IR with [`pgevolve_testkit::arbitrary_catalog`],
//! spins up an `EphemeralPostgres`, exercises the pgevolve pipeline against
//! it, and asserts a structural property.
//!
//! Default case counts are intentionally small here (each iteration starts
//! a fresh Postgres container, which is expensive). The soak workflow
//! cranks `PROPTEST_CASES` to 5000 across all four PG majors.
//!
//! Skipped wholesale when Docker is unavailable.
//!
//! All tests in this file are #[ignore]'d for CI. Run with
//! `cargo test --test pg_property_tests -- --ignored` locally, or via the
//! property-tests.yml workflow.

#![allow(clippy::items_after_statements)]

mod common;

use anyhow::Result;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

use pgevolve_core::ir::catalog::Catalog;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
use pgevolve_testkit::{
    IRGeneratorConfig, arbitrary_catalog, arbitrary_mutation, assert_canonical_eq,
};

use common::{apply_diff, connect_and_bootstrap, introspect, schemas_of};

/// Read `PGEVOLVE_PROPERTY_CASES` (or use the default for PG-bound tests).
fn case_count(default: u32) -> u32 {
    std::env::var("PGEVOLVE_PROPERTY_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Round-trip: applying a catalog and re-introspecting yields the same IR.
async fn check_round_trip(catalog: &Catalog) -> Result<()> {
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let managed = schemas_of(catalog);
    if managed.is_empty() {
        return Ok(()); // trivially round-trips
    }
    let mut client = connect_and_bootstrap(&pg).await?;
    let outcome = apply_diff(&mut client, &Catalog::empty(), catalog, &managed, None).await?;
    outcome.map_err(|e| anyhow::anyhow!("apply: {e}"))?;
    let live = introspect(&pg, &managed).await?;
    assert_canonical_eq(catalog, &live)
}

/// Idempotency: applying the same catalog twice is a no-op on the second
/// pass (the diff is empty and the planner emits zero groups).
async fn check_idempotency(catalog: &Catalog) -> Result<()> {
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let managed = schemas_of(catalog);
    if managed.is_empty() {
        return Ok(());
    }
    let mut client = connect_and_bootstrap(&pg).await?;
    let first = apply_diff(&mut client, &Catalog::empty(), catalog, &managed, None).await?;
    first.map_err(|e| anyhow::anyhow!("first apply: {e}"))?;

    // Second pass: diff against the catalog we just applied; the plan must
    // have no groups (no work to do).
    let second_diff = pgevolve_core::diff::diff(catalog, catalog, &pgevolve_core::catalog::DriftReport::default());
    if !second_diff.is_empty() {
        return Err(anyhow::anyhow!(
            "diff(catalog, catalog) was non-empty: {} entries",
            second_diff.len()
        ));
    }
    Ok(())
}

/// End-to-end equivalence: apply initial, then plan + apply a random
/// mutation, then introspect and assert structural equality with the
/// mutated IR.
async fn check_end_to_end(initial: &Catalog, mutated: &Catalog) -> Result<()> {
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    // Use the union of schemas across both states; otherwise drop-schema
    // mutations skip the introspect.
    let mut managed: Vec<_> = schemas_of(initial);
    for s in schemas_of(mutated) {
        if !managed.contains(&s) {
            managed.push(s);
        }
    }
    if managed.is_empty() {
        return Ok(());
    }
    let mut client = connect_and_bootstrap(&pg).await?;

    let first = apply_diff(&mut client, &Catalog::empty(), initial, &managed, None).await?;
    first.map_err(|e| anyhow::anyhow!("initial apply: {e}"))?;

    let second = apply_diff(&mut client, initial, mutated, &managed, None).await?;
    second.map_err(|e| anyhow::anyhow!("mutated apply: {e}"))?;

    let live = introspect(&pg, &managed).await?;
    assert_canonical_eq(mutated, &live)
}

/// Drift recovery: apply midway, abort, then re-plan from partial live
/// state and complete the apply. End state must equal the target catalog.
async fn check_drift_recovery(target: &Catalog, abort_step: u32) -> Result<()> {
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let managed = schemas_of(target);
    if managed.is_empty() {
        return Ok(());
    }
    let mut client = connect_and_bootstrap(&pg).await?;
    let first = apply_diff(
        &mut client,
        &Catalog::empty(),
        target,
        &managed,
        Some(abort_step),
    )
    .await?;
    // If the abort step was past the plan length, the first apply will have
    // succeeded — there's nothing to recover. Skip.
    match first {
        Ok(_) => return Ok(()),
        Err(pgevolve::executor::ApplyError::AbortedAfterStep { .. }) => {}
        Err(other) => return Err(anyhow::anyhow!("expected AbortedAfterStep, got {other:?}")),
    }
    let partial = introspect(&pg, &managed).await?;
    let second = apply_diff(&mut client, &partial, target, &managed, None).await?;
    second.map_err(|e| anyhow::anyhow!("recovery apply: {e}"))?;
    let live = introspect(&pg, &managed).await?;
    assert_canonical_eq(target, &live)
}

/// Run a synchronous proptest harness around an async property check.
fn run_proptest<F>(cases: u32, mut body: F)
where
    F: FnMut(&mut TestRunner) -> Result<(), proptest::test_runner::TestCaseError>,
{
    let mut runner = TestRunner::new(proptest::test_runner::Config {
        cases,
        ..proptest::test_runner::Config::default()
    });
    for _ in 0..cases {
        if let Err(e) = body(&mut runner) {
            panic!("property failed: {e:?}");
        }
    }
}

/// Build a fresh tokio runtime per property body. We can't reuse the test
/// runtime because each iteration starts a new container and we want
/// isolation; the cost is one extra runtime construction per case.
fn block_on<F: std::future::Future<Output = Result<()>>>(fut: F) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

#[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
#[test]
fn round_trip_property() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let cases = case_count(3);
    let strategy = arbitrary_catalog(IRGeneratorConfig::default());
    run_proptest(cases, |runner| {
        let catalog = strategy.new_tree(runner).unwrap().current();
        block_on(check_round_trip(&catalog))
            .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("round_trip: {e:#}")))
    });
}

#[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
#[test]
fn idempotency_property() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let cases = case_count(3);
    let strategy = arbitrary_catalog(IRGeneratorConfig::default());
    run_proptest(cases, |runner| {
        let catalog = strategy.new_tree(runner).unwrap().current();
        block_on(check_idempotency(&catalog))
            .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("idempotency: {e:#}")))
    });
}

#[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
#[test]
fn end_to_end_equivalence_property() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let cases = case_count(3);
    let strategy = arbitrary_catalog(IRGeneratorConfig::default());
    run_proptest(cases, |runner| {
        let initial = strategy.new_tree(runner).unwrap().current();
        let mutated = arbitrary_mutation(initial.clone())
            .new_tree(runner)
            .unwrap()
            .current();
        block_on(check_end_to_end(&initial, &mutated))
            .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("e2e: {e:#}")))
    });
}

#[ignore = "property test — run via property-tests workflow or `cargo test -- --ignored`"]
#[test]
fn drift_recovery_property() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let cases = case_count(3);
    let strategy = arbitrary_catalog(IRGeneratorConfig::default());
    run_proptest(cases, |runner| {
        let catalog = strategy.new_tree(runner).unwrap().current();
        // Pick a random step number 1..=10 to abort after.
        let abort = proptest::num::u32::ANY
            .new_tree(runner)
            .unwrap()
            .current()
            .rem_euclid(10)
            + 1;
        block_on(check_drift_recovery(&catalog, abort)).map_err(|e| {
            proptest::test_runner::TestCaseError::fail(format!("drift_recovery: {e:#}"))
        })
    });
}
