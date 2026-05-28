//! Re-export the shared `tokio_postgres`-backed [`CatalogQuerier`] adapter
//! from `pgevolve-core`. Enabled via the `tokio-postgres-querier` feature.

pub use pgevolve_core::catalog::pg_querier::{NoRuntimeError, PgCatalogQuerier};
