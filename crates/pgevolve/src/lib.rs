//! pgevolve CLI internals — runtime executor, connection management, config
//! loading, command dispatch.
//!
//! The binary at `src/main.rs` is a thin wrapper around [`run_main`].

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod cli;
pub mod commands;
pub mod config;
pub mod connection;
pub mod executor;
pub mod pg_querier;
pub mod target_identity;

pub use executor::{apply, bootstrap_metadata, ApplyError, ApplyOutcome, ApplyOverrides};
pub use target_identity::compute_target_identity;
