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

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;

/// Drift detected between the canonical catalog IR and the live Postgres state.
///
/// The catalog reader always surfaces all constraints and indexes in the IR
/// regardless of their validation state. This report captures the *extra*
/// observation that some of them are in a transitional / incomplete state:
/// - `pending_validation`: constraints with `pg_constraint.convalidated = false`
///   (added `NOT VALID`, never validated).
/// - `invalid_indexes`: indexes with `pg_index.indisvalid = false` (e.g., a
///   `CREATE INDEX CONCURRENTLY` that failed and left an INVALID index).
///
/// The differ consumes this report and emits [`crate::diff::change::Change::ValidateConstraint`]
/// and [`crate::diff::change::Change::RecreateIndex`] to recover automatically.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriftReport {
    /// Constraints present in the catalog but with `convalidated = false`.
    /// Identified by `(table_qname, constraint_name)`.
    pub pending_validation: Vec<(QualifiedName, Identifier)>,
    /// Indexes present in the catalog but with `indisvalid = false`.
    /// Identified by index qname.
    pub invalid_indexes: Vec<QualifiedName>,
}

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
    /// `pg_class` (relkind IN ('v','m')) joined with `pg_get_viewdef`.
    ViewsAndMvs,
    /// `pg_attribute` for view and materialized view columns.
    ViewColumns,
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
///
/// Returns a `(Catalog, DriftReport)` tuple. The catalog contains all objects
/// including those in transitional states (NOT VALID constraints, INVALID
/// indexes). The drift report captures which objects are in those states so the
/// differ can emit recovery changes.
pub fn read_catalog(
    querier: &dyn CatalogQuerier,
    filter: &CatalogFilter,
) -> Result<(Catalog, DriftReport), CatalogError> {
    let version = PgVersion::detect(querier)?;
    let managed: Vec<&str> = filter.managed_schemas_param();

    let schemas_rows = querier.fetch(CatalogQuery::Schemas, &managed)?;
    let tables_rows = querier.fetch(CatalogQuery::Tables, &managed)?;
    let columns_rows = querier.fetch(CatalogQuery::Columns, &managed)?;
    let constraints_rows = querier.fetch(CatalogQuery::Constraints, &managed)?;
    let indexes_rows = querier.fetch(CatalogQuery::Indexes, &managed)?;
    let sequences_rows = querier.fetch(CatalogQuery::Sequences, &managed)?;
    let dependencies_rows = querier.fetch(CatalogQuery::Dependencies, &managed)?;
    let views_and_mvs_rows = querier.fetch(CatalogQuery::ViewsAndMvs, &managed)?;
    let view_columns_rows = querier.fetch(CatalogQuery::ViewColumns, &managed)?;

    let raw = assemble::RawRows {
        version,
        schemas: schemas_rows,
        tables: tables_rows,
        columns: columns_rows,
        constraints: constraints_rows,
        indexes: indexes_rows,
        sequences: sequences_rows,
        dependencies: dependencies_rows,
        views_and_mvs: views_and_mvs_rows,
        view_columns: view_columns_rows,
    };
    let (catalog, drift) = assemble::assemble(raw, filter)?;
    Ok((catalog.canonicalize()?, drift))
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
        let (cat, drift) = read_catalog(&m, &filter).expect("reads");
        assert!(cat.tables.is_empty());
        assert!(cat.schemas.is_empty());
        assert!(drift.pending_validation.is_empty());
        assert!(drift.invalid_indexes.is_empty());
    }
}
