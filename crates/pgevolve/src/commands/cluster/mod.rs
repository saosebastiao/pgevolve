//! Cluster-level command implementations.
//!
//! Each module corresponds to one `pgevolve cluster <subcommand>`:
//!
//! - [`init`] — scaffold a new cluster project.
//! - [`diff`] — show the diff between source and live cluster.
//! - [`plan`] — produce a `cluster-plans/<id>/` directory.
//! - [`apply`] — apply a cluster plan directory.
//! - [`status`] — list cluster plans.
//!
//! Stage 10 of `docs/superpowers/plans/2026-05-21-cluster-roles.md`.

pub mod apply;
pub mod diff;
pub mod init;
pub mod plan;
pub mod status;
