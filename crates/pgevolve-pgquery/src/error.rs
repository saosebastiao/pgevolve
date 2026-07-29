//! Errors raised by this crate.

use thiserror::Error;

/// What can go wrong parsing or deparsing SQL.
///
/// Narrower than upstream `pg_query::Error`: the `Scan` and `Split` variants are
/// gone with the `scan`/`split` entry points they belonged to, so there is no
/// variant here that cannot actually be produced.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum Error {
    /// The SQL contained an interior NUL byte and cannot cross the C boundary.
    ///
    /// Distinct from [`Self::Parse`]: nothing was parsed, because the string was
    /// never handed to the parser.
    #[error("statement contains an interior NUL byte: {0}")]
    Conversion(#[from] std::ffi::NulError),

    /// The C library returned a protobuf payload that did not decode.
    ///
    /// This is a bug in this crate or a mismatch between the vendored `.proto`
    /// and the checked-in generated module, not bad input.
    #[error("could not decode the parser's protobuf output: {0}")]
    Decode(#[from] prost::DecodeError),

    /// The parser or deparser rejected the input, with Postgres's own message.
    #[error("{0}")]
    Parse(String),

    /// The plpgsql analyzer returned JSON that did not parse.
    #[error("could not parse the plpgsql analyzer's JSON output: {0}")]
    InvalidJson(String),
}

/// Convenience alias for this crate's `Result`.
pub type Result<T> = core::result::Result<T, Error>;
