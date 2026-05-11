//! Per-version SQL strings for the catalog reader.
//!
//! Most queries are stable across PG 14–17. The known divergences
//! (currently: `pg_index.indnullsnotdistinct` lands in PG 15) live in
//! per-version submodules; [`query_for`] dispatches.

pub mod pg14;
pub mod pg15;
pub mod pg16;
pub mod pg17;
pub mod shared;

use crate::catalog::CatalogQuery;
use crate::catalog::version::PgVersion;

/// Pick the SQL string for `query` on the given PG major version.
#[must_use]
pub const fn query_for(version: PgVersion, query: CatalogQuery) -> &'static str {
    match (version, query) {
        (_, CatalogQuery::PgVersion) => shared::PG_VERSION_QUERY,
        (_, CatalogQuery::Schemas) => shared::SCHEMAS_QUERY,
        (_, CatalogQuery::Tables) => shared::TABLES_QUERY,
        (_, CatalogQuery::Columns) => shared::COLUMNS_QUERY,
        (_, CatalogQuery::Constraints) => shared::CONSTRAINTS_QUERY,
        (PgVersion::Pg14, CatalogQuery::Indexes) => pg14::INDEXES_QUERY,
        (_, CatalogQuery::Indexes) => shared::INDEXES_QUERY,
        (_, CatalogQuery::Sequences) => shared::SEQUENCES_QUERY,
        (_, CatalogQuery::Comments) => shared::COMMENTS_QUERY,
        (_, CatalogQuery::Dependencies) => shared::DEPENDENCIES_QUERY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg14_uses_no_nulls_not_distinct_indexes_query() {
        let q = query_for(PgVersion::Pg14, CatalogQuery::Indexes);
        assert!(!q.contains("indnullsnotdistinct"));
    }

    #[test]
    fn pg15_plus_includes_nulls_not_distinct() {
        for v in [PgVersion::Pg15, PgVersion::Pg16, PgVersion::Pg17] {
            assert!(query_for(v, CatalogQuery::Indexes).contains("indnullsnotdistinct"));
        }
    }
}
