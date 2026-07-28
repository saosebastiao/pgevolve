//! Errors raised by the catalog reader.

use thiserror::Error;

use crate::catalog::CatalogQuery;
use crate::ir::IrError;
use crate::parse::ParseError;

/// Errors raised by the catalog reader.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// The querier returned an error.
    #[error("catalog query {query:?} failed: {message}")]
    QueryFailed {
        /// Which query failed.
        query: CatalogQuery,
        /// Adapter-supplied message.
        message: String,
    },

    /// The querier returned no rows for a query that requires at least one.
    #[error("catalog query {query:?} returned no rows")]
    MissingResult {
        /// Which query produced the empty result.
        query: CatalogQuery,
    },

    /// A column expected on a [`crate::catalog::rows::Row`] was missing.
    #[error("catalog row missing column {column:?} for query {query:?}")]
    MissingColumn {
        /// Which query produced the row.
        query: CatalogQuery,
        /// Column name.
        column: String,
    },

    /// A column had an unexpected SQL type.
    #[error("catalog row column {column:?} had unexpected type for query {query:?}: {message}")]
    BadColumnType {
        /// Which query produced the row.
        query: CatalogQuery,
        /// Column name.
        column: String,
        /// Description of the mismatch.
        message: String,
    },

    /// Postgres reported a major version we do not (yet) support.
    #[error(
        "unsupported Postgres major version: {0} (supported: {supported})",
        supported = crate::catalog::version::PgVersion::SUPPORTED_LIST
    )]
    UnsupportedPgVersion(u32),

    /// The configured managed-schema list named a reserved schema we never manage.
    #[error("schema {0:?} is reserved and cannot be managed by pgevolve")]
    CannotManageReservedSchema(String),

    /// A configured ignore glob was syntactically invalid.
    #[error("invalid ignore glob {0:?}: {1}")]
    InvalidIgnoreGlob(String, glob::PatternError),

    /// IR construction failed while assembling rows into [`crate::ir::catalog::Catalog`].
    #[error("IR error while assembling catalog: {0}")]
    Ir(#[from] IrError),

    /// A `pg_get_constraintdef`/`pg_get_indexdef`/default expression failed to parse.
    #[error("re-parsing introspected SQL fragment failed: {0}")]
    ReparseFailed(#[from] Box<ParseError>),

    /// A catalog row referenced an object oid that no other query produced.
    #[error("catalog assembly: dangling reference {kind} for {what}")]
    DanglingReference {
        /// What kind of reference (e.g., "table for column").
        kind: &'static str,
        /// Identifier or oid of the missing object.
        what: String,
    },

    /// A catalog column holding a closed set of single-character codes carried
    /// a code we do not recognise.
    ///
    /// Postgres adds codes to these columns in new majors (`attgenerated` gained
    /// `'v'` in PG 18). Decoding them exhaustively — rather than testing for the
    /// one code we happen to care about — is what turns "a future Postgres
    /// silently produces wrong IR" into a named error.
    #[error("catalog column {column:?} for {object} carried unrecognised code {value:?}")]
    UnknownCatalogCode {
        /// The `pg_catalog` column the code came from (e.g., `attgenerated`).
        column: &'static str,
        /// The object the row described, for operator legibility.
        object: String,
        /// The code we could not decode.
        value: String,
    },

    /// The server described an object using a feature pgevolve can read but
    /// cannot yet represent in its IR.
    ///
    /// Refusing is mandatory here: emitting partial IR for an object whose
    /// definition we only half-understand produces a plan that silently drops
    /// or rewrites the part we missed.
    #[error(
        "{object}: {feature} is not yet supported by pgevolve (tracked in {tracking}); \
         refusing to introspect rather than emit incomplete state"
    )]
    UnsupportedFeature {
        /// The object carrying the feature.
        object: String,
        /// Human-readable feature name (e.g., `VIRTUAL generated columns`).
        feature: &'static str,
        /// Where the work is tracked, so the message is actionable.
        tracking: &'static str,
    },

    /// A server-emitted definition (`pg_get_constraintdef`, `pg_get_viewdef`, …)
    /// could not be re-parsed.
    ///
    /// Distinct from [`Self::ReparseFailed`]: this carries the object and the
    /// offending text so an operator can see *which* definition defeated us,
    /// which matters because these paths previously substituted placeholder
    /// data and continued.
    #[error("{object}: could not re-parse server-emitted {kind}: {def:?}")]
    UnparseableDefinition {
        /// The object whose definition failed to parse.
        object: String,
        /// Which `pg_get_*def` produced it (e.g., `pg_get_constraintdef`).
        kind: &'static str,
        /// The definition text, for diagnosis.
        def: String,
    },
}
