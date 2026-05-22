//! column-position-drift lint rule.

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::column::Column;
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::schema::Schema;
use pgevolve_core::ir::table::Table;
use pgevolve_core::lint::Severity;
use pgevolve_core::lint::universal::run_drift_lints;

fn id(s: &str) -> Identifier {
    Identifier::from_unquoted(s).unwrap()
}

fn qn(schema: &str, name: &str) -> QualifiedName {
    QualifiedName::new(id(schema), id(name))
}

fn col(name: &str) -> Column {
    Column {
        name: id(name),
        ty: ColumnType::Text,
        nullable: true,
        default: None,
        identity: None,
        generated: None,
        collation: None,
        storage: None,
        compression: None,
        comment: None,
    }
}

/// Build a catalog with one schema and one `app.users` table whose columns
/// are in the specified order.
fn catalog_with_columns(order: &[&str]) -> Catalog {
    let mut c = Catalog::empty();
    c.schemas.push(Schema::new(id("app")));
    c.tables.push(Table {
        qname: qn("app", "users"),
        columns: order.iter().map(|name| col(name)).collect(),
        constraints: vec![],
        partition_by: None,
        partition_of: None,
        comment: None,
    });
    c
}

#[test]
fn column_position_drift_surfaces_as_lint_at_plan() {
    let source = catalog_with_columns(&["id", "email", "created_at"]);
    let target = catalog_with_columns(&["id", "created_at", "email"]); // reordered

    let findings = run_drift_lints(&source, &target);
    let pos = findings
        .iter()
        .find(|f| f.rule == "column-position-drift")
        .expect("column-position-drift should fire");
    assert_eq!(pos.severity, Severity::LintAtPlan);
    assert!(
        pos.message.contains("app.users"),
        "should name the table: {}",
        pos.message
    );
    assert!(
        pos.message.contains("position"),
        "should mention 'position': {}",
        pos.message
    );
}

#[test]
fn matching_column_order_does_not_fire() {
    let source = catalog_with_columns(&["id", "email", "created_at"]);
    let target = catalog_with_columns(&["id", "email", "created_at"]);

    let findings = run_drift_lints(&source, &target);
    assert!(
        !findings.iter().any(|f| f.rule == "column-position-drift"),
        "lint should not fire on matching order",
    );
}

#[test]
fn added_column_in_source_does_not_trigger_position_drift() {
    // source has an extra column not in target — should not fire position drift
    let source = catalog_with_columns(&["id", "email", "created_at", "new_col"]);
    let target = catalog_with_columns(&["id", "email", "created_at"]);

    let findings = run_drift_lints(&source, &target);
    assert!(
        !findings.iter().any(|f| f.rule == "column-position-drift"),
        "adding a column should not trigger position drift: {findings:?}",
    );
}

#[test]
fn removed_column_from_target_does_not_trigger_position_drift() {
    // target has an extra column not in source — common columns stay in same
    // relative order, so no position drift
    let source = catalog_with_columns(&["id", "email"]);
    let target = catalog_with_columns(&["id", "extra", "email"]);

    let findings = run_drift_lints(&source, &target);
    assert!(
        !findings.iter().any(|f| f.rule == "column-position-drift"),
        "target-only extra column should not trigger position drift: {findings:?}",
    );
}

#[test]
fn table_only_in_source_does_not_fire() {
    // table exists in source but not target — no target to compare against
    let source = catalog_with_columns(&["id", "email"]);
    let target = Catalog::empty();

    let findings = run_drift_lints(&source, &target);
    assert!(
        !findings.iter().any(|f| f.rule == "column-position-drift"),
        "new table should not trigger position drift",
    );
}
