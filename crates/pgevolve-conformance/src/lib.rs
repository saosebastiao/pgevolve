//! `pgevolve-conformance` — fixture-driven deterministic test suite.
//!
//! Each fixture is a directory under `tests/cases/` containing
//! `before.sql`, `after.sql`, `fixture.toml`, and an `expected/` sub-tree.
//! See `docs/superpowers/specs/2026-05-11-conformance-test-suite-design.md`
//! for the assertion model.

#![warn(missing_docs)]
#![deny(unsafe_code)]

pub mod assertions;
pub mod failure;
pub mod fixture;
pub mod normalize;
pub mod planning;
pub mod walk;

pub use fixture::{Fixture, FixtureError, FixtureExpect, FixtureMeta};
