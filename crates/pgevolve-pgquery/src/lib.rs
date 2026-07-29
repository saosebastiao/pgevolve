//! Parse SQL with the real Postgres grammar, and deparse it back.
//!
//! This crate statically links [`libpg_query`], which is the Postgres server's
//! own parser and deparser extracted into a standalone C library. Parsing is not
//! reimplemented here and never should be: the only implementation that agrees
//! with Postgres in every corner is Postgres.
//!
//! # Why this crate exists
//!
//! pgevolve previously depended on the `pg_query` crate. That crate is treated as
//! permanently unmaintained, and its API exposed problems pgevolve cannot carry:
//!
//! - Its `build.rs` regenerated a checked-in source file with `prost-build`
//!   whenever `protoc` was on `PATH`, writing into its own `src/` directory. For
//!   a dependency that lives in the Cargo registry cache, that mutates an
//!   immutable-by-contract directory. See `build.rs` for the reproduction.
//! - It shipped roughly 4,500 lines of generated node-walking machinery
//!   (`NodeRef`, `NodeMut`, `nodes()`, `truncate`) that pgevolve never called,
//!   including raw-pointer traversal in service of a query-truncation feature.
//!
//! What is kept here is what pgevolve uses and nothing else: [`parse`],
//! [`deparse`], [`parse_plpgsql`], the generated [`protobuf`] AST, and
//! [`NodeEnum::deparse`].
//!
//! # Vendored Postgres major
//!
//! One binding, tracking one Postgres major — see [`VENDORED_PG_MAJOR`]. A
//! binding per supported major was measured and rejected; the PG14-PG18
//! deparsers agree on pgevolve's entire fixture corpus, so per-major bindings
//! would multiply build cost to remove differences that are not there. Version
//! awareness lives in pgevolve's catalog readers and plan-time lints instead.
//!
//! [`libpg_query`]: https://github.com/pganalyze/libpg_query
//!
//! # Example
//!
//! ```
//! let result = pgevolve_pgquery::parse("SELECT 1").expect("parses");
//! assert_eq!(result.protobuf.stmts.len(), 1);
//! assert_eq!(
//!     pgevolve_pgquery::deparse(&result.protobuf).expect("deparses"),
//!     "SELECT 1"
//! );
//! ```

mod error;
mod ffi;
mod node;
mod parse_result;
mod query;

#[rustfmt::skip]
pub mod protobuf;

pub use error::{Error, Result};
pub use parse_result::ParseResult;
pub use query::{deparse, parse, parse_plpgsql};

pub use protobuf::Node;
/// The AST node payload: one variant per Postgres parse-node type.
///
/// This is `protobuf::node::Node` under a name that says what it is. The
/// generated type is called `Node` and nests inside a type also called `Node`,
/// which reads badly at every call site.
pub use protobuf::node::Node as NodeEnum;

/// The Postgres major version whose parser is vendored here.
///
/// Recorded as a constant rather than encoded in the crate version so that the
/// crate can follow the workspace release cadence. Read it in tests and
/// diagnostics; a mismatch against what a live server reports is a real signal.
pub const VENDORED_PG_MAJOR: u32 = 17;

/// The full `PG_VERSION_NUM` of the vendored parser, as `libpg_query` reports it.
///
/// The deparser embeds this in the [`ParseResult`] it round-trips through, so it
/// has to match the C library rather than being declared independently.
#[must_use]
pub const fn vendored_pg_version_num() -> i32 {
    // Widening a positive C constant; the value is ~170000.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "PG_VERSION_NUM is a small positive integer"
    )]
    {
        ffi::PG_VERSION_NUM as i32
    }
}

#[cfg(test)]
mod tests {
    use super::{VENDORED_PG_MAJOR, deparse, parse, vendored_pg_version_num};

    #[test]
    fn parses_and_deparses_a_round_trip() {
        let result = parse("CREATE TABLE app.users (id integer PRIMARY KEY)").expect("parses");
        assert_eq!(result.protobuf.stmts.len(), 1);
        let text = deparse(&result.protobuf).expect("deparses");

        // Note `integer` came back as `int`: the deparser canonicalizes type
        // names rather than echoing the input. That is the property pgevolve's
        // equality checks depend on — and the reason a deparse failure must be
        // an error rather than a fallback to the original text, since the
        // fallback would compare unequal to the same type written the other way.
        assert_eq!(text, "CREATE TABLE app.users (id int PRIMARY KEY)");
    }

    #[test]
    fn deparser_canonicalizes_standard_type_spellings_but_not_internal_aliases() {
        // Measured, not assumed. The deparser rewrites SQL-standard spellings to
        // Postgres's preferred short form, but leaves Postgres's own internal
        // aliases exactly as written:
        //
        //   integer                     -> int        but int4    -> int4
        //   character varying(10)       -> varchar(10)
        //   timestamp without time zone -> timestamp
        //   decimal(5,2)                -> numeric(5, 2)
        //   boolean                     -> boolean    and bool    -> bool
        //   double precision            -> double precision, float8 -> float8
        //
        // This is why deparsing is necessary but not sufficient for type
        // equality, and why pgevolve normalizes types into `ColumnType` instead
        // of comparing deparsed text. A change in either column below is a
        // change in what pgevolve must normalize.
        let converges = [
            ("character varying(10)", "varchar(10)"),
            ("timestamp without time zone", "timestamp"),
            ("decimal(5,2)", "numeric(5, 2)"),
            ("integer", "int"),
        ];
        for (written, expected) in converges {
            assert_eq!(
                deparsed_type(written),
                expected,
                "{written:?} should canonicalize to {expected:?}"
            );
        }

        // The counterexamples matter as much as the conversions: if any of these
        // start converging, pgevolve's own normalization has become redundant in
        // a way worth knowing about.
        for internal in ["int4", "bool", "float8"] {
            assert_eq!(
                deparsed_type(internal),
                internal,
                "{internal:?} is a Postgres-internal alias and is passed through"
            );
        }
    }

    /// Deparse `CREATE TABLE t (c <ty>)` and return just the rendered type.
    fn deparsed_type(ty: &str) -> String {
        let parsed = parse(&format!("CREATE TABLE t (c {ty})")).expect("parses");
        let text = deparse(&parsed.protobuf).expect("deparses");
        text.strip_prefix("CREATE TABLE t (c ")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or(&text)
            .to_owned()
    }

    #[test]
    fn reports_syntax_errors_with_a_message() {
        let err = parse("CREATE TABLE !bad!").expect_err("malformed SQL is rejected");
        assert!(
            format!("{err}").contains("syntax error"),
            "expected a syntax error, got: {err}"
        );
    }

    #[test]
    fn vendored_version_constant_matches_the_c_library() {
        // Guards against the constant drifting from the vendored tree after a
        // Postgres major bump — the whole point of recording it.
        let num = vendored_pg_version_num();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "PG_VERSION_NUM is a small positive integer"
        )]
        let major = (num / 10_000) as u32;
        assert_eq!(
            major, VENDORED_PG_MAJOR,
            "VENDORED_PG_MAJOR says {VENDORED_PG_MAJOR} but the vendored C \
             library reports PG_VERSION_NUM {num}"
        );
    }

    #[test]
    fn multi_statement_input_yields_one_entry_per_statement() {
        let result = parse("CREATE SCHEMA a; CREATE SCHEMA b;").expect("parses");
        assert_eq!(result.protobuf.stmts.len(), 2);
    }
}
