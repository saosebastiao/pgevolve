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
    #[error("unsupported Postgres major version: {0} (supported: 14, 15, 16, 17)")]
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
}
