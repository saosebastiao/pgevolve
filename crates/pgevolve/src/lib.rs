//! pgevolve CLI internals — runtime executor, connection management, config
//! loading, command dispatch.
//!
//! The binary at `src/main.rs` is a thin wrapper that dispatches CLI commands.

#![warn(missing_docs)]
#![deny(unsafe_code)]

pub mod api;
pub mod cli;
pub mod cluster_config;
pub mod commands;
pub mod config;
pub mod connection;
pub mod executor;
pub mod pg_querier;
pub mod shadow;
pub mod target_identity;

pub use api::{BuildPlanError, BuildPlanOptions, build_plan};
pub use executor::{
    ApplyError, ApplyOutcome, ApplyOverrides, apply, apply_plan, bootstrap_metadata,
};
pub use target_identity::compute_target_identity;
