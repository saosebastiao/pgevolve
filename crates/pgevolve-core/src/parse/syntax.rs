//! Syntax-only checks against the SQL parser.
//!
//! Render-side tests assert that everything pgevolve emits is SQL Postgres will
//! accept. They used to call the parser crate directly, which put a parser
//! import in `render/` — 18 of them, enough that swapping the parser binding
//! would have meant editing files that have nothing to do with parsing. The
//! checks only ever needed a yes/no answer plus a message, so that is all this
//! module hands back.

/// Verify that `sql` is accepted by the SQL parser.
///
/// `sql` may contain multiple statements; the parser accepts a whole block, so
/// callers rendering a full catalog can pass it in one piece.
///
/// On rejection the parser's own message is returned as a `String` rather than
/// its error type — keeping the parser out of every caller's signature is the
/// entire point of this module.
pub fn check(sql: &str) -> Result<(), String> {
    pg_query::parse(sql).map(|_| ()).map_err(|e| e.to_string())
}

/// Parse `sql` and report how many top-level statements it contained.
///
/// Distinct from [`check`] because "parses" and "parses into something" are
/// different assertions: a blank string parses to zero statements.
pub fn statement_count(sql: &str) -> Result<usize, String> {
    pg_query::parse(sql)
        .map(|r| r.protobuf.stmts.len())
        .map_err(|e| e.to_string())
}

/// Assert that `sql` parses, panicking with the offending SQL and the parser's
/// message when it does not.
///
/// The three-line `let r = …; assert!(r.is_ok(), …)` dance appeared verbatim at
/// eight render-test call sites; this is that dance, named.
#[track_caller]
pub fn assert_parses(sql: &str) {
    if let Err(message) = check(sql) {
        panic!("parser rejected rendered SQL:\n{sql}\n\nerror: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::{assert_parses, check, statement_count};

    #[test]
    fn accepts_a_create_table() {
        assert_parses("CREATE TABLE app.users (id integer);");
    }

    #[test]
    fn reports_syntax_errors() {
        let err = check("CREATE TABLE !bad!;").expect_err("malformed SQL is rejected");
        assert!(!err.is_empty(), "rejection carries a message");
    }

    #[test]
    fn counts_top_level_statements() {
        assert_eq!(
            statement_count("CREATE SCHEMA a; CREATE SCHEMA b;").expect("parses"),
            2
        );
        assert_eq!(statement_count("").expect("empty input parses"), 0);
    }
}
