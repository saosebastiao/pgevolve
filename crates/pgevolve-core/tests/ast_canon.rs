//! Tests for the AST canonicalization pass (T4).
//!
//! All tests use `parse_directory` on a temporary directory — the
//! canonicalization pass runs automatically as part of `parse_directory`
//! whenever the catalog contains views or materialized views.

use std::path::Path;

use pgevolve_core::parse::parse_directory;

fn write(dir: &Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

// ── helpers ────────────────────────────────────────────────────────────────

const fn schema_file() -> &'static str {
    "-- @pgevolve schema=app\nCREATE SCHEMA app;\n"
}

// ── body_canonical ─────────────────────────────────────────────────────────

#[test]
fn canonicalization_fills_body_canonical_for_view() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY, email text);\n",
    );
    write(
        tmp.path(),
        "app/users_summary.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.users_summary AS SELECT id, email FROM app.users;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");

    assert_eq!(catalog.views.len(), 1);
    let view = &catalog.views[0];
    assert!(
        !view.body_canonical.canonical_text().is_empty(),
        "body_canonical should be non-empty after canonicalization"
    );
    assert_ne!(
        view.body_canonical.canonical_hash(),
        &[0u8; 32],
        "canonical_hash should be non-zero"
    );
}

#[test]
fn canonicalization_fills_body_canonical_for_materialized_view() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/events.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.events (id bigint PRIMARY KEY);\n",
    );
    write(
        tmp.path(),
        "app/events_mv.sql",
        "-- @pgevolve schema=app\n\
         CREATE MATERIALIZED VIEW app.events_mv AS SELECT id FROM app.events WITH NO DATA;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");

    assert_eq!(catalog.materialized_views.len(), 1);
    let mv = &catalog.materialized_views[0];
    assert!(
        !mv.body_canonical.canonical_text().is_empty(),
        "MV body_canonical should be non-empty after canonicalization"
    );
    assert_ne!(
        mv.body_canonical.canonical_hash(),
        &[0u8; 32],
        "MV canonical_hash should be non-zero"
    );
}

// ── body_dependencies ──────────────────────────────────────────────────────

#[test]
fn canonicalization_fills_dep_edges() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );
    write(
        tmp.path(),
        "app/users_summary.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.users_summary AS SELECT id FROM app.users;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");
    let view = &catalog.views[0];
    assert!(
        !view.body_dependencies.is_empty(),
        "should have at least one dep edge"
    );
    let has_users_edge = view
        .body_dependencies
        .iter()
        .any(|e| format!("{:?}", e.to).contains("users"));
    assert!(
        has_users_edge,
        "expected dep edge to app.users, got {:?}",
        view.body_dependencies
    );
}

#[test]
fn dep_edge_deduplication_works() {
    // A view that references the same table twice (e.g. a self-join) should
    // produce only one dep edge.
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.users (id bigint PRIMARY KEY, manager_id bigint);\n",
    );
    write(
        tmp.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.v AS \
         SELECT u.id, m.id AS manager FROM app.users u JOIN app.users m ON u.manager_id = m.id;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");
    let view = &catalog.views[0];
    let users_edge_count = view
        .body_dependencies
        .iter()
        .filter(|e| format!("{:?}", e.to).contains("users"))
        .count();
    assert_eq!(
        users_edge_count, 1,
        "self-join should produce exactly one dep edge to app.users, got {:?}",
        view.body_dependencies
    );
}

// ── error: unresolved reference ────────────────────────────────────────────

#[test]
fn canonicalization_errors_on_missing_referenced_table() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.v AS SELECT id FROM app.nonexistent;\n",
    );

    let err = parse_directory(tmp.path(), &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("nonexistent"),
        "error should name the missing relation: {msg}"
    );
}

#[test]
fn canonicalization_errors_on_missing_referenced_table_names_the_view() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/my_view.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.my_view AS SELECT id FROM app.ghost_table;\n",
    );

    let err = parse_directory(tmp.path(), &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("my_view"),
        "error should name the offending view: {msg}"
    );
}

// ── columns derived from SELECT target list ────────────────────────────────

#[test]
fn columns_derived_when_alias_list_absent() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.users (id bigint PRIMARY KEY, email text);\n",
    );
    write(
        tmp.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.v AS SELECT id, email FROM app.users;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");
    let view = &catalog.views[0];
    let names: Vec<_> = view.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "email"]);
}

#[test]
fn explicit_alias_list_not_overwritten_by_canonicalization() {
    // CREATE VIEW v(alias_a, alias_b) AS SELECT id, email FROM app.users
    // should keep the explicit aliases, not replace them with "id" and "email".
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.users (id bigint PRIMARY KEY, email text);\n",
    );
    write(
        tmp.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.v(alias_a, alias_b) AS SELECT id, email FROM app.users;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");
    let view = &catalog.views[0];
    let names: Vec<_> = view.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["alias_a", "alias_b"]);
}

#[test]
fn qualified_column_ref_gives_rightmost_name() {
    // SELECT app.users.email derives column name "email".
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.users (id bigint PRIMARY KEY, email text);\n",
    );
    write(
        tmp.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.v AS SELECT app.users.email FROM app.users;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");
    let view = &catalog.views[0];
    let names: Vec<_> = view.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["email"]);
}

#[test]
fn res_target_alias_takes_priority_over_column_ref() {
    // SELECT id AS user_id → column name "user_id".
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );
    write(
        tmp.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.v AS SELECT id AS user_id FROM app.users;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");
    let view = &catalog.views[0];
    let names: Vec<_> = view.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["user_id"]);
}

// ── whitespace canonicalization ────────────────────────────────────────────

#[test]
fn whitespace_equivalent_bodies_produce_equal_canonical_text() {
    let tmp1 = tempfile::tempdir().unwrap();
    write(tmp1.path(), "app/schema.sql", schema_file());
    write(
        tmp1.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );
    write(
        tmp1.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\nCREATE VIEW app.v AS SELECT    id   FROM   app.users;\n",
    );

    let tmp2 = tempfile::tempdir().unwrap();
    write(tmp2.path(), "app/schema.sql", schema_file());
    write(
        tmp2.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );
    write(
        tmp2.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\nCREATE VIEW app.v AS SELECT id FROM app.users;\n",
    );

    let c1 = parse_directory(tmp1.path(), &[]).expect("parse 1");
    let c2 = parse_directory(tmp2.path(), &[]).expect("parse 2");

    assert_eq!(
        c1.views[0].body_canonical.canonical_text(),
        c2.views[0].body_canonical.canonical_text(),
        "whitespace-equivalent bodies should produce identical canonical text"
    );
}

// ── v0.1 fixtures not broken ───────────────────────────────────────────────

#[test]
fn parse_directory_without_views_still_works() {
    // v0.1 catalogs (tables only) should parse cleanly; the canonicalization
    // pass is skipped when there are no views.
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("v0.1 parse");
    assert_eq!(catalog.views.len(), 0);
    assert_eq!(catalog.materialized_views.len(), 0);
    assert_eq!(catalog.tables.len(), 1);
}

// ── subquery / CTE deps ───────────────────────────────────────────────────

#[test]
fn subquery_dep_extracted() {
    // A view whose body contains a subquery should still extract the dep edge.
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "app/schema.sql", schema_file());
    write(
        tmp.path(),
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );
    write(
        tmp.path(),
        "app/v.sql",
        "-- @pgevolve schema=app\n\
         CREATE VIEW app.v AS SELECT * FROM (SELECT id FROM app.users) sub;\n",
    );

    let catalog = parse_directory(tmp.path(), &[]).expect("parse + canonicalize");
    let view = &catalog.views[0];
    let has_users_edge = view
        .body_dependencies
        .iter()
        .any(|e| format!("{:?}", e.to).contains("users"));
    assert!(
        has_users_edge,
        "subquery dep to app.users should be extracted, got {:?}",
        view.body_dependencies
    );
}
