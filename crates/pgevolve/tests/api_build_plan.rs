//! Docker-gated happy-path test for `pgevolve::api::build_plan`.
//!
//! Spins up an `EphemeralPostgres`, seeds a minimal schema, calls
//! `build_plan` against a tempdir source, and asserts the returned plan
//! has the expected shape.

use anyhow::Result;
use pgevolve::api::{BuildPlanOptions, build_plan};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::plan::Strategy;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_plan_produces_a_create_table_plan() -> Result<()> {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return Ok(());
    }

    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let mut client = pg.connect().await?;
    pgevolve::executor::bootstrap_metadata(&mut client).await?;
    // Seed an empty `app` schema so the catalog has *something* in
    // managed scope but no tables.
    client.batch_execute("CREATE SCHEMA app;").await?;
    drop(client);

    let tmp = tempfile::tempdir()?;
    std::fs::create_dir_all(tmp.path().join("schema"))?;
    std::fs::write(
        tmp.path().join("schema/0001.sql"),
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    )?;

    let opts = BuildPlanOptions {
        managed_schemas: vec![Identifier::from_unquoted("app").unwrap()],
        ignore_objects: vec![],
        strategy: Strategy::Online,
        planner_ruleset_version: pgevolve_core::plan::PlannerPolicy::default()
            .planner_ruleset_version,
        existing_lint_waivers: vec![],
        source_rev: None,
    };
    let client = pg.connect().await?;
    let plan = build_plan(&tmp.path().join("schema"), client, opts).await?;

    assert!(!plan.groups.is_empty(), "expected at least one group");
    let step_count: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
    assert!(step_count >= 1, "expected at least one step, got {step_count}");
    Ok(())
}
