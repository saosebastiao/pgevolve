//! `pgevolve-core` — the declarative-schema-management engine.
//!
//! This crate performs no network I/O: live-database access is injected by the
//! caller via a `CatalogQuerier` implementation, and the crate returns IR,
//! diffs, and plans as data. (It does read source `.sql` files from disk in
//! [`parse::parse_directory`].) See the workspace `docs/superpowers/specs/` for
//! the design.
#![warn(missing_docs)]
#![deny(unsafe_code)]
// ParseError + CatalogError carry rich SourceLocation / QualifiedName context
// in many variants, which grows the enum past clippy's 128-byte
// `result_large_err` threshold. Boxing every error site would obscure the
// error-handling code with `Box<…>::new(…)` noise without changing the
// observable behavior — error paths are off the hot path. Allow crate-
// wide rather than scattering `#![allow]` annotations across every parse /
// catalog file.
#![allow(clippy::result_large_err)]

pub mod catalog;
pub mod diff;
pub mod error;
pub mod identifier;
pub mod ir;
pub mod lint;
pub mod parse;
pub mod plan;
pub mod render;

pub use crate::parse::normalize_body::{BodyError, NormalizedBody};

/// Crate version, exposed for embedding in plan manifests.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_nonempty() {
        assert!(!VERSION.is_empty());
    }
}
