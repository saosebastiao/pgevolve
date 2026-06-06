//! Cluster-level IR — objects that live above the per-database surface.
//!
//! Currently holds roles and tablespaces. Cluster settings, foreign servers,
//! user mappings, and the databases list are deferred to follow-up sub-specs.
//! See `docs/superpowers/specs/2026-05-21-cluster-roles-design.md` and
//! `docs/superpowers/specs/2026-05-15-v0.2-architecture-review-design.md` §17.

pub mod catalog;
pub mod role;
pub mod tablespace;

pub use catalog::ClusterCatalog;
pub use role::{Role, RoleAttributes};
pub use tablespace::Tablespace;
