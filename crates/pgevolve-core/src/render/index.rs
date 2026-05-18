//! Index renderer.

use crate::ir::index::Index;
use crate::plan::rewrite::sql as rewrite_sql;

/// Render an `Index` as SQL.
///
/// Emits `CREATE [UNIQUE] INDEX <name> ON <table> USING <method> (...) ...;`
/// followed by an optional `COMMENT ON INDEX ...` statement.
///
/// Note: the schema-qualified index name (not the table-schema-local short name)
/// is used because indexes live in the same schema as their table in Postgres.
#[must_use]
pub fn render_index(i: &Index) -> String {
    let mut out = rewrite_sql::create_index(i, false);
    out.push('\n');
    if let Some(comment) = &i.comment {
        out.push_str(&rewrite_sql::comment_on_index(
            &i.qname,
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
    use crate::identifier::QualifiedName;
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn col(name: &str) -> IndexColumn {
        IndexColumn {
            expr: IndexColumnExpr::Column(id(name)),
            collation: None,
            opclass: None,
            sort_order: SortOrder::Asc,
            nulls_order: NullsOrder::NullsLast,
        }
    }

    fn assert_pg_parseable(sql: &str) {
        let r = pg_query::parse(sql);
        assert!(r.is_ok(), "pg_query rejected SQL:\n{sql}\nerr: {r:?}");
    }

    fn base_index() -> Index {
        Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![col("email")],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        }
    }

    #[test]
    fn plain_index_renders() {
        let sql = render_index(&base_index());
        assert!(sql.contains("CREATE INDEX users_email_idx"));
        assert!(sql.contains("ON app.users"));
        assert!(sql.contains("USING btree"));
        assert!(sql.contains("(email)"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn unique_index_renders() {
        let idx = Index {
            unique: true,
            ..base_index()
        };
        let sql = render_index(&idx);
        assert!(sql.contains("CREATE UNIQUE INDEX"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn index_with_include_renders() {
        let idx = Index {
            include: vec![id("created_at")],
            ..base_index()
        };
        let sql = render_index(&idx);
        assert!(sql.contains("INCLUDE (created_at)"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn partial_index_renders() {
        let idx = Index {
            predicate: Some(NormalizedExpr::from_text("deleted_at is null")),
            ..base_index()
        };
        let sql = render_index(&idx);
        assert!(sql.contains("WHERE deleted_at is null"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn index_with_desc_sort_renders() {
        let idx = Index {
            columns: vec![IndexColumn {
                sort_order: SortOrder::Desc,
                ..col("name")
            }],
            ..base_index()
        };
        let sql = render_index(&idx);
        assert!(sql.contains("DESC"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn index_with_comment_renders() {
        let idx = Index {
            comment: Some("user lookup index".into()),
            ..base_index()
        };
        let sql = render_index(&idx);
        assert!(
            sql.contains("COMMENT ON INDEX"),
            "expected COMMENT ON INDEX"
        );
        assert_pg_parseable(&sql);
    }

    #[test]
    fn expression_index_renders() {
        let idx = Index {
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Expression(NormalizedExpr::from_text("lower(email)")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            ..base_index()
        };
        let sql = render_index(&idx);
        assert!(sql.contains("(lower(email))"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn all_methods_render() {
        for method in [
            IndexMethod::BTree,
            IndexMethod::Hash,
            IndexMethod::Gin,
            IndexMethod::Gist,
            IndexMethod::Brin,
            IndexMethod::Spgist,
        ] {
            let idx = Index {
                method,
                ..base_index()
            };
            let sql = render_index(&idx);
            assert!(sql.contains("USING "));
            // All methods should produce parseable SQL (pg_query accepts all).
            assert_pg_parseable(&sql);
        }
    }
}
