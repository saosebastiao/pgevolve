//! Sequence renderer.

use crate::ir::sequence::Sequence;
use crate::plan::rewrite::sql as rewrite_sql;

/// Render a `Sequence` as SQL.
///
/// Emits `CREATE SEQUENCE <qname> AS <type> ... ;` followed by an optional
/// `COMMENT ON SEQUENCE ...` statement.
#[must_use]
pub fn render_sequence(s: &Sequence) -> String {
    let mut out = rewrite_sql::create_sequence(s);
    out.push('\n');
    if let Some(comment) = &s.comment {
        out.push_str(&rewrite_sql::comment_on_sequence(
            &s.qname,
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
    use crate::ir::column_type::ColumnType;
    use crate::ir::sequence::{Sequence, SequenceOwner};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn assert_pg_parseable(sql: &str) {
        let r = pg_query::parse(sql);
        assert!(r.is_ok(), "pg_query rejected SQL:\n{sql}\nerr: {r:?}");
    }

    fn base_seq() -> Sequence {
        Sequence {
            qname: qn("app", "users_id_seq"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn plain_sequence_renders() {
        let sql = render_sequence(&base_seq());
        assert!(sql.contains("CREATE SEQUENCE app.users_id_seq"));
        assert!(sql.contains("AS bigint"));
        assert!(!sql.contains("COMMENT"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn sequence_with_min_max_renders() {
        let seq = Sequence {
            min_value: Some(1),
            max_value: Some(9_999_999),
            ..base_seq()
        };
        let sql = render_sequence(&seq);
        assert!(sql.contains("MINVALUE 1"));
        assert!(sql.contains("MAXVALUE 9999999"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn sequence_with_no_minmax_renders() {
        let sql = render_sequence(&base_seq());
        assert!(sql.contains("NO MINVALUE"));
        assert!(sql.contains("NO MAXVALUE"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn cycling_sequence_renders() {
        let seq = Sequence {
            cycle: true,
            ..base_seq()
        };
        let sql = render_sequence(&seq);
        assert!(sql.contains(" CYCLE"));
        assert!(!sql.contains("NO CYCLE"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn sequence_with_owned_by_renders() {
        let seq = Sequence {
            owned_by: Some(SequenceOwner {
                table: qn("app", "users"),
                column: id("id"),
            }),
            ..base_seq()
        };
        let sql = render_sequence(&seq);
        assert!(sql.contains("OWNED BY app.users.id"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn sequence_with_comment_renders() {
        let seq = Sequence {
            comment: Some("primary key sequence".into()),
            ..base_seq()
        };
        let sql = render_sequence(&seq);
        assert!(sql.contains("COMMENT ON SEQUENCE app.users_id_seq IS 'primary key sequence';"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn smallint_sequence_renders() {
        let seq = Sequence {
            data_type: ColumnType::SmallInt,
            ..base_seq()
        };
        let sql = render_sequence(&seq);
        assert!(sql.contains("AS smallint"));
        assert_pg_parseable(&sql);
    }

    #[test]
    fn integer_sequence_renders() {
        let seq = Sequence {
            data_type: ColumnType::Integer,
            start: 100,
            increment: 10,
            cache: 5,
            ..base_seq()
        };
        let sql = render_sequence(&seq);
        assert!(sql.contains("AS integer"));
        assert!(sql.contains("INCREMENT BY 10"));
        assert!(sql.contains("START WITH 100"));
        assert!(sql.contains("CACHE 5"));
        assert_pg_parseable(&sql);
    }
}
