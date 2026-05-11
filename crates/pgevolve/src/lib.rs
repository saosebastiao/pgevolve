//! pgevolve CLI internals — runtime executor, connection management, config
//! loading, command dispatch.
//!
//! The binary at `src/main.rs` is a thin wrapper around [`run_main`].

#![warn(missing_docs)]
#![deny(unsafe_code)]

pub mod cli;
pub mod commands;
pub mod config;
pub mod connection;
pub mod executor;
pub mod pg_querier;
pub mod shadow_pg;
pub mod target_identity;

pub use executor::{ApplyError, ApplyOutcome, ApplyOverrides, apply, bootstrap_metadata};
pub use target_identity::compute_target_identity;
