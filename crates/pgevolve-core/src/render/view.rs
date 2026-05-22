//! View and materialized-view renderer.
//!
//! Produces `CREATE VIEW` and `CREATE MATERIALIZED VIEW` statements for use
//! in `render_catalog` (shadow-validate path) and `pgevolve dump`.

use crate::ir::view::{MaterializedView, View};

/// Render a `View` as a `CREATE VIEW ... AS <body>;` SQL string.
///
/// Intentionally omits `OR REPLACE` — this renderer targets a fresh shadow
/// DB (apply path), so the simpler `CREATE VIEW` form is correct.
#[must_use]
pub fn render_view(v: &View) -> String {
    use crate::plan::rewrite::views as emit;
    emit::emit_create_view(v, false)
}

/// Render a `MaterializedView` as a `CREATE MATERIALIZED VIEW ... AS <body> WITH NO DATA;` string.
#[must_use]
pub fn render_materialized_view(mv: &MaterializedView) -> String {
    use crate::plan::rewrite::views as emit;
    emit::emit_create_materialized_view(mv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column_type::ColumnType;
    use crate::ir::view::{MaterializedView, View, ViewColumn};
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

    #[test]
    fn render_view_produces_create_view() {
        let v = View {
            qname: qn("app", "active_users"),
            columns: vec![],
            body_canonical: body("SELECT id FROM app.users WHERE active = true"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        };
        let sql = render_view(&v);
        assert!(sql.starts_with("CREATE VIEW"), "got: {sql}");
        assert!(sql.contains("app.active_users"), "got: {sql}");
        assert!(sql.contains("SELECT"), "got: {sql}");
        assert!(sql.ends_with(';'), "got: {sql}");
    }

    #[test]
    fn render_view_no_or_replace() {
        let v = View {
            qname: qn("app", "v"),
            columns: vec![],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        };
        let sql = render_view(&v);
        assert!(
            !sql.contains("OR REPLACE"),
            "render_view should not emit OR REPLACE: {sql}"
        );
    }

    #[test]
    fn render_materialized_view_produces_create_materialized_view() {
        let mv = MaterializedView {
            qname: qn("app", "user_summary"),
            columns: vec![ViewColumn {
                name: id("cnt"),
                column_type: ColumnType::BigInt,
                comment: None,
            }],
            body_canonical: body("SELECT count(*) FROM app.users"),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        };
        let sql = render_materialized_view(&mv);
        assert!(sql.starts_with("CREATE MATERIALIZED VIEW"), "got: {sql}");
        assert!(sql.contains("app.user_summary"), "got: {sql}");
        assert!(sql.contains("SELECT"), "got: {sql}");
        assert!(sql.ends_with(';'), "got: {sql}");
    }
}
