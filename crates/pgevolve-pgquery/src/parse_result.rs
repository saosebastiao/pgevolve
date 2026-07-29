//! The result of a successful parse.

use crate::error::Result;
use crate::protobuf;

/// A parsed statement batch, plus anything the parser wrote to stderr.
///
/// Much smaller than upstream's `ParseResult`, which eagerly walked the whole
/// tree on construction to populate `tables`, `aliases`, `cte_names`,
/// `functions`, and `filter_columns`. pgevolve reads none of those — it walks the
/// AST itself, because it needs schema-qualified identities rather than the bare
/// relation names that walk produced — so the walk was pure cost on every parse.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// Postgres's parse tree.
    pub protobuf: protobuf::ParseResult,

    /// Warnings the parser wrote to stderr, one per line.
    ///
    /// Retained because a parse that *succeeds with a warning* is exactly the
    /// case where silently dropping the warning would hide something.
    pub warnings: Vec<String>,
}

impl ParseResult {
    /// Build a result from a decoded tree and the parser's raw stderr text.
    pub(crate) fn new(protobuf: protobuf::ParseResult, stderr: &str) -> Self {
        let warnings = stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect();
        Self { protobuf, warnings }
    }

    /// Render the whole batch back into canonical SQL.
    pub fn deparse(&self) -> Result<String> {
        crate::query::deparse(&self.protobuf)
    }
}
