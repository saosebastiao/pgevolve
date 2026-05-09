//! Top-level error types for `pgevolve-core`.
//!
//! Per-phase error variants (parse, catalog, diff, plan) are added by their
//! respective modules. This file declares the umbrella type and re-exports.

use thiserror::Error;

/// Top-level error type. Each variant carries the typed error from one phase.
#[derive(Debug, Error)]
pub enum Error {
    /// IR-construction error (e.g., invalid identifier).
    #[error(transparent)]
    Ir(#[from] crate::ir::IrError),
    /// Source-parser error.
    #[error(transparent)]
    Parse(#[from] crate::parse::ParseError),
    // Catalog, Diff, Plan variants added by later phases.
}

/// Result alias for crate-level operations.
pub type Result<T> = std::result::Result<T, Error>;
