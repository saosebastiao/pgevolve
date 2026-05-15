//! --shadow-validate flag smoke tests.

#[test]
fn plan_help_shows_shadow_validate_flag() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pgevolve"))
        .arg("plan")
        .arg("--help")
        .output()
        .expect("run pgevolve");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("--shadow-validate"),
        "help should list --shadow-validate: {stdout}"
    );
}

#[test]
fn shadow_strict_requires_shadow_validate() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(
        dir.join("pgevolve.toml"),
        "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n\
         [environments.dev]\nurl=\"postgres://unused\"\n",
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pgevolve"))
        .arg("plan")
        .arg("--db")
        .arg("dev")
        .arg("--shadow-strict")
        .current_dir(dir)
        .output()
        .expect("run pgevolve");

    // clap should reject --shadow-strict without --shadow-validate at
    // argument-parse time, before any DB connection is attempted.
    assert!(
        !output.status.success(),
        "should refuse --shadow-strict without --shadow-validate"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("shadow-validate") || stderr.contains("requires"),
        "clap should mention the requirement: {stderr}"
    );
}

#[test]
fn diff_help_shows_shadow_validate_flag() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pgevolve"))
        .arg("diff")
        .arg("--help")
        .output()
        .expect("run pgevolve");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--shadow-validate"),
        "diff help should list --shadow-validate"
    );
}

#[test]
fn validate_help_shows_shadow_validate_flag() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pgevolve"))
        .arg("validate")
        .arg("--help")
        .output()
        .expect("run pgevolve");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--shadow-validate"),
        "validate help should list --shadow-validate"
    );
}
