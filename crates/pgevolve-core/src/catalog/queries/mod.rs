//! Per-version SQL strings for the catalog reader.
//!
//! Most queries are stable across PG 14–17. The known divergences
//! (currently: `pg_index.indnullsnotdistinct` lands in PG 15) live in
//! per-version submodules; [`query_for`] dispatches.

pub mod cluster;
pub mod collations;
pub mod default_privileges;
pub mod extensions;
pub mod functions;
pub mod partitioned_tables;
pub mod partitions;
pub mod pg14;
pub mod pg15;
pub mod pg16;
pub mod pg17;
pub mod pg18;
pub mod policies;
pub mod shared;
pub mod triggers;
pub mod types;
pub mod views;

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
        (_, CatalogQuery::ViewsAndMvs) => views::SELECT_VIEWS_AND_MVS,
        (_, CatalogQuery::ViewColumns) => views::SELECT_VIEW_COLUMNS,
        (_, CatalogQuery::UserTypes) => types::SELECT_USER_TYPES,
        (_, CatalogQuery::EnumValues) => types::SELECT_ENUM_VALUES,
        (_, CatalogQuery::DomainDetails) => types::SELECT_DOMAIN_DETAILS,
        (_, CatalogQuery::DomainChecks) => types::SELECT_DOMAIN_CHECKS,
        (_, CatalogQuery::CompositeAttributes) => types::SELECT_COMPOSITE_ATTRIBUTES,
        (_, CatalogQuery::Functions) => functions::SELECT_FUNCTIONS,
        (_, CatalogQuery::Extensions) => extensions::SELECT_EXTENSIONS,
        (_, CatalogQuery::Triggers) => triggers::SELECT_TRIGGERS,
        (_, CatalogQuery::PartitionedTables) => partitioned_tables::SELECT_PARTITIONED_TABLES,
        (_, CatalogQuery::Partitions) => partitions::SELECT_PARTITIONS,
        (_, CatalogQuery::ClusterRoles) => cluster::CLUSTER_ROLES_QUERY,
        (_, CatalogQuery::ClusterMembers) => cluster::CLUSTER_MEMBERS_QUERY,
        (_, CatalogQuery::DefaultPrivileges) => default_privileges::DEFAULT_PRIVILEGES_QUERY,
        (_, CatalogQuery::Policies) => policies::POLICIES_QUERY,
        (_, CatalogQuery::Publications) => shared::PUBLICATIONS_QUERY,
        (PgVersion::Pg14, CatalogQuery::PublicationRel) => pg14::PUBLICATION_REL_QUERY_PG14,
        (_, CatalogQuery::PublicationRel) => shared::PUBLICATION_REL_QUERY,
        (PgVersion::Pg14, CatalogQuery::PublicationNamespace) => {
            pg14::PUBLICATION_NAMESPACE_QUERY_PG14
        }
        (_, CatalogQuery::PublicationNamespace) => shared::PUBLICATION_NAMESPACE_QUERY,
        (PgVersion::Pg14, CatalogQuery::PublicationAttributes) => {
            pg14::PUBLICATION_ATTRIBUTES_QUERY_PG14
        }
        (_, CatalogQuery::PublicationAttributes) => shared::PUBLICATION_ATTRIBUTES_QUERY,
        (PgVersion::Pg14, CatalogQuery::Subscriptions) => pg14::SUBSCRIPTIONS_QUERY_PG14,
        (PgVersion::Pg15, CatalogQuery::Subscriptions) => pg15::SUBSCRIPTIONS_QUERY_PG15,
        (PgVersion::Pg16, CatalogQuery::Subscriptions) => pg16::SUBSCRIPTIONS_QUERY_PG16,
        (_, CatalogQuery::Subscriptions) => shared::SUBSCRIPTIONS_QUERY,
        (_, CatalogQuery::Statistics) => shared::STATISTICS_QUERY,
        (_, CatalogQuery::StatisticAttributes) => shared::STATISTIC_ATTRIBUTES_QUERY,
        (_, CatalogQuery::StatisticExpressions) => shared::STATISTIC_EXPRESSIONS_QUERY,
        (PgVersion::Pg14 | PgVersion::Pg15, CatalogQuery::Collations) => {
            collations::SELECT_COLLATIONS_PG14_15
        }
        (PgVersion::Pg16 | PgVersion::Pg17 | PgVersion::Pg18, CatalogQuery::Collations) => {
            collations::SELECT_COLLATIONS_PG16_PLUS
        }
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
        for v in [
            PgVersion::Pg15,
            PgVersion::Pg16,
            PgVersion::Pg17,
            PgVersion::Pg18,
        ] {
            assert!(query_for(v, CatalogQuery::Indexes).contains("indnullsnotdistinct"));
        }
    }

    #[test]
    fn pg14_15_collations_query_uses_legacy_columns_only() {
        for v in [PgVersion::Pg14, PgVersion::Pg15] {
            let q = query_for(v, CatalogQuery::Collations);
            assert!(!q.contains("colllocale"));
            assert!(q.contains("collcollate"));
        }
    }

    #[test]
    fn pg16_plus_collations_query_coalesces_colllocale() {
        for v in [PgVersion::Pg16, PgVersion::Pg17, PgVersion::Pg18] {
            let q = query_for(v, CatalogQuery::Collations);
            assert!(q.contains("colllocale"));
            assert!(q.contains("COALESCE"));
        }
    }
}
