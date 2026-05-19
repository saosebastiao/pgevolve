//! AST resolution pass tests.

use pgevolve_core::parse::parse_directory;
use tempfile::tempdir;

fn write(dir: &std::path::Path, rel: &str, contents: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, contents).unwrap();
}

#[test]
fn fk_to_undeclared_table_is_caught_at_ast_resolution() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/orders.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.orders (\n\
             id bigint PRIMARY KEY,\n\
             user_id bigint NOT NULL,\n\
             CONSTRAINT fk_user FOREIGN KEY (user_id) REFERENCES app.users (id)\n\
         );\n",
    );
    // Note: app.users is NOT declared. Must fail.

    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("AST resolution failed"),
        "wrong error variant: {msg}",
    );
    assert!(
        msg.contains("app.users"),
        "error should name the missing table: {msg}",
    );
    assert!(
        msg.contains("fk_user"),
        "error should name the FK constraint: {msg}",
    );
    assert!(
        msg.contains("app.orders"),
        "error should name the referencing table: {msg}",
    );
}

#[test]
fn fk_to_declared_table_resolves() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );
    write(
        dir,
        "app/orders.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.orders (\n\
             id bigint PRIMARY KEY,\n\
             user_id bigint NOT NULL,\n\
             CONSTRAINT fk_user FOREIGN KEY (user_id) REFERENCES app.users (id)\n\
         );\n",
    );

    parse_directory(dir, &[]).expect("FK to declared table should resolve");
}

#[test]
fn default_nextval_on_undeclared_sequence_is_caught() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/widgets.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.widgets (\n\
             id bigint NOT NULL DEFAULT nextval('app.widget_id_seq'),\n\
             name text\n\
         );\n",
    );

    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("widget_id_seq"),
        "error should name the missing sequence: {msg}"
    );
}

#[test]
fn all_unresolved_refs_accumulated_not_short_circuited() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/t.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.t (\n\
             a_id bigint NOT NULL,\n\
             b_id bigint NOT NULL,\n\
             CONSTRAINT fk_a FOREIGN KEY (a_id) REFERENCES app.a (id),\n\
             CONSTRAINT fk_b FOREIGN KEY (b_id) REFERENCES app.b (id)\n\
         );\n",
    );
    // Both app.a and app.b are undeclared — both errors must appear.

    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("app.a"),
        "error should name missing table app.a: {msg}"
    );
    assert!(
        msg.contains("app.b"),
        "error should name missing table app.b: {msg}"
    );
}

#[test]
fn body_cycle_variant_renders_node_names() {
    use pgevolve_core::identifier::{Identifier, QualifiedName};
    use pgevolve_core::plan::edges::NodeId;
    use pgevolve_core::plan::error::PlanError;

    let nodes = vec![
        NodeId::Table(QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("a").unwrap(),
        )),
        NodeId::Table(QualifiedName::new(
            Identifier::from_unquoted("app").unwrap(),
            Identifier::from_unquoted("b").unwrap(),
        )),
    ];
    let err = PlanError::BodyCycle { nodes };
    let msg = err.to_string();
    assert!(
        msg.contains("body-derived"),
        "message should mention 'body-derived': {msg}"
    );
    assert!(msg.contains("app.a"), "should name 'app.a': {msg}");
    assert!(msg.contains("app.b"), "should name 'app.b': {msg}");
}

// ---------------------------------------------------------------------------
// UserDefined type reference resolution (T5)
// ---------------------------------------------------------------------------

#[test]
fn table_column_with_undeclared_user_type_fails() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/orders.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.orders (\n\
             id bigint PRIMARY KEY,\n\
             status app.order_status NOT NULL\n\
         );\n",
    );
    // app.order_status is NOT declared — must fail.

    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("app.order_status"),
        "error should name the missing type: {msg}",
    );
    // Error must indicate it's not declared/resolved.
    assert!(
        msg.contains("not declared") || msg.contains("undeclared") || msg.contains("not found"),
        "error should indicate undeclared type: {msg}",
    );
}

#[test]
fn table_column_with_declared_user_type_resolves() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/order_status.sql",
        "-- @pgevolve schema=app\n\
         CREATE TYPE app.order_status AS ENUM ('pending', 'shipped');\n",
    );
    write(
        dir,
        "app/orders.sql",
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.orders (\n\
             id bigint PRIMARY KEY,\n\
             status app.order_status NOT NULL\n\
         );\n",
    );

    let catalog = parse_directory(dir, &[]).expect("declared user type should resolve");
    assert_eq!(catalog.types.len(), 1, "should have 1 type");
    assert_eq!(catalog.tables.len(), 1, "should have 1 table");

    // Load-bearing check: the column type actually resolves to UserDefined,
    // not silently to ColumnType::Other (which was the pre-T5 bug).
    let status_col = catalog.tables[0]
        .columns
        .iter()
        .find(|c| c.name.as_str() == "status")
        .expect("status column must exist");
    match &status_col.ty {
        pgevolve_core::ir::column_type::ColumnType::UserDefined(q) => {
            assert_eq!(q.to_string(), "app.order_status");
        }
        other => panic!("expected UserDefined(app.order_status), got {other:?}"),
    }
}

#[test]
fn domain_base_with_undeclared_type_fails() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/my_domain.sql",
        "-- @pgevolve schema=app\n\
         CREATE DOMAIN app.my_domain AS app.nonexistent;\n",
    );
    // app.nonexistent is NOT declared — must fail.

    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("app.nonexistent"),
        "error should name the missing base type: {msg}",
    );
}

// ---------------------------------------------------------------------------
// Routine body dependency resolution (T6)
// ---------------------------------------------------------------------------

#[test]
fn function_body_with_undeclared_table_ref_fails() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/f.sql",
        "-- @pgevolve schema=app\n\
         CREATE FUNCTION app.f() RETURNS integer LANGUAGE sql AS $$ SELECT id FROM app.nonexistent $$;\n",
    );
    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("app.nonexistent"), "{msg}");
}

#[test]
fn function_body_with_declared_table_ref_resolves() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/users.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    );
    write(
        dir,
        "app/f.sql",
        "-- @pgevolve schema=app\n\
         CREATE FUNCTION app.f() RETURNS integer LANGUAGE sql AS $$ SELECT id FROM app.users LIMIT 1 $$;\n",
    );
    let catalog = parse_directory(dir, &[]).expect("declared table dep should resolve");
    assert_eq!(catalog.functions.len(), 1);
}

#[test]
fn procedure_body_with_undeclared_table_ref_fails() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/p.sql",
        "-- @pgevolve schema=app\n\
         CREATE PROCEDURE app.p() LANGUAGE plpgsql AS $$ BEGIN INSERT INTO app.nonexistent VALUES(1); END $$;\n",
    );
    let err = parse_directory(dir, &[]).unwrap_err();
    assert!(err.to_string().contains("app.nonexistent"));
}

#[test]
fn directive_dep_resolves_when_target_exists() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/log.sql",
        "-- @pgevolve schema=app\nCREATE TABLE app.log (n integer);\n",
    );
    write(
        dir,
        "app/f.sql",
        "-- @pgevolve schema=app\n\
         CREATE FUNCTION app.f() RETURNS void LANGUAGE plpgsql AS $$\n\
         -- @pgevolve dep: app.log\n\
         BEGIN EXECUTE 'INSERT INTO app.log VALUES (1)'; END\n\
         $$;\n",
    );
    let catalog = parse_directory(dir, &[]).expect("directive should resolve");
    assert_eq!(catalog.functions.len(), 1);
}

#[test]
fn composite_attribute_with_undeclared_type_fails() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "app/schema.sql",
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    );
    write(
        dir,
        "app/my_composite.sql",
        "-- @pgevolve schema=app\n\
         CREATE TYPE app.my_composite AS (foo app.nonexistent);\n",
    );
    // app.nonexistent is NOT declared — must fail.

    let err = parse_directory(dir, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("app.nonexistent"),
        "error should name the missing attribute type: {msg}",
    );
}
