//! Lint-waiver end-to-end tests.
//!
//! Verifies that `pgevolve plan` exits with code 2 when a `LintAtPlan`
//! finding (column-position-drift) has no matching `[[lint_waiver]]` in
//! `intent.toml`, and that adding a waiver allows the plan to proceed.
//!
//! Skipped when Docker is unavailable.

use std::fs;
use std::path::Path;
use std::process::Command;

use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

fn cargo_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_BIN_EXE_pgevolve"));
    if !p.exists() {
        p = std::path::PathBuf::from("target/debug/pgevolve");
    }
    p
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Write a pgevolve.toml with the given DSN.
fn write_config(project: &Path, dsn: &str) {
    let cfg = format!(
        "[project]\n\
         name = \"lint-waiver-e2e\"\n\
         schema_dir = \"schema\"\n\
         plan_dir = \"plans\"\n\
         layout_profile = \"schema-mirror\"\n\n\
         [managed]\n\
         schemas = [\"app\"]\n\n\
         [planner]\n\
         strategy = \"online\"\n\n\
         [environments.dev]\n\
         url = \"{dsn}\"\n"
    );
    fs::write(project.join("pgevolve.toml"), cfg).unwrap();
}

/// Bootstrap pgevolve metadata schema in the DB.
fn run_bootstrap(project: &Path, bin: &Path) {
    let status = Command::new(bin)
        .current_dir(project)
        .args(["bootstrap", "--db", "dev"])
        .status()
        .expect("run bootstrap");
    assert!(status.success(), "bootstrap failed");
}

/// Run `pgevolve plan` and return the `Output`.
fn run_plan(project: &Path, bin: &Path) -> std::process::Output {
    Command::new(bin)
        .current_dir(project)
        .args(["plan", "--db", "dev"])
        .output()
        .expect("run plan")
}

/// Apply DDL directly to the DB to set up the pre-existing state.
async fn exec_sql(pg: &EphemeralPostgres, sql: &str) {
    let client = pg.connect().await.expect("connect");
    client.execute(sql, &[]).await.expect("exec sql");
}

// ---------------------------------------------------------------------------
// Test 1: plan refuses when column-position-drift has no waiver
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_refuses_unwaived_column_position_drift() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start pg");
    let bin = cargo_bin();

    let project = tempfile::tempdir().unwrap();
    let project_path = project.path();

    write_config(project_path, pg.dsn());

    // Seed the live DB with columns in order (id, email, created_at).
    exec_sql(
        &pg,
        "CREATE SCHEMA app; \
         CREATE TABLE app.users ( \
             id bigint NOT NULL, \
             email text NOT NULL, \
             created_at timestamptz NOT NULL, \
             CONSTRAINT users_pkey PRIMARY KEY (id) \
         );",
    )
    .await;

    // Source declares columns in a DIFFERENT order (id, created_at, email).
    write(
        &project_path.join("schema/app/0001-users.sql"),
        "-- @pgevolve schema=app\n\
         CREATE SCHEMA app;\n\
         CREATE TABLE app.users (\n\
             id bigint NOT NULL,\n\
             created_at timestamptz NOT NULL,\n\
             email text NOT NULL,\n\
             CONSTRAINT users_pkey PRIMARY KEY (id)\n\
         );\n",
    );

    run_bootstrap(project_path, &bin);

    // `plan` must exit with code 2 (unwaived LintAtPlan finding).
    let out = run_plan(project_path, &bin);
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit code 2; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("column-position-drift"),
        "expected column-position-drift in stderr; got: {stderr}"
    );
    assert!(
        stderr.contains("lint_waiver"),
        "expected lint_waiver hint in stderr; got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: plan proceeds when a matching [[lint_waiver]] is present
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_proceeds_with_matching_lint_waiver() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version())
        .await
        .expect("start pg");
    let bin = cargo_bin();

    let project = tempfile::tempdir().unwrap();
    let project_path = project.path();

    write_config(project_path, pg.dsn());

    // Seed the live DB with columns in order (id, email, created_at).
    exec_sql(
        &pg,
        "CREATE SCHEMA app; \
         CREATE TABLE app.users ( \
             id bigint NOT NULL, \
             email text NOT NULL, \
             created_at timestamptz NOT NULL, \
             CONSTRAINT users_pkey PRIMARY KEY (id) \
         );",
    )
    .await;

    // Source declares columns in a DIFFERENT order (id, created_at, email).
    write(
        &project_path.join("schema/app/0001-users.sql"),
        "-- @pgevolve schema=app\n\
         CREATE SCHEMA app;\n\
         CREATE TABLE app.users (\n\
             id bigint NOT NULL,\n\
             created_at timestamptz NOT NULL,\n\
             email text NOT NULL,\n\
             CONSTRAINT users_pkey PRIMARY KEY (id)\n\
         );\n",
    );

    run_bootstrap(project_path, &bin);

    // First run: expect exit code 2.
    let out = run_plan(project_path, &bin);
    assert_eq!(
        out.status.code(),
        Some(2),
        "first plan should fail; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The plan command tells us where it would write the intent.toml.
    // Since it exits before writing, we need to compute the plan dir ourselves.
    // The plan dir is plans/<date>-<plan_id_short>. We can cheat: write a
    // temporary waiver directly into a pre-created intent.toml path.
    //
    // Re-run plan with a --output flag pointing to a pre-staged directory.
    let staged_dir = project_path.join("plans/staged");
    fs::create_dir_all(&staged_dir).unwrap();

    // Stage an intent.toml with the matching waiver.
    let waiver_toml = "\
plan_id = \"00000000000000000000000000000000000000000000000000000000deadbeef\"\n\
\n\
[[lint_waiver]]\n\
rule = \"column-position-drift\"\n\
target = \"app.users\"\n\
reason = \"column order drift acknowledged; rewrite-table pending\"\n";
    // The plan_id will be overwritten by the actual plan; we just need the
    // waiver to be present for the gate check. The gate loads waivers from
    // the output directory's existing intent.toml BEFORE writing.
    write(&staged_dir.join("intent.toml"), waiver_toml);

    // Re-run plan with --output pointing to our staged directory.
    let out = Command::new(&bin)
        .current_dir(project_path)
        .args(["plan", "--db", "dev", "--output"])
        .arg(&staged_dir)
        .output()
        .expect("run plan with --output");

    assert!(
        out.status.success(),
        "plan with waiver should succeed; stderr: {}; stdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Wrote plan"),
        "expected 'Wrote plan' in stdout; got: {stdout}"
    );

    // Confirm the plan directory has the three files.
    assert!(staged_dir.join("plan.sql").exists(), "plan.sql not written");
    assert!(
        staged_dir.join("intent.toml").exists(),
        "intent.toml not written"
    );
    assert!(
        staged_dir.join("manifest.toml").exists(),
        "manifest.toml not written"
    );
}

// ---------------------------------------------------------------------------
// Test 3: unit-level — LintWaiver round-trips through intent.toml serialize +
// deserialize (pure in-process, no Docker needed).
// ---------------------------------------------------------------------------

#[test]
fn lint_waiver_survives_intent_toml_round_trip() {
    use pgevolve_core::catalog::DriftReport;
    use pgevolve_core::plan::{
        LintWaiver, Plan, PlannerPolicy, group_steps, order, rewrite,
        write_intent_toml, read_intent_toml,
    };
    use pgevolve_core::ir::catalog::Catalog;

    // Build a trivial empty plan.
    let source = Catalog::empty();
    let target = Catalog::empty();
    let changes = pgevolve_core::diff::diff(&target, &source, &DriftReport::default());
    let ordered = order(&target, &source, changes).expect("order");
    let policy = PlannerPolicy::default();
    let steps = rewrite(ordered, &target, &policy);
    let groups = group_steps(steps);
    let mut plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        "test-identity".into(),
        None,
        "0.1.0",
        1,
    );

    // Inject a waiver into the plan.
    plan.lint_waivers.push(LintWaiver {
        rule: "column-position-drift".to_string(),
        target: "app.users".to_string(),
        reason: "acknowledged; rewrite pending".to_string(),
    });

    // Serialize to intent.toml bytes.
    let mut buf = Vec::new();
    write_intent_toml(&plan, &mut buf).unwrap();
    let toml_str = String::from_utf8(buf).unwrap();

    // Confirm the waiver section is present.
    assert!(
        toml_str.contains("[[lint_waiver]]"),
        "expected [[lint_waiver]] section; got:\n{toml_str}"
    );
    assert!(toml_str.contains("column-position-drift"));
    assert!(toml_str.contains("app.users"));

    // Deserialize and confirm the waiver survives.
    let parsed = read_intent_toml(&toml_str).unwrap();
    assert_eq!(parsed.lint_waivers.len(), 1);
    assert_eq!(parsed.lint_waivers[0].rule, "column-position-drift");
    assert_eq!(parsed.lint_waivers[0].target, "app.users");
    assert_eq!(parsed.lint_waivers[0].reason, "acknowledged; rewrite pending");
}
