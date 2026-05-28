//! Re-export the shared `tokio_postgres`-backed catalog querier from
//! `pgevolve-core`. Enabled via the `tokio-postgres-querier` feature.
//!
//! See [`pgevolve_core::catalog::pg_querier`] for the implementation.

pub use pgevolve_core::catalog::pg_querier::{NoRuntimeError, PgCatalogQuerier};
