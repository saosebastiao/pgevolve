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
    write(dir, "app/schema.sql", "-- @pgevolve schema=app\nCREATE SCHEMA app;\n");
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
    write(dir, "app/schema.sql", "-- @pgevolve schema=app\nCREATE SCHEMA app;\n");
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
    write(dir, "app/schema.sql", "-- @pgevolve schema=app\nCREATE SCHEMA app;\n");
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
    assert!(msg.contains("widget_id_seq"), "error should name the missing sequence: {msg}");
}

#[test]
fn all_unresolved_refs_accumulated_not_short_circuited() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    write(dir, "app/schema.sql", "-- @pgevolve schema=app\nCREATE SCHEMA app;\n");
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
    assert!(msg.contains("app.a"), "error should name missing table app.a: {msg}");
    assert!(msg.contains("app.b"), "error should name missing table app.b: {msg}");
}
