//! End-to-end CLI flow: `init` → `plan` → `apply` → `status` against
//! [`EphemeralPostgres`]. Skipped when Docker is unavailable.

use std::fs;
use std::path::Path;
use std::process::Command;

use pgevolve_core::catalog::PgVersion;
use pgevolve_testkit::ephemeral_pg::{docker_available, EphemeralPostgres};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_to_end_init_plan_apply_status() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(PgVersion::Pg16).await.unwrap();

    let project = tempfile::tempdir().unwrap();
    let project_path = project.path();

    // 1. init
    let status = Command::new(cargo_bin())
        .args(["init", "--dir"])
        .arg(project_path)
        .status()
        .expect("run init");
    assert!(status.success(), "init exited non-zero");
    assert!(project_path.join("pgevolve.toml").exists());

    // Overwrite pgevolve.toml with the real DSN and a single managed schema.
    let cfg = format!(
        "[project]\nname = \"e2e\"\nschema_dir = \"schema\"\nplan_dir = \"plans\"\nlayout_profile = \"schema-mirror\"\n\n\
         [managed]\nschemas = [\"app\"]\n\n\
         [planner]\nstrategy = \"online\"\n\n\
         [environments.dev]\nurl = \"{}\"\n",
        pg.dsn()
    );
    fs::write(project_path.join("pgevolve.toml"), cfg).unwrap();

    // Seed source: one schema + one table.
    write(
        &project_path.join("schema/app/0001-init.sql"),
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.users (id bigint NOT NULL, CONSTRAINT users_pkey PRIMARY KEY (id));\n",
    );

    // 2. bootstrap — pre-creates the pgevolve schema.
    let status = Command::new(cargo_bin())
        .current_dir(project_path)
        .args(["bootstrap", "--db", "dev"])
        .status()
        .expect("run bootstrap");
    assert!(status.success());

    // 3. plan
    let out = Command::new(cargo_bin())
        .current_dir(project_path)
        .args(["plan", "--db", "dev"])
        .output()
        .expect("run plan");
    assert!(out.status.success(), "plan stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("Wrote plan"), "got: {stdout}");

    // Extract the plan directory from the output.
    let plan_dir_line = stdout
        .lines()
        .find(|l| l.starts_with("Wrote plan"))
        .expect("plan output");
    let plan_dir_str = plan_dir_line
        .split(" to ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let plan_dir_rel = Path::new(plan_dir_str).to_path_buf();
    let plan_dir_abs = project_path.join(&plan_dir_rel);
    assert!(
        plan_dir_abs.is_dir(),
        "plan dir not found at {}",
        plan_dir_abs.display(),
    );

    // 4. apply
    let out = Command::new(cargo_bin())
        .current_dir(project_path)
        .args(["apply"])
        .arg(&plan_dir_rel)
        .args(["--db", "dev"])
        .output()
        .expect("run apply");
    assert!(
        out.status.success(),
        "apply failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // 5. verify the table was actually created.
    let client = pg.connect().await.unwrap();
    let row = client
        .query_one("SELECT to_regclass('app.users')::text IS NOT NULL", &[])
        .await
        .unwrap();
    let exists: bool = row.get(0);
    assert!(exists, "app.users not present after apply");

    // 6. status
    let out = Command::new(cargo_bin())
        .current_dir(project_path)
        .args(["status", "--db", "dev"])
        .output()
        .expect("run status");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("status=succeeded"), "status said: {stdout}");
}

#[tokio::test]
async fn help_lists_all_nine_commands() {
    let out = Command::new(cargo_bin())
        .arg("--help")
        .output()
        .expect("run --help");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    for cmd in [
        "init",
        "lint",
        "validate",
        "diff",
        "plan",
        "apply",
        "status",
        "dump",
        "bootstrap",
    ] {
        assert!(stdout.contains(cmd), "--help missing `{cmd}`:\n{stdout}");
    }
}
