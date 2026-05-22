//! Cluster-source parser end-to-end tests using a temp directory.

use std::fs;

use pgevolve_core::parse::cluster::parse_cluster_directory;
use tempfile::TempDir;

fn write_roles(td: &TempDir, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = td.path().join("roles");
    fs::create_dir(&dir).unwrap();
    for (name, sql) in files {
        fs::write(dir.join(name), sql).unwrap();
    }
    dir
}

#[test]
fn create_role_defaults() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "CREATE ROLE app_user;")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    assert_eq!(cat.roles.len(), 1);
    let r = &cat.roles[0];
    assert_eq!(r.name.as_str(), "app_user");
    assert!(!r.attributes.login);
    assert!(r.attributes.inherit);
    assert!(r.member_of.is_empty());
}

#[test]
fn create_user_implies_login() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "CREATE USER app_user;")]);
    let cat = parse_cluster_directory(&dir).unwrap();
    assert!(
        cat.roles[0].attributes.login,
        "CREATE USER must imply LOGIN=true"
    );
}

#[test]
fn full_attribute_matrix() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(
        &td,
        &[(
            "a.sql",
            "CREATE ROLE admin WITH SUPERUSER CREATEDB CREATEROLE LOGIN \
             CONNECTION LIMIT 50 VALID UNTIL '2030-01-01T00:00:00Z';",
        )],
    );
    let cat = parse_cluster_directory(&dir).unwrap();
    let r = &cat.roles[0];
    assert!(r.attributes.superuser);
    assert!(r.attributes.createdb);
    assert!(r.attributes.createrole);
    assert!(r.attributes.login);
    assert_eq!(r.attributes.connection_limit, Some(50));
    assert_eq!(
        r.attributes.valid_until.as_deref(),
        Some("2030-01-01T00:00:00Z")
    );
}

#[test]
fn grant_role_to_role() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(
        &td,
        &[(
            "a.sql",
            "CREATE ROLE readers; CREATE ROLE app_user; GRANT readers TO app_user;",
        )],
    );
    let cat = parse_cluster_directory(&dir).unwrap();
    let app = cat
        .roles
        .iter()
        .find(|r| r.name.as_str() == "app_user")
        .unwrap();
    assert_eq!(
        app.member_of
            .iter()
            .map(pgevolve_core::identifier::Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["readers"]
    );
}

#[test]
fn in_role_inline_form() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(
        &td,
        &[(
            "a.sql",
            "CREATE ROLE readers; CREATE ROLE app_user IN ROLE readers;",
        )],
    );
    let cat = parse_cluster_directory(&dir).unwrap();
    let app = cat
        .roles
        .iter()
        .find(|r| r.name.as_str() == "app_user")
        .unwrap();
    assert_eq!(
        app.member_of
            .iter()
            .map(pgevolve_core::identifier::Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["readers"]
    );
}

#[test]
fn alter_role_modifies_existing() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(
        &td,
        &[(
            "a.sql",
            "CREATE ROLE app_user; ALTER ROLE app_user WITH LOGIN CREATEDB;",
        )],
    );
    let cat = parse_cluster_directory(&dir).unwrap();
    let r = &cat.roles[0];
    assert!(r.attributes.login);
    assert!(r.attributes.createdb);
}

#[test]
fn drop_role_in_source_errors() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "DROP ROLE app_user;")]);
    let err = parse_cluster_directory(&dir).unwrap_err();
    assert!(err.to_string().contains("DROP ROLE"), "got: {err}");
}

#[test]
fn password_clause_is_dropped_silently() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(
        &td,
        &[(
            "a.sql",
            "CREATE ROLE app_user WITH LOGIN PASSWORD 'hunter2';",
        )],
    );
    let cat = parse_cluster_directory(&dir).unwrap();
    assert!(cat.roles[0].attributes.login);
    // No assertion on password — the IR doesn't carry it.
}

#[test]
fn comment_on_role() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(
        &td,
        &[(
            "a.sql",
            "CREATE ROLE app_user; COMMENT ON ROLE app_user IS 'application service account';",
        )],
    );
    let cat = parse_cluster_directory(&dir).unwrap();
    assert_eq!(
        cat.roles[0].comment.as_deref(),
        Some("application service account")
    );
}

#[test]
fn unknown_statement_kind_errors() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "GRANT SELECT ON TABLE foo TO bar;")]);
    let err = parse_cluster_directory(&dir).unwrap_err();
    assert!(err.to_string().contains("not supported"), "got: {err}");
}

#[test]
fn alter_role_unknown_role_errors() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "ALTER ROLE nonexistent WITH LOGIN;")]);
    let err = parse_cluster_directory(&dir).unwrap_err();
    assert!(err.to_string().contains("unknown role"), "got: {err}");
}

#[test]
fn revoke_role_in_source_errors() {
    let td = TempDir::new().unwrap();
    let dir = write_roles(&td, &[("a.sql", "REVOKE readers FROM app_user;")]);
    let err = parse_cluster_directory(&dir).unwrap_err();
    assert!(err.to_string().contains("REVOKE"), "got: {err}");
}
