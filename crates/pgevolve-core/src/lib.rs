//! `pgevolve-core` — the declarative-schema-management engine.
//!
//! This crate is I/O-free: it accepts source SQL bytes and a `CatalogQuerier`
//! implementation from callers, and returns IR, diffs, and plans as data.
//! See the workspace `docs/superpowers/specs/` for the design.
#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod catalog;
pub mod error;
pub mod identifier;
pub mod ir;
pub mod parse;

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
