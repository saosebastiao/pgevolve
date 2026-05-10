//! Catalog reader: live Postgres `pg_catalog` → [`crate::ir::catalog::Catalog`].
//!
//! The reader is split into:
//!
//! - [`CatalogQuerier`] — a sync, driver-agnostic trait. Adapters (the binary
//!   uses `tokio-postgres`) execute parameterized SQL and return [`rows::Row`]
//!   values.
//! - Per-version SQL strings in [`queries`].
//! - [`filter::CatalogFilter`] — managed-schema list + ignore globs.
//! - [`read_catalog`] — top-level entry point that orchestrates the queries
//!   and assembles their rows into IR.

pub mod error;
pub mod filter;
pub mod queries;
pub mod rows;
pub mod version;

pub use error::CatalogError;
pub use filter::CatalogFilter;
pub use rows::{Row, Value};
pub use version::PgVersion;

mod assemble;

use crate::ir::catalog::Catalog;

/// Identifier for each catalog query the reader runs. Adapters dispatch on
/// this enum to pick the per-version SQL string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CatalogQuery {
    /// `SHOW server_version_num`.
    PgVersion,
    /// `pg_namespace` rows for managed schemas.
    Schemas,
    /// `pg_class` (relkind='r') for managed tables.
    Tables,
    /// `pg_attribute` joined with `pg_attrdef`/`pg_type` for managed tables.
    Columns,
    /// `pg_constraint` for managed tables (PK/UNIQUE/FK/CHECK).
    Constraints,
    /// `pg_index` for managed tables (excluding constraint-backing indexes).
    Indexes,
    /// `pg_class` (relkind='S') joined with `pg_sequence`.
    Sequences,
    /// `pg_description` (currently inlined into the per-object queries).
    Comments,
    /// `pg_depend` rows linking sequences to their owning columns.
    Dependencies,
}

/// Sync, driver-agnostic catalog query interface.
///
/// Interface implemented by callers (typically the binary) to execute catalog
/// queries against a live database. Implementations are expected to be sync —
/// async drivers can wrap their runtime in [`fetch`](Self::fetch).
pub trait CatalogQuerier {
    /// Execute the named query with the supplied schema-list parameter (when
    /// applicable; the [`CatalogQuery::PgVersion`] query takes no parameters).
    fn fetch(
        &self,
        query: CatalogQuery,
        managed_schemas: &[&str],
    ) -> Result<Vec<Row>, CatalogError>;
}

/// Read every catalog query, assemble the IR, and canonicalize.
pub fn read_catalog(
    querier: &dyn CatalogQuerier,
    filter: &CatalogFilter,
) -> Result<Catalog, CatalogError> {
    let version = PgVersion::detect(querier)?;
    let managed: Vec<&str> = filter.managed_schemas_param();

    let schemas_rows = querier.fetch(CatalogQuery::Schemas, &managed)?;
    let tables_rows = querier.fetch(CatalogQuery::Tables, &managed)?;
    let columns_rows = querier.fetch(CatalogQuery::Columns, &managed)?;
    let constraints_rows = querier.fetch(CatalogQuery::Constraints, &managed)?;
    let indexes_rows = querier.fetch(CatalogQuery::Indexes, &managed)?;
    let sequences_rows = querier.fetch(CatalogQuery::Sequences, &managed)?;
    let dependencies_rows = querier.fetch(CatalogQuery::Dependencies, &managed)?;

    let raw = assemble::RawRows {
        version,
        schemas: schemas_rows,
        tables: tables_rows,
        columns: columns_rows,
        constraints: constraints_rows,
        indexes: indexes_rows,
        sequences: sequences_rows,
        dependencies: dependencies_rows,
    };
    let catalog = assemble::assemble(raw, filter)?;
    Ok(catalog.canonicalize()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Mock querier that returns canned rows by query name.
    struct MockQuerier {
        rows: RefCell<HashMap<CatalogQuery, Vec<Row>>>,
    }

    impl MockQuerier {
        fn new() -> Self {
            Self {
                rows: RefCell::new(HashMap::new()),
            }
        }
        fn set(&self, q: CatalogQuery, rows: Vec<Row>) {
            self.rows.borrow_mut().insert(q, rows);
        }
    }

    impl CatalogQuerier for MockQuerier {
        fn fetch(&self, q: CatalogQuery, _: &[&str]) -> Result<Vec<Row>, CatalogError> {
            Ok(self.rows.borrow().get(&q).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn empty_catalog_round_trips() {
        let m = MockQuerier::new();
        m.set(
            CatalogQuery::PgVersion,
            vec![Row::new().with("server_version_num", Value::Integer(160_000))],
        );
        let filter = CatalogFilter::new(vec![], vec![]).unwrap();
        let cat = read_catalog(&m, &filter).expect("reads");
        assert!(cat.tables.is_empty());
        assert!(cat.schemas.is_empty());
    }
}
