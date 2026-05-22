//! Cluster-side rewrite pass: translate [`ClusterChangeSet`] → `Vec<RawStep>`.
//!
//! Mirrors `plan::rewrite` for per-DB ops. `sql` holds the DDL renderers;
//! `emit` turns each [`crate::diff::cluster::ClusterChange`] into a
//! [`crate::plan::raw_step::RawStep`].
//!
//! [`ClusterChangeSet`]: crate::diff::cluster::ClusterChangeSet

pub mod emit;
pub mod sql;

pub use emit::emit_cluster_changes;
