//! pgevolve CLI internals — runtime executor, connection management, and
//! supporting helpers. The binary at `src/main.rs` is a thin wrapper.
//!
//! Spec §8 covers the apply flow; see the [`executor`] module.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod executor;
pub mod target_identity;

pub use executor::{apply, bootstrap_metadata, ApplyError, ApplyOutcome, ApplyOverrides};
pub use target_identity::compute_target_identity;
