//! `pgevolve-testkit` — internal test infrastructure for the pgevolve workspace.
//!
//! Consumed only as a `dev-dependency`. Provides ephemeral Postgres
//! provisioning, IR generators, equivalence asserters, and end-to-end
//! harnesses for property and chaos testing.
#![warn(missing_docs)]
#![deny(unsafe_code)]

pub mod catalog_snapshotter;
pub mod ephemeral_pg;
pub mod equivalence_asserter;
pub mod ir_generator;
pub mod ir_mutator;
pub mod migration_fixture;
pub mod pg_querier;
pub mod test_pg_backend;

pub use ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
pub use equivalence_asserter::assert_canonical_eq;
pub use ir_generator::{
    IRGeneratorConfig, arbitrary_catalog, arbitrary_column_type, arbitrary_view_catalog,
};
pub use ir_mutator::arbitrary_mutation;
pub use migration_fixture::MigrationFixture;
pub use pg_querier::PgCatalogQuerier;

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
