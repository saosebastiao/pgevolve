//! Errors raised by the source parser.

use std::path::PathBuf;

use thiserror::Error;

/// Position within a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// Path to the file (relative to the source root, when available).
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column.
    pub column: usize,
}

impl SourceLocation {
    /// Construct.
    pub const fn new(file: PathBuf, line: usize, column: usize) -> Self {
        Self { file, line, column }
    }
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.column)
    }
}

/// Errors raised by the source parser.
#[derive(Debug, Error)]
pub enum ParseError {
    /// I/O error while reading a source file.
    #[error("I/O error reading {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// `pg_query` rejected the SQL.
    #[error("pg_query parse error at {location}: {message}")]
    PgQuery {
        /// Source location.
        location: SourceLocation,
        /// Message from `pg_query`.
        message: String,
    },

    /// CREATE was for an object kind not supported in v0.1.
    #[error(
        "{location}: {kind} is not supported in pgevolve v0.1 — \
         see docs §2 for the v0.1 object-kind list; expected to land in a later phase"
    )]
    UnsupportedObjectKind {
        /// Source location.
        location: SourceLocation,
        /// Object kind name (e.g., "CREATE VIEW").
        kind: &'static str,
    },

    /// CREATE was missing required schema qualification and no `-- @pgevolve schema=...`
    /// directive applied.
    #[error(
        "{location}: object name must be schema-qualified, or the file must declare \
         `-- @pgevolve schema=<name>`"
    )]
    UnqualifiedName {
        /// Source location.
        location: SourceLocation,
    },

    /// Generic structural error during AST → IR conversion.
    #[error("{location}: {message}")]
    Structural {
        /// Source location.
        location: SourceLocation,
        /// Diagnostic message.
        message: String,
    },

    /// IR construction failed (e.g., invalid identifier in source).
    #[error("{location}: {source}")]
    Ir {
        /// Source location.
        location: SourceLocation,
        /// Underlying error.
        #[source]
        source: crate::ir::IrError,
    },

    /// A directive was malformed.
    #[error("{location}: invalid pgevolve directive: {message}")]
    InvalidDirective {
        /// Source location.
        location: SourceLocation,
        /// Diagnostic.
        message: String,
    },

    /// Two definitions of the same object qname were found.
    #[error("duplicate object {qname} defined at {first} and {second}")]
    DuplicateObject {
        /// Object qname (rendered).
        qname: String,
        /// First definition location.
        first: SourceLocation,
        /// Second definition location.
        second: SourceLocation,
    },

    /// One or more structural references could not be resolved in source IR.
    #[error("AST resolution failed:\n{}", format_resolution_errors(.0))]
    AstResolution(Vec<crate::parse::ast_resolution::AstResolutionError>),
}

fn format_resolution_errors(
    errs: &[crate::parse::ast_resolution::AstResolutionError],
) -> String {
    let mut s = String::new();
    for (i, e) in errs.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str("  - ");
        s.push_str(&e.to_string());
    }
    s
}
