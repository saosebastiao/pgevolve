//! pgevolve CLI internals — runtime executor, connection management, config
//! loading, command dispatch.
//!
//! The binary at `src/main.rs` is a thin wrapper that dispatches CLI commands.

#![warn(missing_docs)]
#![deny(unsafe_code)]
// pgevolve-core's ParseError / CatalogError flow through this crate's
// Result types; see the matching allow in crates/pgevolve-core/src/lib.rs.
#![allow(clippy::result_large_err)]

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
pub use api::{ClusterPlan, ClusterPlanError, build_cluster_plan};
pub use executor::{
    ApplyError, ApplyOutcome, ApplyOverrides, apply, apply_plan, bootstrap_metadata,
};
pub use executor::{ClusterApplyError, apply_cluster_plan, apply_cluster_plan_dir};
pub use target_identity::compute_target_identity;
