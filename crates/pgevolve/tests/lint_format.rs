//! `pgevolve lint --format json` smoke tests. No Docker needed.

use std::fs;
use std::path::Path;
use std::process::Command;

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

/// Set up a tiny project that lints clean.
fn setup_project(root: &Path) {
    write(
        &root.join("pgevolve.toml"),
        "[project]\n\
         name = \"t\"\n\
         schema_dir = \"schema\"\n\
         plan_dir = \"plans\"\n\
         layout_profile = \"free-form\"\n",
    );
    write(
        &root.join("schema/app/schema.sql"),
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
}

#[test]
fn lint_default_format_is_human() {
    let tmp = tempfile::tempdir().unwrap();
    setup_project(tmp.path());
    let out = Command::new(cargo_bin())
        .arg("lint")
        .current_dir(tmp.path())
        .output()
        .expect("run pgevolve lint");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0 findings"),
        "expected human '0 findings'; stdout: {stdout}",
    );
    // Must NOT be JSON.
    assert!(
        !stdout.contains("\"findings\""),
        "default format should be human, not JSON; stdout: {stdout}",
    );
}

#[test]
fn lint_json_format_emits_structured_output() {
    let tmp = tempfile::tempdir().unwrap();
    setup_project(tmp.path());
    let out = Command::new(cargo_bin())
        .args(["--format", "json", "lint"])
        .current_dir(tmp.path())
        .output()
        .expect("run pgevolve lint --format json");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\n---\n{stdout}"));
    assert_eq!(parsed["total"], 0, "expected total=0; got {parsed}");
    assert_eq!(parsed["errors"], 0, "expected errors=0; got {parsed}");
    assert!(parsed["findings"].is_array(), "findings should be an array");
}

#[test]
fn lint_sql_format_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    setup_project(tmp.path());
    let out = Command::new(cargo_bin())
        .args(["--format", "sql", "lint"])
        .current_dir(tmp.path())
        .output()
        .expect("run pgevolve lint --format sql");
    assert!(!out.status.success(), "lint should reject --format sql");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("sql") && stderr.contains("diff"),
        "stderr should explain that sql is for diff only: {stderr}",
    );
}
