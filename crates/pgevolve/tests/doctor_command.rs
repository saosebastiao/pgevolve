//! `pgevolve doctor` and `rewrite-table` smoke tests.

fn cargo_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_BIN_EXE_pgevolve"));
    if !p.exists() {
        p = std::path::PathBuf::from("target/debug/pgevolve");
    }
    p
}

#[test]
fn doctor_help_includes_command() {
    let output = std::process::Command::new(cargo_bin())
        .arg("doctor")
        .arg("--help")
        .output()
        .expect("run pgevolve");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.to_lowercase().contains("health") || stdout.contains("doctor"),
        "help should describe the doctor command: {stdout}"
    );
}

#[test]
fn rewrite_table_refuses_without_confirm_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(
        dir.join("pgevolve.toml"),
        "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n\
         [environments.dev]\nurl=\"postgres://unused\"\n",
    )
    .unwrap();

    let output = std::process::Command::new(cargo_bin())
        .arg("rewrite-table")
        .arg("app.users")
        .arg("--db")
        .arg("dev")
        .current_dir(dir)
        .output()
        .expect("run pgevolve");

    assert!(
        !output.status.success(),
        "should refuse without --confirm-rewrite"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--confirm-rewrite"),
        "stderr should explain the flag: {stderr}"
    );
}

#[test]
fn rewrite_table_with_confirm_reports_not_yet_implemented() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(
        dir.join("pgevolve.toml"),
        "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n\
         [environments.dev]\nurl=\"postgres://unused\"\n",
    )
    .unwrap();

    let output = std::process::Command::new(cargo_bin())
        .arg("rewrite-table")
        .arg("app.users")
        .arg("--db")
        .arg("dev")
        .arg("--confirm-rewrite")
        .current_dir(dir)
        .output()
        .expect("run pgevolve");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not yet implemented") || stderr.contains("v0.2"),
        "stderr should explain v0.2 status: {stderr}"
    );
}
