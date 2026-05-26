//! `pgevolve-conformance` — fixture-driven deterministic test suite.
//!
//! Each fixture is a directory under `tests/cases/` containing
//! `before.sql`, `after.sql`, `fixture.toml`, and an `expected/` sub-tree.
//! See `docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md`
//! for the assertion model.

#![warn(missing_docs)]
#![deny(unsafe_code)]
// pgevolve-core's ParseError / CatalogError flow through this crate's
// Result types; see crates/pgevolve-core/src/lib.rs.
#![allow(clippy::result_large_err)]

pub mod assertions;
pub mod failure;
pub mod fixture;
pub mod normalize;
pub mod planning;
pub mod walk;

pub use fixture::{ExpectAdvisory, Fixture, FixtureError, FixtureExpect, FixtureMeta};
