//! Integration smoke test for `parse_directory` against a hand-built fixture
//! directory laid down in a `tempdir`.

use std::fs;
use std::path::Path;

use pgevolve_core::parse;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn parses_a_two_table_two_index_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    write(&root.join("000_schema.sql"), "CREATE SCHEMA app;\n");
    write(
        &root.join("100_users.sql"),
        "-- @pgevolve schema=app\n\
         CREATE TABLE users (\n\
             id bigserial PRIMARY KEY,\n\
             email text NOT NULL\n\
         );\n\
         CREATE UNIQUE INDEX users_email_idx ON users (email);\n",
    );
    write(
        &root.join("200_orgs.sql"),
        "CREATE TABLE app.orgs (\n\
             id bigint PRIMARY KEY,\n\
             name text NOT NULL\n\
         );\n",
    );
    write(
        &root.join("300_links.sql"),
        "ALTER TABLE app.users ADD CONSTRAINT users_org_fkey \
         FOREIGN KEY (id) REFERENCES app.orgs (id);\n",
    );

    let catalog = parse::parse_directory(root, &[]).expect("parses");

    assert_eq!(catalog.schemas.len(), 1);
    assert_eq!(catalog.schemas[0].name.as_str(), "app");

    assert_eq!(catalog.tables.len(), 2);
    let users = catalog
        .tables
        .iter()
        .find(|t| t.qname.to_string() == "app.users")
        .expect("users present");
    let id = users
        .columns
        .iter()
        .find(|c| c.name.as_str() == "id")
        .expect("id column");
    // bigserial → BigInt + auto sequence + sequence default + NOT NULL.
    assert!(matches!(
        id.ty,
        pgevolve_core::ir::column_type::ColumnType::BigInt
    ));
    assert!(!id.nullable);
    assert!(matches!(
        id.default,
        Some(pgevolve_core::ir::default_expr::DefaultExpr::Sequence(_))
    ));

    // Constraints: PK + the FK from ALTER TABLE.
    assert!(users.constraints.iter().any(|c| matches!(
        c.kind,
        pgevolve_core::ir::constraint::ConstraintKind::PrimaryKey { .. }
    )));
    assert!(users.constraints.iter().any(|c| matches!(
        c.kind,
        pgevolve_core::ir::constraint::ConstraintKind::ForeignKey(_)
    )));

    assert_eq!(catalog.indexes.len(), 1);
    assert_eq!(catalog.indexes[0].qname.to_string(), "app.users_email_idx");

    // Synthesized sequence from bigserial.
    assert!(
        catalog
            .sequences
            .iter()
            .any(|s| s.qname.to_string() == "app.users_id_seq")
    );
}

#[test]
fn rejects_duplicate_table() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    write(&root.join("a.sql"), "CREATE TABLE app.t (id integer);\n");
    write(&root.join("b.sql"), "CREATE TABLE app.t (id integer);\n");

    let err = parse::parse_directory(root, &[]).unwrap_err();
    assert!(matches!(err, parse::ParseError::DuplicateObject { .. }));
}

#[test]
fn unsupported_object_kind_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    write(
        &root.join("bad.sql"),
        "CREATE FUNCTION app.f() RETURNS integer LANGUAGE sql AS $$ SELECT 1; $$;\n",
    );
    let err = parse::parse_directory(root, &[]).unwrap_err();
    match err {
        parse::ParseError::UnsupportedObjectKind { kind, .. } => {
            assert_eq!(kind, "CREATE FUNCTION/PROCEDURE");
        }
        other => panic!("expected UnsupportedObjectKind, got {other:?}"),
    }
}

#[test]
fn unqualified_name_without_directive_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("bare.sql"), "CREATE TABLE bare (id integer);\n");
    let err = parse::parse_directory(root, &[]).unwrap_err();
    assert!(matches!(err, parse::ParseError::UnqualifiedName { .. }));
}

#[test]
fn ignored_paths_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("good.sql"), "CREATE SCHEMA app;\n");
    write(
        &root.join("ignore_me/bad.sql"),
        "CREATE VIEW app.v AS SELECT 1;\n",
    );
    let pat = glob::Pattern::new(&format!("{}/ignore_me/**/*", root.display())).unwrap();
    let catalog = parse::parse_directory(root, &[pat]).expect("parses without bad");
    assert_eq!(catalog.schemas.len(), 1);
}
