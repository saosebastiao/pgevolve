//! SQL emission for view and MV planner steps.
//!
//! Each `emit_*` function produces a single canonical SQL statement string
//! (always ending with `;`) suitable for direct embedding in `plan.sql`.
//! Output is deterministic: same input IR always produces the same bytes.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::view::{CheckOption, MaterializedView, View};

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// `CREATE [OR REPLACE] VIEW qname [(columns)] [WITH (...)] AS <body>;`
pub(crate) fn emit_create_view(v: &View, or_replace: bool) -> String {
    let mut sql = String::new();
    if or_replace {
        sql.push_str("CREATE OR REPLACE VIEW ");
    } else {
        sql.push_str("CREATE VIEW ");
    }
    sql.push_str(&v.qname.render_sql());
    if !v.columns.is_empty() {
        sql.push_str(" (");
        for (i, c) in v.columns.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&c.name.render_sql());
        }
        sql.push(')');
    }
    if let Some(opts) = view_with_clause(v) {
        sql.push_str(" WITH (");
        sql.push_str(&opts);
        sql.push(')');
    }
    sql.push_str(" AS\n");
    let body = v.body_canonical.canonical_text().trim_end();
    // Strip trailing semicolon from body if present — the check option clause or
    // our own semicolon goes at the end.
    let body = body.trim_end_matches(';');
    sql.push_str(body);
    // Append WITH CHECK OPTION clause when present.
    if let Some(co) = v.check_option {
        sql.push_str("\nWITH ");
        sql.push_str(match co {
            CheckOption::Local => "LOCAL",
            CheckOption::Cascaded => "CASCADED",
        });
        sql.push_str(" CHECK OPTION");
    }
    sql.push(';');
    sql
}

fn view_with_clause(v: &View) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = v.security_barrier {
        parts.push(format!("security_barrier = {b}"));
    }
    if let Some(i) = v.security_invoker {
        parts.push(format!("security_invoker = {i}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// `DROP VIEW qname;`
pub(crate) fn emit_drop_view(qname: &QualifiedName) -> String {
    format!("DROP VIEW {};", qname.render_sql())
}

/// `ALTER VIEW qname SET (security_barrier = ..., ...);`
pub(crate) fn emit_alter_view_set_reloption(
    qname: &QualifiedName,
    security_barrier: Option<bool>,
    security_invoker: Option<bool>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = security_barrier {
        parts.push(format!("security_barrier = {b}"));
    }
    if let Some(i) = security_invoker {
        parts.push(format!("security_invoker = {i}"));
    }
    format!(
        "ALTER VIEW {} SET ({});",
        qname.render_sql(),
        parts.join(", ")
    )
}

/// `CREATE OR REPLACE VIEW qname … WITH [LOCAL|CASCADED] CHECK OPTION;`
///
/// PG has no `ALTER VIEW … SET CHECK OPTION`; pgevolve re-issues the full
/// `CREATE OR REPLACE VIEW` with the desired `check_option` state. The caller
/// must pass the full source `View` IR (with the new `check_option` set).
pub(crate) fn emit_alter_view_set_check_option(v: &View) -> String {
    emit_create_view(v, true)
}

/// `COMMENT ON VIEW qname IS '...'|NULL;`
pub(crate) fn emit_comment_on_view(qname: &QualifiedName, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON VIEW {} IS '{}';",
            qname.render_sql(),
            c.replace('\'', "''"),
        ),
        None => format!("COMMENT ON VIEW {} IS NULL;", qname.render_sql()),
    }
}

/// `COMMENT ON COLUMN qname.col IS '...'|NULL;`
///
/// Used for view column comments (PG uses the same `COMMENT ON COLUMN` syntax
/// for both table and view columns).
pub(crate) fn emit_comment_on_view_column(
    qname: &QualifiedName,
    column: &Identifier,
    comment: Option<&str>,
) -> String {
    let col_ref = format!("{}.{}", qname.render_sql(), column.render_sql());
    match comment {
        Some(c) => format!(
            "COMMENT ON COLUMN {col_ref} IS '{}';",
            c.replace('\'', "''")
        ),
        None => format!("COMMENT ON COLUMN {col_ref} IS NULL;"),
    }
}

// ---------------------------------------------------------------------------
// Materialized views
// ---------------------------------------------------------------------------

/// `CREATE MATERIALIZED VIEW qname [(columns)] AS <body>\nWITH NO DATA;`
pub(crate) fn emit_create_materialized_view(mv: &MaterializedView) -> String {
    let mut sql = String::new();
    sql.push_str("CREATE MATERIALIZED VIEW ");
    sql.push_str(&mv.qname.render_sql());
    if !mv.columns.is_empty() {
        sql.push_str(" (");
        for (i, c) in mv.columns.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&c.name.render_sql());
        }
        sql.push(')');
    }
    sql.push_str(" AS\n");
    // Body should NOT end with `;` — the `WITH NO DATA` clause follows.
    let body = mv.body_canonical.canonical_text().trim_end();
    let body = body.trim_end_matches(';');
    sql.push_str(body);
    sql.push_str("\nWITH NO DATA;");
    sql
}

/// `DROP MATERIALIZED VIEW qname;`
pub(crate) fn emit_drop_materialized_view(qname: &QualifiedName) -> String {
    format!("DROP MATERIALIZED VIEW {};", qname.render_sql())
}

/// `REFRESH MATERIALIZED VIEW [CONCURRENTLY] qname;`
pub(crate) fn emit_refresh_mv(qname: &QualifiedName, concurrently: bool) -> String {
    if concurrently {
        format!(
            "REFRESH MATERIALIZED VIEW CONCURRENTLY {};",
            qname.render_sql()
        )
    } else {
        format!("REFRESH MATERIALIZED VIEW {};", qname.render_sql())
    }
}

/// `COMMENT ON MATERIALIZED VIEW qname IS '...'|NULL;`
pub(crate) fn emit_comment_on_materialized_view(
    qname: &QualifiedName,
    comment: Option<&str>,
) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON MATERIALIZED VIEW {} IS '{}';",
            qname.render_sql(),
            c.replace('\'', "''"),
        ),
        None => format!(
            "COMMENT ON MATERIALIZED VIEW {} IS NULL;",
            qname.render_sql()
        ),
    }
}

/// `COMMENT ON COLUMN qname.col IS '...'|NULL;`
///
/// PG treats MV columns the same as table columns for `COMMENT ON COLUMN`.
pub(crate) fn emit_comment_on_mv_column(
    qname: &QualifiedName,
    column: &Identifier,
    comment: Option<&str>,
) -> String {
    let col_ref = format!("{}.{}", qname.render_sql(), column.render_sql());
    match comment {
        Some(c) => format!(
            "COMMENT ON COLUMN {col_ref} IS '{}';",
            c.replace('\'', "''")
        ),
        None => format!("COMMENT ON COLUMN {col_ref} IS NULL;"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column_type::ColumnType;
    use crate::ir::view::ViewColumn;
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn body(sql: &str) -> NormalizedBody {
        NormalizedBody::from_sql(sql).unwrap()
    }

    fn simple_view() -> View {
        View {
            qname: qn("app", "active_users"),
            columns: vec![],
            body_canonical: body("SELECT id, email FROM app.users WHERE active"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        }
    }

    fn simple_mv() -> MaterializedView {
        MaterializedView {
            qname: qn("app", "user_summary"),
            columns: vec![],
            body_canonical: body("SELECT count(*) FROM app.users"),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        }
    }

    // --- emit_create_view ---

    #[test]
    fn create_view_basic() {
        let v = simple_view();
        let sql = emit_create_view(&v, false);
        assert_eq!(
            sql,
            "CREATE VIEW app.active_users AS\nSELECT id, email FROM app.users WHERE active;"
        );
    }

    #[test]
    fn create_view_or_replace() {
        let v = simple_view();
        let sql = emit_create_view(&v, true);
        assert!(sql.starts_with("CREATE OR REPLACE VIEW app.active_users AS\n"));
    }

    #[test]
    fn create_view_with_explicit_columns() {
        let v = View {
            columns: vec![
                ViewColumn {
                    name: id("a"),
                    column_type: Some(ColumnType::BigInt),
                    comment: None,
                },
                ViewColumn {
                    name: id("b"),
                    column_type: Some(ColumnType::Text),
                    comment: None,
                },
            ],
            ..simple_view()
        };
        let sql = emit_create_view(&v, false);
        assert!(
            sql.contains("(a, b)"),
            "expected explicit column list, got: {sql}"
        );
    }

    #[test]
    fn create_view_with_security_barrier() {
        let v = View {
            security_barrier: Some(true),
            ..simple_view()
        };
        let sql = emit_create_view(&v, false);
        assert!(sql.contains("WITH (security_barrier = true)"), "got: {sql}");
    }

    #[test]
    fn create_view_with_both_reloptions() {
        let v = View {
            security_barrier: Some(false),
            security_invoker: Some(true),
            ..simple_view()
        };
        let sql = emit_create_view(&v, false);
        assert!(
            sql.contains("WITH (security_barrier = false, security_invoker = true)"),
            "got: {sql}"
        );
    }

    #[test]
    fn create_view_body_already_ends_with_semicolon() {
        // NormalizedBody strips trailing whitespace but may or may not have ';'.
        // emit_create_view always ensures exactly one trailing ';'.
        let v = simple_view();
        let sql = emit_create_view(&v, false);
        assert!(sql.ends_with(';'));
        // Only one trailing semicolon.
        assert!(!sql.ends_with(";;"));
    }

    // --- emit_drop_view ---

    #[test]
    fn drop_view_basic() {
        assert_eq!(
            emit_drop_view(&qn("app", "old_view")),
            "DROP VIEW app.old_view;"
        );
    }

    // --- emit_alter_view_set_reloption ---

    #[test]
    fn alter_view_set_reloption_single() {
        let sql = emit_alter_view_set_reloption(&qn("app", "v"), Some(true), None);
        assert_eq!(sql, "ALTER VIEW app.v SET (security_barrier = true);");
    }

    #[test]
    fn alter_view_set_reloption_both() {
        let sql = emit_alter_view_set_reloption(&qn("app", "v"), Some(false), Some(true));
        assert_eq!(
            sql,
            "ALTER VIEW app.v SET (security_barrier = false, security_invoker = true);"
        );
    }

    // --- emit_comment_on_view ---

    #[test]
    fn comment_on_view_set() {
        let sql = emit_comment_on_view(&qn("app", "v"), Some("my comment"));
        assert_eq!(sql, "COMMENT ON VIEW app.v IS 'my comment';");
    }

    #[test]
    fn comment_on_view_clear() {
        let sql = emit_comment_on_view(&qn("app", "v"), None);
        assert_eq!(sql, "COMMENT ON VIEW app.v IS NULL;");
    }

    #[test]
    fn comment_on_view_escapes_single_quotes() {
        let sql = emit_comment_on_view(&qn("app", "v"), Some("it's a view"));
        assert_eq!(sql, "COMMENT ON VIEW app.v IS 'it''s a view';");
    }

    // --- emit_comment_on_view_column ---

    #[test]
    fn comment_on_view_column_set() {
        let sql = emit_comment_on_view_column(&qn("app", "v"), &id("col"), Some("the col"));
        assert_eq!(sql, "COMMENT ON COLUMN app.v.col IS 'the col';");
    }

    #[test]
    fn comment_on_view_column_clear() {
        let sql = emit_comment_on_view_column(&qn("app", "v"), &id("col"), None);
        assert_eq!(sql, "COMMENT ON COLUMN app.v.col IS NULL;");
    }

    // --- emit_create_materialized_view ---

    #[test]
    fn create_materialized_view_basic() {
        let mv = simple_mv();
        let sql = emit_create_materialized_view(&mv);
        assert_eq!(
            sql,
            "CREATE MATERIALIZED VIEW app.user_summary AS\nSELECT count(*) FROM app.users\nWITH NO DATA;"
        );
    }

    #[test]
    fn create_materialized_view_does_not_double_semicolon() {
        let mv = simple_mv();
        let sql = emit_create_materialized_view(&mv);
        // Should end with exactly "WITH NO DATA;"
        assert!(sql.ends_with("WITH NO DATA;"));
        assert!(!sql.ends_with("WITH NO DATA;;"));
    }

    #[test]
    fn create_materialized_view_with_columns() {
        let mv = MaterializedView {
            columns: vec![ViewColumn {
                name: id("total"),
                column_type: Some(ColumnType::BigInt),
                comment: None,
            }],
            ..simple_mv()
        };
        let sql = emit_create_materialized_view(&mv);
        assert!(sql.contains("(total)"), "expected column list, got: {sql}");
    }

    // --- emit_drop_materialized_view ---

    #[test]
    fn drop_materialized_view_basic() {
        assert_eq!(
            emit_drop_materialized_view(&qn("app", "user_summary")),
            "DROP MATERIALIZED VIEW app.user_summary;"
        );
    }

    // --- emit_refresh_mv ---

    #[test]
    fn refresh_mv_basic() {
        let sql = emit_refresh_mv(&qn("app", "user_summary"), false);
        assert_eq!(sql, "REFRESH MATERIALIZED VIEW app.user_summary;");
    }

    #[test]
    fn refresh_mv_concurrently() {
        let sql = emit_refresh_mv(&qn("app", "user_summary"), true);
        assert_eq!(
            sql,
            "REFRESH MATERIALIZED VIEW CONCURRENTLY app.user_summary;"
        );
    }

    // --- emit_comment_on_materialized_view ---

    #[test]
    fn comment_on_mv_set() {
        let sql = emit_comment_on_materialized_view(&qn("app", "mv"), Some("summary"));
        assert_eq!(sql, "COMMENT ON MATERIALIZED VIEW app.mv IS 'summary';");
    }

    #[test]
    fn comment_on_mv_clear() {
        let sql = emit_comment_on_materialized_view(&qn("app", "mv"), None);
        assert_eq!(sql, "COMMENT ON MATERIALIZED VIEW app.mv IS NULL;");
    }

    #[test]
    fn comment_on_mv_escapes_quotes() {
        let sql = emit_comment_on_materialized_view(&qn("app", "mv"), Some("it's fast"));
        assert_eq!(sql, "COMMENT ON MATERIALIZED VIEW app.mv IS 'it''s fast';");
    }

    // --- emit_comment_on_mv_column ---

    #[test]
    fn comment_on_mv_column_set() {
        let sql = emit_comment_on_mv_column(&qn("app", "mv"), &id("total"), Some("the total"));
        assert_eq!(sql, "COMMENT ON COLUMN app.mv.total IS 'the total';");
    }

    #[test]
    fn comment_on_mv_column_clear() {
        let sql = emit_comment_on_mv_column(&qn("app", "mv"), &id("total"), None);
        assert_eq!(sql, "COMMENT ON COLUMN app.mv.total IS NULL;");
    }
}
