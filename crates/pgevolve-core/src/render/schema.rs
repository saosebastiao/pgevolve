//! Schema renderer.

use crate::ir::schema::Schema;
use crate::plan::rewrite::sql as rewrite_sql;

/// Render a `Schema` as SQL.
///
/// Emits `CREATE SCHEMA <name>;` followed by an optional
/// `COMMENT ON SCHEMA <name> IS '...';`.
#[must_use]
pub fn render_schema(s: &Schema) -> String {
    let mut out = rewrite_sql::create_schema(s);
    out.push('\n');
    if let Some(comment) = &s.comment {
        out.push_str(&rewrite_sql::comment_on_schema(
            &s.name,
            Some(comment.as_str()),
        ));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::schema::Schema;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn plain_schema_renders() {
        let s = Schema::new(id("app"));
        let sql = render_schema(&s);
        assert!(sql.contains("CREATE SCHEMA app;"));
        assert!(!sql.contains("COMMENT"));
        let r = pg_query::parse(sql.trim());
        assert!(r.is_ok(), "pg_query rejected: {sql:?}\nerr: {r:?}");
    }

    #[test]
    fn schema_with_comment_renders() {
        let s = Schema {
            name: id("billing"),
            comment: Some("billing namespace".into()),
            owner: None,
            grants: vec![],
        };
        let sql = render_schema(&s);
        assert!(sql.contains("CREATE SCHEMA billing;"));
        assert!(sql.contains("COMMENT ON SCHEMA billing IS 'billing namespace';"));

        let r = pg_query::parse(&sql);
        assert!(r.is_ok(), "pg_query rejected SQL:\n{sql}\nerr: {r:?}");
    }

    #[test]
    fn quoted_schema_name_renders() {
        // Quoted names must be preserved.
        let s = Schema::new(Identifier::from_quoted("MySchema").unwrap());
        let sql = render_schema(&s);
        assert!(
            sql.contains("\"MySchema\""),
            "expected quoted name in: {sql}"
        );
        let r = pg_query::parse(&sql);
        assert!(r.is_ok(), "pg_query rejected SQL:\n{sql}\nerr: {r:?}");
    }
}
