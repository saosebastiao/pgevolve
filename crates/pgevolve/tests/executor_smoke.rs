//! Tier-4 smoke tests for the executor.
//!
//! These tests provision an ephemeral Postgres container, run the
//! bootstrap/lock/identity/audit/execute paths, and assert against the
//! `pgevolve.*` audit tables. Skipped when Docker is not available.

use pgevolve_core::catalog::PgVersion;
use pgevolve_testkit::ephemeral_pg::{docker_available, EphemeralPostgres};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_is_idempotent() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(PgVersion::Pg16)
        .await
        .expect("start pg");
    let mut client = pg.connect().await.expect("connect");

    pgevolve::executor::bootstrap_metadata(&mut client)
        .await
        .expect("first bootstrap");
    pgevolve::executor::bootstrap_metadata(&mut client)
        .await
        .expect("second bootstrap is no-op");

    // All four tables exist.
    let row = client
        .query_one(
            "SELECT COUNT(*)::int FROM information_schema.tables \
             WHERE table_schema='pgevolve' \
               AND table_name IN ('bootstrap_version','apply_log','plan_steps','lock')",
            &[],
        )
        .await
        .unwrap();
    let n: i32 = row.get(0);
    assert_eq!(n, 4);

    // Exactly one bootstrap-version row at v1 (idempotent).
    let row = client
        .query_one(
            "SELECT COUNT(*)::int FROM pgevolve.bootstrap_version WHERE version=1",
            &[],
        )
        .await
        .unwrap();
    let n: i32 = row.get(0);
    assert_eq!(n, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn advisory_lock_contention() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(PgVersion::Pg16)
        .await
        .expect("start pg");

    let mut a = pg.connect().await.expect("connect a");
    let b = pg.connect().await.expect("connect b");

    pgevolve::executor::bootstrap_metadata(&mut a)
        .await
        .expect("bootstrap");

    pgevolve::executor::try_acquire_lock(&a, "actor-a")
        .await
        .expect("first acquire");
    let err = pgevolve::executor::try_acquire_lock(&b, "actor-b")
        .await
        .expect_err("second acquire must fail");
    assert!(matches!(err, pgevolve::executor::ApplyError::LockHeld));

    pgevolve::executor::release_lock(&a).await.expect("release");
    // Now b can take it.
    pgevolve::executor::try_acquire_lock(&b, "actor-b")
        .await
        .expect("second-try acquire after release");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_identity_is_stable_across_reconnects() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(PgVersion::Pg16)
        .await
        .expect("start pg");
    let c1 = pg.connect().await.unwrap();
    let c2 = pg.connect().await.unwrap();
    let a = pgevolve::compute_target_identity(&c1).await.unwrap();
    let b = pgevolve::compute_target_identity(&c2).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 16);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_identity_differs_between_distinct_databases() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg_a = EphemeralPostgres::start(PgVersion::Pg16).await.unwrap();
    let pg_b = EphemeralPostgres::start(PgVersion::Pg16).await.unwrap();
    let id_a = pgevolve::compute_target_identity(&pg_a.connect().await.unwrap())
        .await
        .unwrap();
    let id_b = pgevolve::compute_target_identity(&pg_b.connect().await.unwrap())
        .await
        .unwrap();
    assert_ne!(id_a, id_b);
}
