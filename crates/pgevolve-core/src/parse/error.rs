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

    /// AST canonicalization pass failed (view body normalization or reference
    /// resolution error).
    #[error("{0}")]
    AstCanon(crate::parse::ast_canon::AstCanonError),

    /// A string could not be parsed as a valid identifier.
    #[error("invalid identifier {0:?}: {1}")]
    InvalidIdentifier(String, String),

    // ── Publication parse errors ─────────────────────────────────────────────
    /// A publication with this name was declared more than once.
    #[error("{1}: publication {0:?} declared more than once")]
    DuplicatePublication(crate::identifier::Identifier, SourceLocation),

    /// `FOR ALL TABLES` was combined with explicit object specs.
    #[error(
        "{1}: publication {0:?}: FOR ALL TABLES cannot be combined with \
         FOR TABLE or FOR TABLES IN SCHEMA"
    )]
    PublicationAllTablesWithObjects(crate::identifier::Identifier, SourceLocation),

    /// A publication object spec node had an unexpected shape.
    #[error("{1}: publication {0:?}: malformed publication object spec node")]
    PublicationObjectMalformed(crate::identifier::Identifier, SourceLocation),

    /// `FOR TABLES IN CURRENT SCHEMA` is not declarative — pgevolve rejects it.
    #[error(
        "{1}: publication {0:?}: FOR TABLES IN CURRENT SCHEMA is not supported \
         (not declarative; use explicit schema names)"
    )]
    PublicationCurrentSchemaForm(crate::identifier::Identifier, SourceLocation),

    /// An unrecognized `PublicationObjSpecType` integer was encountered.
    #[error(
        "{2}: publication {1:?}: unknown publication object type {0} \
         (expected 1=TABLE, 2=TABLES IN SCHEMA, 3=CUR_SCHEMA)"
    )]
    UnknownPublicationObjectType(i32, crate::identifier::Identifier, SourceLocation),

    /// A table in a publication was not schema-qualified.
    #[error("{1}: publication {0:?}: table must be schema-qualified")]
    UnqualifiedPublicationTable(crate::identifier::Identifier, SourceLocation),

    /// Failed to parse the row filter expression for a published table.
    #[error("{3}: publication {0:?} table {1}: row filter parse error: {2}")]
    PublicationFilterParse(
        crate::identifier::Identifier,
        crate::identifier::QualifiedName,
        String,
        SourceLocation,
    ),

    /// A `WITH (...)` option node for a publication had an unexpected shape.
    #[error("{1}: publication {0:?}: malformed publication option node")]
    PublicationOptionMalformed(crate::identifier::Identifier, SourceLocation),

    /// An unrecognized key appeared in a publication `WITH (...)` clause.
    #[error("{2}: publication {1:?}: unknown publication option {0:?}")]
    UnknownPublicationOption(String, crate::identifier::Identifier, SourceLocation),

    /// An unrecognized value appeared in a `publish = '...'` clause.
    #[error(
        "{2}: publication {1:?}: unknown publish kind {0:?} \
         (valid: insert, update, delete, truncate)"
    )]
    UnknownPublishKind(String, crate::identifier::Identifier, SourceLocation),

    /// A `publish = '...'` clause was empty (no DML kinds enabled).
    #[error("{1}: publication {0:?}: empty publish list — at least one DML kind required")]
    EmptyPublishBitset(crate::identifier::Identifier, SourceLocation),

    /// A `CREATE PUBLICATION p` had no scope clause (empty selective).
    #[error(
        "{1}: publication {0:?}: no scope clause — add FOR ALL TABLES, \
         FOR TABLE ..., or FOR TABLES IN SCHEMA ..."
    )]
    EmptyPublicationScope(crate::identifier::Identifier, SourceLocation),

    /// `ALTER PUBLICATION p RENAME TO q` is not supported.
    #[error("{1}: publication {0:?}: RENAME is not supported in pgevolve")]
    PublicationRenameNotSupported(crate::identifier::Identifier, SourceLocation),

    /// `ALTER PUBLICATION p ...` appeared before the matching `CREATE PUBLICATION p`.
    #[error("{1}: publication {0:?}: ALTER PUBLICATION before CREATE PUBLICATION")]
    AlterPublicationBeforeCreate(crate::identifier::Identifier, SourceLocation),
}

fn format_resolution_errors(errs: &[crate::parse::ast_resolution::AstResolutionError]) -> String {
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
