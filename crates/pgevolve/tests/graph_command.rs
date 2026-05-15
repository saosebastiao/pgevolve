//! `pgevolve graph` CLI smoke tests.

mod common;

fn cargo_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_BIN_EXE_pgevolve"));
    if !p.exists() {
        p = std::path::PathBuf::from("target/debug/pgevolve");
    }
    p
}

#[test]
fn graph_dot_renders_v01_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Write a small project.
    std::fs::create_dir_all(dir.join("schema/app")).unwrap();
    std::fs::write(
        dir.join("schema/app/schema.sql"),
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("schema/app/users.sql"),
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("schema/app/idx.sql"),
        "-- @pgevolve schema=app\nCREATE INDEX users_id_idx ON app.users (id);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("pgevolve.toml"),
        "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n",
    )
    .unwrap();

    let output = std::process::Command::new(cargo_bin())
        .arg("graph")
        .arg("--graph-format=dot")
        .current_dir(dir)
        .output()
        .expect("run pgevolve");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit code: {:?}, stderr: {}",
        output.status.code(),
        stderr
    );
    assert!(
        stdout.contains("digraph pgevolve_deps"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("schema:app"), "stdout:\n{stdout}");
    assert!(stdout.contains("table:app.users"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("index:app.users_id_idx"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"table:app.users\" -> \"schema:app\""),
        "expected table→schema edge; stdout:\n{stdout}"
    );
}

#[test]
fn graph_mermaid_renders() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("schema/app")).unwrap();
    std::fs::write(
        dir.join("schema/app/schema.sql"),
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("schema/app/users.sql"),
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("pgevolve.toml"),
        "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n",
    )
    .unwrap();

    let output = std::process::Command::new(cargo_bin())
        .arg("graph")
        .arg("--graph-format=mermaid")
        .current_dir(dir)
        .output()
        .expect("run pgevolve");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit code: {:?}, stderr: {}",
        output.status.code(),
        stderr
    );
    assert!(
        stdout.starts_with("graph LR\n"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("-->"),
        "stdout should contain --> arrows:\n{stdout}"
    );
}

#[test]
fn graph_default_format_is_dot() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("schema/app")).unwrap();
    std::fs::write(
        dir.join("schema/app/schema.sql"),
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("pgevolve.toml"),
        "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n",
    )
    .unwrap();

    let output = std::process::Command::new(cargo_bin())
        .arg("graph")
        .current_dir(dir)
        .output()
        .expect("run pgevolve");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("digraph pgevolve_deps"),
        "stdout:\n{stdout}"
    );
}
