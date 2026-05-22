//! Tier-4 smoke tests for the executor.
//!
//! These tests provision an ephemeral Postgres container, run the
//! bootstrap/lock/identity/audit/execute paths, and assert against the
//! `pgevolve.*` audit tables. Skipped when Docker is not available.

// Per-fixture helpers below intentionally mix imports with statements
// (each test owns its own use-list); these clippy lints fight the test
// readability and are silenced at file scope.
#![allow(clippy::items_after_statements)]
#![allow(clippy::similar_names)]

use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_is_idempotent() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version())
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
    let pg = EphemeralPostgres::start(default_pg_version())
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
    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start pg");
    let c1 = pg.connect().await.unwrap();
    let c2 = pg.connect().await.unwrap();
    let a = pgevolve::compute_target_identity(&c1).await.unwrap();
    let b = pgevolve::compute_target_identity(&c2).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 16);
}

/// Build an in-memory `Plan` with one transactional group containing two
/// safe steps (CREATE SCHEMA / CREATE TABLE), write it to `dir`, and return
/// the constructed `Plan` for assertion.
fn build_demo_plan(dir: &std::path::Path, target_identity: &str) -> pgevolve_core::plan::Plan {
    use pgevolve_core::identifier::{Identifier, QualifiedName};
    use pgevolve_core::ir::catalog::Catalog;
    use pgevolve_core::ir::schema::Schema;
    use pgevolve_core::plan::Plan;
    use pgevolve_core::plan::grouping::TransactionGroup;
    use pgevolve_core::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    let id = |s: &str| Identifier::from_unquoted(s).unwrap();
    let qn = |sch: &str, n: &str| QualifiedName::new(id(sch), id(n));

    let mut source = Catalog::empty();
    source.schemas.push(Schema::new(id("demo")));
    // Target snapshot: empty (the schema doesn't exist yet).
    let target = Catalog::empty();

    let groups = vec![TransactionGroup {
        id: 1,
        transactional: true,
        steps: vec![
            RawStep {
                step_no: 0,
                kind: StepKind::CreateSchema,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qn("demo", "demo")],
                sql: "CREATE SCHEMA demo;".into(),
                transactional: TransactionConstraint::InTransaction,
            },
            RawStep {
                step_no: 0,
                kind: StepKind::CreateTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qn("demo", "widgets")],
                sql: "CREATE TABLE demo.widgets (id bigint NOT NULL);".into(),
                transactional: TransactionConstraint::InTransaction,
            },
        ],
    }];
    let plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        target_identity.to_string(),
        Some("git:test".into()),
        "0.1.0",
        1,
    )
    .expect("from_grouped");
    pgevolve_core::plan::serialize::write_plan_dir(&plan, dir).expect("write plan dir");
    plan
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_succeeds_end_to_end_and_persists_audit_rows() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .unwrap();
    let mut client = pg.connect().await.unwrap();

    let identity = pgevolve::compute_target_identity(&client).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _plan = build_demo_plan(dir.path(), &identity);

    let filter = pgevolve_core::catalog::CatalogFilter::new(
        vec![pgevolve_core::identifier::Identifier::from_unquoted("demo").unwrap()],
        vec![],
    )
    .unwrap();
    let outcome = pgevolve::apply(
        dir.path(),
        &mut client,
        &filter,
        pgevolve::executor::ApplyOverrides {
            allow_drift: true, // drift checker is a stub in v0.1
            ..Default::default()
        },
    )
    .await
    .expect("apply ok");

    let apply_id = match outcome {
        pgevolve::executor::ApplyOutcome::Succeeded { apply_id } => apply_id,
    };

    // The schema and table now exist.
    let row = client
        .query_one("SELECT to_regclass('demo.widgets')::text IS NOT NULL", &[])
        .await
        .unwrap();
    let exists: bool = row.get(0);
    assert!(exists);

    // Audit rows reflect success.
    let row = client
        .query_one(
            "SELECT status FROM pgevolve.apply_log WHERE apply_id=$1",
            &[&apply_id],
        )
        .await
        .unwrap();
    let status: String = row.get(0);
    assert_eq!(status, "succeeded");

    let row = client
        .query_one(
            "SELECT COUNT(*)::int FROM pgevolve.plan_steps \
             WHERE apply_id=$1 AND status='succeeded'",
            &[&apply_id],
        )
        .await
        .unwrap();
    let n: i32 = row.get(0);
    assert_eq!(n, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
// The rollback-on-failure scenario requires setting up multi-step groups,
// injecting a deliberate SQL error, running apply, and then asserting both
// the schema state and the audit log. There is no natural extraction point
// that would reduce the line count without obscuring the test flow.
#[allow(clippy::too_many_lines)]
async fn apply_rolls_back_transactional_group_on_failure() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    use pgevolve_core::identifier::QualifiedName;
    use pgevolve_core::ir::catalog::Catalog;
    use pgevolve_core::plan::Plan;
    use pgevolve_core::plan::grouping::TransactionGroup;
    use pgevolve_core::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .unwrap();
    let mut client = pg.connect().await.unwrap();
    let identity = pgevolve::compute_target_identity(&client).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    // Build a plan whose step 2 fails (intentional SQL error).
    let id = |s: &str| pgevolve_core::identifier::Identifier::from_unquoted(s).unwrap();
    let groups = vec![TransactionGroup {
        id: 1,
        transactional: true,
        steps: vec![
            RawStep {
                step_no: 0,
                kind: StepKind::CreateSchema,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![QualifiedName::new(id("demo"), id("demo"))],
                sql: "CREATE SCHEMA demo;".into(),
                transactional: TransactionConstraint::InTransaction,
            },
            RawStep {
                step_no: 0,
                kind: StepKind::CreateTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![QualifiedName::new(id("demo"), id("widgets"))],
                sql: "CREATE TABLE demo.widgets (id NOT_A_REAL_TYPE);".into(),
                transactional: TransactionConstraint::InTransaction,
            },
        ],
    }];
    let plan = Plan::from_grouped(
        groups,
        &Catalog::empty(),
        &Catalog::empty(),
        identity,
        None,
        "0.1.0",
        1,
    )
    .unwrap();
    pgevolve_core::plan::serialize::write_plan_dir(&plan, dir.path()).unwrap();

    let filter = pgevolve_core::catalog::CatalogFilter::new(vec![id("demo")], vec![]).unwrap();
    let err = pgevolve::apply(
        dir.path(),
        &mut client,
        &filter,
        pgevolve::executor::ApplyOverrides {
            allow_drift: true,
            ..Default::default()
        },
    )
    .await
    .expect_err("apply must fail");
    assert!(matches!(
        err,
        pgevolve::executor::ApplyError::StepFailed { .. }
    ));

    // The schema was rolled back: no `demo` schema in pg_namespace.
    let row = client
        .query_one(
            "SELECT COUNT(*)::int FROM pg_namespace WHERE nspname='demo'",
            &[],
        )
        .await
        .unwrap();
    let n: i32 = row.get(0);
    assert_eq!(n, 0);

    // Audit recorded the failure: apply_log is `failed`, both step rows ended in
    // `failed` (step 2) or `rolled_back` (step 1).
    let row = client
        .query_one(
            "SELECT status FROM pgevolve.apply_log ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    let status: String = row.get(0);
    assert_eq!(status, "failed");

    let rows = client
        .query(
            "SELECT step_no, status FROM pgevolve.plan_steps \
             WHERE apply_id=(SELECT apply_id FROM pgevolve.apply_log ORDER BY started_at DESC LIMIT 1) \
             ORDER BY step_no",
            &[],
        )
        .await
        .unwrap();
    let states: Vec<(i32, String)> = rows.iter().map(|r| (r.get(0), r.get(1))).collect();
    assert_eq!(
        states,
        vec![(1, "rolled_back".into()), (2, "failed".into())]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_rejects_target_identity_mismatch() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .unwrap();
    let mut client = pg.connect().await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let _plan = build_demo_plan(dir.path(), "not-the-real-identity");

    let filter = pgevolve_core::catalog::CatalogFilter::new(
        vec![pgevolve_core::identifier::Identifier::from_unquoted("demo").unwrap()],
        vec![],
    )
    .unwrap();
    let err = pgevolve::apply(
        dir.path(),
        &mut client,
        &filter,
        pgevolve::executor::ApplyOverrides::default(),
    )
    .await
    .expect_err("apply must reject mismatched target");
    assert!(matches!(
        err,
        pgevolve::executor::ApplyError::TargetIdentityMismatch { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_queries_return_recent_apply_with_steps() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .unwrap();
    let mut client = pg.connect().await.unwrap();
    let identity = pgevolve::compute_target_identity(&client).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let _plan = build_demo_plan(dir.path(), &identity);

    let filter = pgevolve_core::catalog::CatalogFilter::new(
        vec![pgevolve_core::identifier::Identifier::from_unquoted("demo").unwrap()],
        vec![],
    )
    .unwrap();
    pgevolve::apply(
        dir.path(),
        &mut client,
        &filter,
        pgevolve::executor::ApplyOverrides {
            allow_drift: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let recent = pgevolve::executor::status::fetch_recent_applies(&client, 10)
        .await
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].status, "succeeded");

    let steps = pgevolve::executor::status::fetch_apply_steps(&client, recent[0].apply_id)
        .await
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert!(steps.iter().all(|s| s.status == "succeeded"));

    let human = pgevolve::executor::status::format_status_human(&recent[0], &steps);
    assert!(human.contains("status=succeeded"));
    assert!(human.contains("create_schema"));
    assert!(human.contains("create_table"));

    let json = pgevolve::executor::status::format_status_json(&recent[0], &steps).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["apply"]["status"], "succeeded");
    assert_eq!(v["steps"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_identity_differs_between_distinct_databases() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg_a = EphemeralPostgres::start(default_pg_version())
        .await
        .unwrap();
    let pg_b = EphemeralPostgres::start(default_pg_version())
        .await
        .unwrap();
    let id_a = pgevolve::compute_target_identity(&pg_a.connect().await.unwrap())
        .await
        .unwrap();
    let id_b = pgevolve::compute_target_identity(&pg_b.connect().await.unwrap())
        .await
        .unwrap();
    assert_ne!(id_a, id_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // integration test; inline fixture by design
async fn refresh_mv_suppressed_by_step_override() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    use pgevolve_core::identifier::{Identifier, QualifiedName};
    use pgevolve_core::ir::catalog::Catalog;
    use pgevolve_core::plan::Plan;
    use pgevolve_core::plan::StepOverride;
    use pgevolve_core::plan::grouping::TransactionGroup;
    use pgevolve_core::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .unwrap();
    let mut client = pg.connect().await.unwrap();
    let identity = pgevolve::compute_target_identity(&client).await.unwrap();

    let id = |s: &str| Identifier::from_unquoted(s).unwrap();
    let qn = |sch: &str, n: &str| QualifiedName::new(id(sch), id(n));

    // Build a plan with two steps:
    // 1. CREATE MATERIALIZED VIEW app.mv_revenue WITH NO DATA
    // 2. REFRESH MATERIALIZED VIEW app.mv_revenue  (suppressed by step_override)
    let groups = vec![TransactionGroup {
        id: 1,
        transactional: true,
        steps: vec![
            RawStep {
                step_no: 0,
                kind: StepKind::CreateSchema,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qn("app", "app")],
                sql: "CREATE SCHEMA app;".into(),
                transactional: TransactionConstraint::InTransaction,
            },
            RawStep {
                step_no: 0,
                kind: StepKind::CreateMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qn("app", "mv_revenue")],
                sql: "CREATE MATERIALIZED VIEW app.mv_revenue AS SELECT 1 AS n WITH NO DATA;"
                    .into(),
                transactional: TransactionConstraint::InTransaction,
            },
            RawStep {
                step_no: 0,
                kind: StepKind::RefreshMaterializedView,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qn("app", "mv_revenue")],
                sql: "REFRESH MATERIALIZED VIEW app.mv_revenue;".into(),
                transactional: TransactionConstraint::InTransaction,
            },
        ],
    }];

    let mut plan = Plan::from_grouped(
        groups,
        &Catalog::empty(),
        &Catalog::empty(),
        identity,
        None,
        "0.2.0",
        2,
    )
    .unwrap();

    // Suppress the REFRESH step via a [[step_override]] row.
    plan.step_overrides.push(StepOverride {
        kind: "refresh_materialized_view".to_string(),
        target: "app.mv_revenue".to_string(),
        suppress: true,
    });

    // Write plan to disk so the full apply path (including read_plan_dir)
    // exercises the serialize/deserialize round-trip.
    let dir = tempfile::tempdir().unwrap();
    pgevolve_core::plan::serialize::write_plan_dir(&plan, dir.path()).unwrap();

    let filter = pgevolve_core::catalog::CatalogFilter::new(vec![id("app")], vec![]).unwrap();
    pgevolve::apply(
        dir.path(),
        &mut client,
        &filter,
        pgevolve::executor::ApplyOverrides {
            allow_drift: true,
            ..Default::default()
        },
    )
    .await
    .expect("apply ok");

    // The MV should exist in pg_class.
    let row = client
        .query_one(
            "SELECT COUNT(*)::int FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'app' AND c.relname = 'mv_revenue'",
            &[],
        )
        .await
        .unwrap();
    let exists: i32 = row.get(0);
    assert_eq!(exists, 1, "MV should exist");

    // The MV should NOT be populated (relispopulated = false) because the
    // REFRESH was suppressed.
    let row = client
        .query_one(
            "SELECT relispopulated FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'app' AND c.relname = 'mv_revenue'",
            &[],
        )
        .await
        .unwrap();
    let populated: bool = row.get(0);
    assert!(
        !populated,
        "MV should not be populated (REFRESH was suppressed)"
    );

    // The audit log should show the REFRESH step as 'skipped'.
    let row = client
        .query_one(
            "SELECT status FROM pgevolve.plan_steps \
             WHERE apply_id = (SELECT apply_id FROM pgevolve.apply_log ORDER BY started_at DESC LIMIT 1) \
               AND kind = 'refresh_materialized_view'",
            &[],
        )
        .await
        .unwrap();
    let status: String = row.get(0);
    assert_eq!(
        status, "skipped",
        "Suppressed REFRESH step should be skipped in audit log"
    );
}
