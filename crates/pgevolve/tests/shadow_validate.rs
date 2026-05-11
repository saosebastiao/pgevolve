//! End-to-end test for `pgevolve validate --shadow`.
//!
//! Spins up a real shadow Postgres via testcontainers, runs the binary
//! against a hand-authored project tree, and asserts:
//! - clean round-trip → exit 0
//! - mismatched source → exit 1
//!
//! Skipped when Docker is unavailable.

use std::fs;
use std::path::Path;
use std::process::Command;

use pgevolve_core::catalog::PgVersion;
use pgevolve_testkit::ephemeral_pg::docker_available;

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

fn pgevolve_toml(pg_version: PgVersion) -> String {
    let pg = match pg_version {
        PgVersion::Pg14 => "14",
        PgVersion::Pg15 => "15",
        PgVersion::Pg16 => "16",
        PgVersion::Pg17 => "17",
    };
    format!(
        r#"[project]
name           = "shadow-validate-e2e"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "free-form"

[managed]
schemas = ["app"]

[planner]
strategy = "online"

[environments.dev]
url = "postgres://localhost/unused"

[shadow]
provider         = "testcontainers"
postgres_version = "{pg}"
"#,
    )
}

#[test]
fn shadow_round_trip_succeeds_on_clean_source() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }

    let project = tempfile::tempdir().unwrap();
    let project_path = project.path();
    fs::write(
        project_path.join("pgevolve.toml"),
        pgevolve_toml(PgVersion::Pg16),
    )
    .unwrap();
    write(
        &project_path.join("schema/app/0001-init.sql"),
        "-- @pgevolve schema=app\n\
         CREATE SCHEMA app;\n\
         CREATE TABLE app.users (\n\
             id bigint NOT NULL,\n\
             email text NOT NULL,\n\
             CONSTRAINT users_pkey PRIMARY KEY (id)\n\
         );\n",
    );

    let out = Command::new(cargo_bin())
        .current_dir(project_path)
        .args(["validate", "--shadow"])
        .output()
        .expect("run validate --shadow");
    assert!(
        out.status.success(),
        "validate --shadow stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("round-trip matched"), "got: {stdout}");
}

#[test]
fn shadow_without_section_errors_cleanly() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("pgevolve.toml"),
        // Same body as the happy-path test but with no [shadow] section.
        r#"[project]
name           = "no-shadow-section"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "free-form"

[environments.dev]
url = "postgres://localhost/unused"
"#,
    )
    .unwrap();
    fs::create_dir_all(project.path().join("schema")).unwrap();

    let out = Command::new(cargo_bin())
        .current_dir(project.path())
        .args(["validate", "--shadow"])
        .output()
        .expect("run validate --shadow");
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("[shadow] section"),
        "expected helpful error, got: {stderr}",
    );
}
