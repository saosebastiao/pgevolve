//! Docker-gated tests for `pgevolve::api::build_plan`.
//!
//! Each test spins up an `EphemeralPostgres`, seeds state in the live DB,
//! calls `build_plan` against a tempdir source schema, and asserts the
//! returned plan has the expected shape.

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
    assert!(
        step_count >= 1,
        "expected at least one step, got {step_count}"
    );
    Ok(())
}

/// Verify that `build_plan` surfaces a `storage-downgrade-not-retroactive`
/// advisory finding when the diff contains a `SetColumnStorage` downgrade.
///
/// Setup:
///   live DB  → `app.docs.body` with `STORAGE EXTERNAL`   (target / pre-image)
///   source   → `app.docs.body` with `STORAGE PLAIN`       (desired)
///
/// The diff produces `SetColumnStorage { from: External, to: Plain }` which
/// `check_changeset` flags as a downgrade. `build_plan` must expose it through
/// `plan.advisory_findings`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_plan_surfaces_storage_downgrade_warning() -> Result<()> {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return Ok(());
    }

    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let mut client = pg.connect().await?;
    pgevolve::executor::bootstrap_metadata(&mut client).await?;

    // Seed the live DB with the pre-image state: table exists with STORAGE EXTERNAL.
    client
        .batch_execute(
            "CREATE SCHEMA app;\
             CREATE TABLE app.docs (id bigint PRIMARY KEY, body text);\
             ALTER TABLE app.docs ALTER COLUMN body SET STORAGE EXTERNAL;",
        )
        .await?;
    drop(client);

    // Source schema requests STORAGE PLAIN — a downgrade from EXTERNAL.
    let tmp = tempfile::tempdir()?;
    std::fs::create_dir_all(tmp.path().join("schema"))?;
    std::fs::write(
        tmp.path().join("schema/0001.sql"),
        "-- @pgevolve schema=app\n\
         CREATE SCHEMA app;\n\
         CREATE TABLE app.docs (\n\
             id bigint PRIMARY KEY,\n\
             body text STORAGE PLAIN\n\
         );\n",
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

    let downgrade_findings: Vec<_> = plan
        .advisory_findings
        .iter()
        .filter(|f| f.rule == "storage-downgrade-not-retroactive")
        .collect();

    assert_eq!(
        downgrade_findings.len(),
        1,
        "expected one storage-downgrade-not-retroactive advisory finding; \
         got {} total advisory finding(s): {:#?}",
        plan.advisory_findings.len(),
        plan.advisory_findings,
    );
    assert!(
        downgrade_findings[0].message.contains("EXTERNAL"),
        "finding message should mention EXTERNAL: {}",
        downgrade_findings[0].message,
    );
    assert!(
        downgrade_findings[0].message.contains("PLAIN"),
        "finding message should mention PLAIN: {}",
        downgrade_findings[0].message,
    );
    Ok(())
}
