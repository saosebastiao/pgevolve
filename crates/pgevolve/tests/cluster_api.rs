//! Docker-gated integration tests for `pgevolve::api::cluster`.
//!
//! These tests verify the **wiring** of the cluster plan pipeline — in
//! particular that `check_cluster_changeset` is actually called inside
//! `build_cluster_plan` so that lint findings appear in production rather
//! than only in unit tests.
//!
//! Stage 9 of the cluster-roles implementation plan.

use pgevolve::api::build_cluster_plan;
use pgevolve::cluster_config::{
    Bootstrap, ClusterConfig, ClusterConnection, ClusterProject, TablespacesConfig,
};
use pgevolve_core::plan::raw_step::StepKind;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

/// Helper: write a minimal `roles/*.sql` file tree under `dir`.
fn write_roles_dir(dir: &std::path::Path, sql: &str) {
    let roles_dir = dir.join("roles");
    std::fs::create_dir_all(&roles_dir).unwrap();
    std::fs::write(roles_dir.join("roles.sql"), sql).unwrap();
}

/// Smoke test: with an empty roles dir, `build_cluster_plan` should succeed
/// and produce no steps (the live cluster only has bootstrap roles which are
/// filtered out) and no findings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_cluster_plan_empty_roles_dir() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start ephemeral postgres");
    let dsn = pg.dsn().to_string();

    let tmp = tempfile::TempDir::new().unwrap();
    // Write an empty roles dir (no .sql files).
    std::fs::create_dir_all(tmp.path().join("roles")).unwrap();

    let cfg = ClusterConfig {
        project: ClusterProject {
            name: "test".into(),
        },
        connection: ClusterConnection { dsn },
        bootstrap: Bootstrap {
            roles: vec!["postgres".into(), "pgevolve".into()],
        },
        tablespaces: TablespacesConfig::default(),
    };

    let plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("build_cluster_plan should succeed");

    // No roles declared in source, bootstrap roles filtered → no steps.
    assert!(
        plan.steps.is_empty(),
        "expected no steps for empty roles dir, got {:?}",
        plan.steps
    );
    assert!(
        plan.advisory_findings.is_empty(),
        "expected no advisory findings, got {:?}",
        plan.advisory_findings
    );
}

/// Wiring test: `build_cluster_plan` must call `check_cluster_changeset` and
/// surface `role-loses-superuser` findings in `ClusterPlan::advisory_findings`.
///
/// This test is the guard against the v0.2.1 lint-wiring bug: the lint
/// dispatcher was exported but never called from `build_plan`, so lint rules
/// fired only in unit tests, not in production.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_cluster_plan_surfaces_role_loses_superuser_finding() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start ephemeral postgres");

    // Seed a superuser role in the live cluster.
    pg.exec_sql("CREATE ROLE admin WITH SUPERUSER LOGIN;")
        .await
        .expect("seed superuser role");

    let dsn = pg.dsn().to_string();

    // Source declares `admin` WITHOUT superuser — triggering role-loses-superuser.
    let tmp = tempfile::TempDir::new().unwrap();
    write_roles_dir(tmp.path(), "CREATE ROLE admin WITH LOGIN;\n");

    let cfg = ClusterConfig {
        project: ClusterProject {
            name: "test".into(),
        },
        connection: ClusterConnection { dsn },
        bootstrap: Bootstrap {
            roles: vec!["postgres".into(), "pgevolve".into()],
        },
        tablespaces: TablespacesConfig::default(),
    };

    let plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("build_cluster_plan should succeed");

    let has_superuser_finding = plan
        .advisory_findings
        .iter()
        .any(|f| f.rule == "role-loses-superuser");

    assert!(
        has_superuser_finding,
        "expected at least one role-loses-superuser finding — check_cluster_changeset \
         is not being called from build_cluster_plan. Got {} total findings: {:#?}",
        plan.advisory_findings.len(),
        plan.advisory_findings
    );
}

/// Verify that `build_cluster_plan` produces `CreateRole` steps when the
/// source has a role that the live cluster does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_cluster_plan_creates_new_role() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }

    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start ephemeral postgres");
    let dsn = pg.dsn().to_string();

    let tmp = tempfile::TempDir::new().unwrap();
    write_roles_dir(tmp.path(), "CREATE ROLE app_user WITH LOGIN;\n");

    let cfg = ClusterConfig {
        project: ClusterProject {
            name: "test".into(),
        },
        connection: ClusterConnection { dsn },
        bootstrap: Bootstrap {
            roles: vec!["postgres".into(), "pgevolve".into()],
        },
        tablespaces: TablespacesConfig::default(),
    };

    let plan = build_cluster_plan(tmp.path(), &cfg)
        .await
        .expect("build_cluster_plan should succeed");

    let has_create_role_step = plan
        .steps
        .iter()
        .any(|s| matches!(s.kind, StepKind::CreateRole));

    assert!(
        has_create_role_step,
        "expected at least one CreateRole step for a role that exists in source \
         but not in the live cluster. Got {} steps: {:#?}",
        plan.steps.len(),
        plan.steps
    );
}
