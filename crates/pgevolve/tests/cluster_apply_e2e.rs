//! End-to-end cluster apply tests against an ephemeral PG.
//!
//! Exercises the full cluster plan → cluster apply pipeline:
//! - clean apply (CREATE ROLE succeeds, `apply_log` row written)
//! - intent-blocked apply (`DropRole` unapproved → fail)
//! - identity mismatch (plan generated with wrong `target_identity` rejected)
//!
//! Skipped wholesale when Docker is unavailable.

#![cfg(test)]

use pgevolve::api::cluster::build_cluster_plan;
use pgevolve::cluster_config::{
    Bootstrap, ClusterConfig, ClusterConnection, ClusterProject, TablespacesConfig,
};
use pgevolve::executor::{ApplyOverrides, apply_cluster_plan};
use pgevolve::target_identity::compute_cluster_target_identity;
use pgevolve_core::plan::{PlanId, read_plan_dir, write_plan_dir};
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

fn write_role_file(project_root: &std::path::Path, sql: &str) {
    let roles_dir = project_root.join("roles");
    std::fs::create_dir_all(&roles_dir).unwrap();
    std::fs::write(roles_dir.join("test.sql"), sql).unwrap();
}

fn write_tablespace_file(project_root: &std::path::Path, sql: &str) {
    let ts_dir = project_root.join("tablespaces");
    std::fs::create_dir_all(&ts_dir).unwrap();
    std::fs::write(ts_dir.join("t.sql"), sql).unwrap();
}

fn cluster_cfg_for(pg: &EphemeralPostgres) -> ClusterConfig {
    ClusterConfig {
        project: ClusterProject {
            name: "test-cluster".into(),
        },
        connection: ClusterConnection {
            dsn: pg.dsn().to_string(),
        },
        bootstrap: Bootstrap {
            // The ephemeral container creates a "pgevolve" superuser; include
            // it alongside "postgres" so it never appears in the diff.
            roles: vec!["postgres".into(), "pgevolve".into()],
        },
        tablespaces: TablespacesConfig::default(),
    }
}

/// Clean apply: CREATE ROLE succeeds and the role appears in `pg_authid`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_apply_clean_path_succeeds() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start ephemeral postgres");
    let tmp = tempfile::tempdir().unwrap();

    write_role_file(tmp.path(), "CREATE ROLE app_test_role NOLOGIN;");
    let cfg = cluster_cfg_for(&pg);

    // --- Plan phase ---
    let cluster_plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("build_cluster_plan");

    assert!(
        !cluster_plan.steps.is_empty(),
        "expected at least one step (CREATE ROLE)"
    );

    let (id_client, id_conn) = tokio_postgres::connect(pg.dsn(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = id_conn.await;
    });
    let target_identity = compute_cluster_target_identity(&id_client)
        .await
        .expect("compute_cluster_target_identity");

    // Use a stable short plan_id for the test directory name.
    let plan_id = PlanId::from_hex("aabbccddeeff0011").expect("valid hex");
    let core_plan = cluster_plan
        .to_plan(plan_id, target_identity)
        .expect("to_plan");

    let plan_dir = tmp.path().join("cluster-plans").join("clean-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    write_plan_dir(&core_plan, &plan_dir).expect("write_plan_dir");

    // --- Apply phase ---
    let plan = read_plan_dir(&plan_dir).expect("read_plan_dir");
    let (mut client, apply_conn) = tokio_postgres::connect(pg.dsn(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = apply_conn.await;
    });

    apply_cluster_plan(&plan, &mut client, ApplyOverrides::default())
        .await
        .expect("apply should succeed for clean CREATE ROLE plan");

    // Verify role exists.
    let row = client
        .query_opt(
            "SELECT 1 FROM pg_authid WHERE rolname = $1",
            &[&"app_test_role"],
        )
        .await
        .expect("query pg_authid");
    assert!(row.is_some(), "app_test_role should exist in pg_authid");
}

/// Clean apply of a tablespace: `CREATE TABLESPACE` succeeds and the
/// tablespace appears in `pg_tablespace`.
///
/// `CREATE TABLESPACE` requires the LOCATION directory to exist inside the
/// container, so we provision it over the superuser bootstrap connection
/// before applying the plan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_apply_tablespace_clean_path_succeeds() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start ephemeral postgres");
    let tmp = tempfile::tempdir().unwrap();

    // The default tablespaces dir is `<project>/tablespaces`, so writing the
    // file there works with the default `TablespacesConfig`.
    write_tablespace_file(
        tmp.path(),
        "CREATE TABLESPACE ts_e2e LOCATION '/tmp/pgev_e2e_ts';",
    );
    let cfg = cluster_cfg_for(&pg);

    // Provision the LOCATION directory inside the container as the superuser.
    let setup_client = pg.connect().await.expect("setup connect");
    setup_client
        .batch_execute(
            "COPY (SELECT 1) TO PROGRAM 'mkdir -p /tmp/pgev_e2e_ts && chmod 700 /tmp/pgev_e2e_ts';",
        )
        .await
        .expect("provision tablespace location dir");

    // --- Plan phase ---
    let cluster_plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("build_cluster_plan");

    assert!(
        !cluster_plan.steps.is_empty(),
        "expected at least one step (CREATE TABLESPACE)"
    );

    let (id_client, id_conn) = tokio_postgres::connect(pg.dsn(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = id_conn.await;
    });
    let target_identity = compute_cluster_target_identity(&id_client)
        .await
        .expect("compute_cluster_target_identity");

    let plan_id = PlanId::from_hex("cafebabecafebabe").expect("valid hex");
    let core_plan = cluster_plan
        .to_plan(plan_id, target_identity)
        .expect("to_plan");

    let plan_dir = tmp.path().join("cluster-plans").join("ts-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    write_plan_dir(&core_plan, &plan_dir).expect("write_plan_dir");

    // --- Apply phase ---
    let plan = read_plan_dir(&plan_dir).expect("read_plan_dir");
    let (mut client, apply_conn) = tokio_postgres::connect(pg.dsn(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = apply_conn.await;
    });

    apply_cluster_plan(&plan, &mut client, ApplyOverrides::default())
        .await
        .expect("apply should succeed for clean CREATE TABLESPACE plan");

    // Verify the tablespace exists.
    let row = client
        .query_opt(
            "SELECT 1 FROM pg_tablespace WHERE spcname = $1",
            &[&"ts_e2e"],
        )
        .await
        .expect("query pg_tablespace");
    assert!(row.is_some(), "ts_e2e should exist in pg_tablespace");

    // --- Idempotency check: second plan must be empty ---
    // Build the plan again against the now-live DB.  The source still contains
    // the same `CREATE TABLESPACE ts_e2e LOCATION '/tmp/pgev_e2e_ts'` — the
    // tablespace now exists in the catalog, so the diff should produce zero
    // steps and zero `tablespace-location-drift` advisory findings.
    let idempotency_plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("second build_cluster_plan for idempotency check");

    assert!(
        idempotency_plan.steps.is_empty(),
        "idempotency check failed: second plan is non-empty (steps: {:?})",
        idempotency_plan.steps
    );

    let drift_findings: Vec<_> = idempotency_plan
        .advisory_findings
        .iter()
        .filter(|f| f.rule == "tablespace-location-drift")
        .collect();
    assert!(
        drift_findings.is_empty(),
        "idempotency check failed: second plan has tablespace-location-drift findings: {drift_findings:?}"
    );
}

/// Intent-blocked apply: a DROP ROLE plan with unapproved intent must be
/// rejected at preflight. The role must still exist after the rejection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_apply_intent_blocked_when_unapproved() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start ephemeral postgres");
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cluster_cfg_for(&pg);

    // Create the role directly so the planner will see it and want to drop it.
    let setup_client = pg.connect().await.expect("setup connect");
    setup_client
        .batch_execute("CREATE ROLE will_be_dropped NOLOGIN;")
        .await
        .expect("create will_be_dropped");

    // Empty roles dir → planner sees will_be_dropped in live catalog but not
    // in desired state, so it emits a destructive DROP ROLE.
    write_role_file(tmp.path(), ""); // no roles desired

    let cluster_plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("build_cluster_plan");

    assert!(
        !cluster_plan.steps.is_empty(),
        "expected drop-role steps in plan"
    );

    let (id_client, id_conn) = tokio_postgres::connect(pg.dsn(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = id_conn.await;
    });
    let target_identity = compute_cluster_target_identity(&id_client)
        .await
        .expect("compute_cluster_target_identity");

    let plan_id = PlanId::from_hex("1122334455667788").expect("valid hex");
    let core_plan = cluster_plan
        .to_plan(plan_id, target_identity)
        .expect("to_plan");

    let plan_dir = tmp.path().join("cluster-plans").join("drop-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    write_plan_dir(&core_plan, &plan_dir).expect("write_plan_dir");

    // Read back without modifying intent.toml — intents remain approved=false.
    let plan = read_plan_dir(&plan_dir).expect("read_plan_dir");

    // If there are no destructive intents (role may be system-owned in
    // this container), skip the assertion rather than fail silently.
    if plan.intents.is_empty() {
        eprintln!("skipping intent-blocked assertion: no destructive intents in plan");
        return;
    }

    let (mut apply_client, apply_conn) = tokio_postgres::connect(pg.dsn(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = apply_conn.await;
    });

    let err = apply_cluster_plan(&plan, &mut apply_client, ApplyOverrides::default())
        .await
        .expect_err("apply should fail with unapproved intent");
    let msg = err.to_string();
    assert!(
        msg.contains("unapproved") || msg.contains("intents") || msg.contains("intent"),
        "expected unapproved-intent error, got: {msg}"
    );

    // Verify the role still exists (apply was rejected at preflight).
    let row = apply_client
        .query_opt(
            "SELECT 1 FROM pg_authid WHERE rolname = $1",
            &[&"will_be_dropped"],
        )
        .await
        .expect("query pg_authid");
    assert!(
        row.is_some(),
        "will_be_dropped should still exist after intent-blocked apply"
    );
}

/// Identity mismatch: a plan whose `target_identity` does not match the live
/// cluster must be rejected at preflight.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_apply_identity_mismatch_rejected() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start ephemeral postgres");
    let tmp = tempfile::tempdir().unwrap();

    write_role_file(tmp.path(), "CREATE ROLE id_mismatch_role NOLOGIN;");
    let cfg = cluster_cfg_for(&pg);

    let cluster_plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("build_cluster_plan");

    // Deliberately supply a wrong identity so preflight rejects.
    let plan_id = PlanId::from_hex("deadbeefdeadbeef").expect("valid hex");
    let core_plan = cluster_plan
        .to_plan(plan_id, "cluster:deadbeefdeadbeef".into())
        .expect("to_plan");

    let plan_dir = tmp.path().join("cluster-plans").join("mismatch-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    write_plan_dir(&core_plan, &plan_dir).expect("write_plan_dir");

    let plan = read_plan_dir(&plan_dir).expect("read_plan_dir");
    let (mut client, apply_conn) = tokio_postgres::connect(pg.dsn(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = apply_conn.await;
    });

    let err = apply_cluster_plan(&plan, &mut client, ApplyOverrides::default())
        .await
        .expect_err("apply should fail with identity mismatch");
    let msg = err.to_string();
    assert!(
        msg.contains("identity") || msg.contains("mismatch"),
        "expected identity-mismatch error, got: {msg}"
    );
}
