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

pub mod cluster;
pub mod error;
pub mod filter;
#[cfg(feature = "tokio-postgres-querier")]
pub mod pg_querier;
pub mod queries;
pub mod rows;
pub mod version;

pub use error::CatalogError;
pub use filter::CatalogFilter;
pub use rows::{Row, Value};
pub use version::PgVersion;

mod assemble;
pub(crate) mod collations;
pub(crate) mod grants;
pub(crate) mod publications;
pub(crate) mod reloptions;
pub(crate) mod statistics;
pub(crate) mod subscriptions;

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
/// - `unmanaged_language_routines`: routines whose `LANGUAGE` is neither `sql`
///   nor `plpgsql` (e.g., `plperl`, `python3u`). pgevolve v0.2 does not
///   manage these; they are surfaced in the drift report so callers can
///   inspect them. The associated row is skipped and never appears in
///   `catalog.functions` / `catalog.procedures`.
/// - `unmanaged_aggregates`: ordered-set / hypothetical-set aggregates, or
///   aggregates whose state/final function is in an unmanaged language. The
///   associated row is skipped and never appears in `catalog.aggregates`.
/// - `unmanaged_casts`: `WITH FUNCTION` casts whose conversion function is in
///   a language other than `sql` / `plpgsql`. The associated row is skipped
///   and never appears in `catalog.casts`.
/// - `unreadable_subscriptions`: the connection used for the catalog read had
///   insufficient privilege to query `pg_subscription` (sqlstate 42501). The
///   subscription list in the returned catalog is empty; the operator must use
///   a superuser connection to get subscription data.
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
    /// Routines whose `LANGUAGE` is not `sql` or `plpgsql`.
    /// Identified by `(qname, language_name)`.
    pub unmanaged_language_routines: Vec<(QualifiedName, String)>,
    /// Aggregates pgevolve does not manage: ordered-set / hypothetical-set
    /// aggregates (`aggkind <> 'n'`), or aggregates whose state / final
    /// function is in a language other than `sql` / `plpgsql`. Identified by
    /// the aggregate qname. The associated row is skipped and never appears in
    /// `catalog.aggregates`.
    pub unmanaged_aggregates: Vec<QualifiedName>,
    /// Casts pgevolve does not manage: `WITH FUNCTION` casts whose conversion
    /// function is in a language other than `sql` / `plpgsql`. Identified by
    /// `(source_qname, target_qname)`. The associated row is skipped and never
    /// appears in `catalog.casts`.
    pub unmanaged_casts: Vec<(QualifiedName, QualifiedName)>,
    /// `pg_subscription` was unreadable due to insufficient privilege (sqlstate
    /// 42501). `catalog.subscriptions` will be empty when this is `true`.
    pub unreadable_subscriptions: bool,
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
    /// `pg_type` filtered to `typtype IN ('e','d','c')` for user-defined types.
    UserTypes,
    /// `pg_enum` labels for enum types.
    EnumValues,
    /// Base-type and nullability details for domain types.
    DomainDetails,
    /// Named CHECK constraints attached to domain types.
    DomainChecks,
    /// Attributes (fields) of composite types.
    CompositeAttributes,
    /// `pg_proc` rows for functions and procedures (prokind IN 'f','p').
    Functions,
    /// `pg_aggregate` rows joined to their wrapper `pg_proc` entry.
    ///
    /// Schema-filtered via `$1::text[]` (same param group as [`Self::Functions`]).
    /// The assembler skips ordered-set / hypothetical-set aggregates and any
    /// aggregate whose state/final function is in an unmanaged language,
    /// recording those in [`DriftReport::unmanaged_aggregates`].
    Aggregates,
    /// `pg_extension` rows for installed extensions.
    Extensions,
    /// `pg_trigger` rows for user triggers (excluding internal + extension-owned).
    Triggers,
    /// `pg_class` (relkind='p') rows for partitioned-table parents.
    PartitionedTables,
    /// `pg_class` (relispartition=true) rows for child partitions.
    Partitions,
    /// `pg_authid` rows for cluster roles (with `pg_shdescription` for comments).
    ///
    /// Uses `$1::text[]` as the bootstrap-role filter (names to exclude), not a
    /// managed-schema list. `takes_text_array_param` returns `true` so the adapter
    /// passes the parameter; the cluster reader supplies bootstrap role names.
    ClusterRoles,
    /// `pg_auth_members` edges joined to `pg_authid` for role names.
    ///
    /// Same `$1::text[]` bootstrap-role filter as [`Self::ClusterRoles`].
    ClusterMembers,
    /// `pg_tablespace` rows (with `pg_shdescription` for comments) for managed
    /// cluster tablespaces.
    ///
    /// Built-in `pg_default` / `pg_global` are always excluded. Uses
    /// `$1::text[]` as the bootstrap filter (names to exclude), in the same
    /// param-group as [`Self::ClusterRoles`]; `takes_text_array_param` returns
    /// `true`.
    ClusterTablespaces,
    /// `pg_default_acl` rows joined to `pg_authid` and `pg_namespace`.
    ///
    /// Returns one row per (`target_role`, schema, `object_type`) tuple. Rows for
    /// predefined `pg_*` roles are filtered out. Takes **no** `$1::text[]`
    /// parameter; `takes_text_array_param` returns `false` for this variant.
    DefaultPrivileges,
    /// `pg_policies` rows for managed schemas.
    ///
    /// Returns one row per policy, scoped to `schemaname = ANY($1::text[])`.
    /// Decoded into [`crate::ir::policy::Policy`] and attached to their
    /// owning `Table` by the assembler. Policies on unmanaged tables are
    /// silently dropped.
    Policies,
    /// `pg_publication` rows for all publications in the database.
    ///
    /// Publications are database-global (not schema-scoped); takes **no**
    /// `$1::text[]` parameter (`takes_text_array_param` returns `false`).
    Publications,
    /// `pg_publication_rel` rows — one per (publication, table) membership.
    ///
    /// PG 15+ includes `prqual` (row filter) and `prattrs` (column list);
    /// PG 14 variant substitutes `NULL` for both. Takes **no** parameter.
    PublicationRel,
    /// `pg_publication_namespace` rows — one per (publication, schema)
    /// membership (PG 15+ only). PG 14 variant returns zero rows.
    /// Takes **no** parameter.
    PublicationNamespace,
    /// `pg_attribute` rows for every column of every table referenced by
    /// any publication. Used to resolve column attnums to names. Takes **no**
    /// parameter.
    PublicationAttributes,
    /// `pg_event_trigger` rows for all event triggers in the database.
    ///
    /// Event triggers are database-global (not schema-scoped); takes **no**
    /// `$1::text[]` parameter (`takes_text_array_param` returns `false`).
    /// Extension-owned event triggers (`pg_depend.deptype = 'e'`) are excluded
    /// at the SQL layer.
    EventTriggers,
    /// `pg_subscription` rows for all subscriptions in the database.
    ///
    /// Subscriptions are database-global (not schema-scoped). Takes **no**
    /// `$1::text[]` parameter (`takes_text_array_param` returns `false`).
    ///
    /// `pg_subscription` is superuser-readable only. Non-super connections
    /// will receive an empty result or a permission error; the assembler
    /// catches the error and sets `DriftReport::unreadable_subscriptions`.
    Subscriptions,
    /// `pg_statistic_ext` rows for managed schemas. Takes `$1::text[]`
    /// (managed schema names).
    Statistics,
    /// Column-attnum resolver for statistics target tables. Bulk-fetched once;
    /// grouped by `target_oid` in the assembler. Takes `$1::text[]`.
    StatisticAttributes,
    /// Bulk expression decode via `pg_get_statisticsobjdef_expressions` for all
    /// statistics in managed schemas. Returns one row per expression entry with
    /// columns `(stat_oid, expr_index, expr_sql)`. Takes `$1::text[]`
    /// (managed schema names).
    StatisticExpressions,
    /// `pg_collation` rows for managed schemas — user-defined collations only
    /// (built-in and extension-owned collations are filtered out at the SQL
    /// layer). Takes `$1::text[]` (managed schema names).
    Collations,
    /// `pg_cast` rows for all user-defined casts in the database.
    ///
    /// Casts are database-global (not schema-scoped); takes **no**
    /// `$1::text[]` parameter (`takes_text_array_param` returns `false`).
    /// System / built-in casts (`oid < 16384`) and extension-owned casts
    /// (`pg_depend.deptype = 'e'`) are excluded at the SQL layer. The
    /// assembler skips casts whose conversion function is in an unmanaged
    /// language (anything other than `sql` / `plpgsql`), recording those in
    /// [`DriftReport::unmanaged_casts`].
    Casts,
    /// `pg_ts_dict` rows for managed schemas — user-defined text-search
    /// dictionaries only (extension-owned dictionaries are filtered out at the
    /// SQL layer). Takes `$1::text[]` (managed schema names).
    TsDictionaries,
    /// `pg_ts_config` rows for managed schemas — one row per user-defined
    /// text-search configuration. Extension-owned configurations are filtered
    /// at the SQL layer. Takes `$1::text[]` (managed schema names).
    TsConfigurations,
    /// `pg_ts_config_map` rows for managed schemas — one row per
    /// (config, `token_type`, dictionary) triple, ordered by
    /// `(config_schema, config_name, token_alias, mapseqno)`. Token types are
    /// resolved to alias strings via `ts_token_type(cfgparser)` lateral.
    /// Takes `$1::text[]` (managed schema names).
    TsConfigMappings,
}

impl CatalogQuery {
    /// Whether this query accepts a `$1::text[]` argument.
    ///
    /// The semantic meaning of the array varies by variant: managed-schema
    /// names for per-DB queries, bootstrap-role names for cluster queries.
    /// The adapter is responsible for passing the right slice to the right
    /// variant.
    ///
    /// A few variants (`PgVersion`, `Extensions`) take no parameters at all;
    /// this method returns `false` for those.
    #[must_use]
    pub const fn takes_text_array_param(self) -> bool {
        !matches!(
            self,
            Self::PgVersion
                | Self::Extensions
                | Self::DefaultPrivileges
                | Self::Publications
                | Self::PublicationRel
                | Self::PublicationNamespace
                | Self::PublicationAttributes
                | Self::EventTriggers
                | Self::Subscriptions
                | Self::Casts
        )
    }

    // Note: `Policies` takes `$1::text[]` (managed schemas), so it is NOT in
    // the exclusion list above — `takes_text_array_param` returns `true` for it.
}

/// Sync, driver-agnostic catalog query interface.
///
/// Interface implemented by callers (typically the binary) to execute catalog
/// queries against a live database. Implementations are expected to be sync —
/// async drivers can wrap their runtime in [`fetch`](Self::fetch).
pub trait CatalogQuerier {
    /// Execute the named query with the supplied `$1::text[]` parameter (when
    /// applicable; see [`CatalogQuery::takes_text_array_param`]).
    ///
    /// The semantic meaning of `text_array_param` varies by variant:
    /// managed-schema names for per-DB queries, bootstrap-role names for
    /// cluster queries. Pass an empty slice for queries that take no parameter.
    fn fetch(
        &self,
        query: CatalogQuery,
        text_array_param: &[&str],
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
    let user_types_rows = querier.fetch(CatalogQuery::UserTypes, &managed)?;
    let enum_values_rows = querier.fetch(CatalogQuery::EnumValues, &managed)?;
    let domain_details_rows = querier.fetch(CatalogQuery::DomainDetails, &managed)?;
    let domain_checks_rows = querier.fetch(CatalogQuery::DomainChecks, &managed)?;
    let composite_attributes_rows = querier.fetch(CatalogQuery::CompositeAttributes, &managed)?;
    let functions_rows = querier.fetch(CatalogQuery::Functions, &managed)?;
    let aggregates_rows = querier.fetch(CatalogQuery::Aggregates, &managed)?;
    let extensions_rows = querier.fetch(CatalogQuery::Extensions, &managed)?;
    let triggers_rows = querier.fetch(CatalogQuery::Triggers, &managed)?;
    let partitioned_tables_rows = querier.fetch(CatalogQuery::PartitionedTables, &managed)?;
    let partitions_rows = querier.fetch(CatalogQuery::Partitions, &managed)?;
    let default_privileges_rows = querier.fetch(CatalogQuery::DefaultPrivileges, &[])?;
    let policies_rows = querier.fetch(CatalogQuery::Policies, &managed)?;
    let publications_rows = querier.fetch(CatalogQuery::Publications, &[])?;
    let publication_rels_rows = querier.fetch(CatalogQuery::PublicationRel, &[])?;
    let publication_namespaces_rows = querier.fetch(CatalogQuery::PublicationNamespace, &[])?;
    let publication_attributes_rows = querier.fetch(CatalogQuery::PublicationAttributes, &[])?;
    let event_triggers_rows = querier.fetch(CatalogQuery::EventTriggers, &[])?;

    // `pg_subscription` is superuser-only. If the querier returns a
    // `QueryFailed` error whose message contains the PG sqlstate 42501
    // (insufficient_privilege), we silently return empty rows and record the
    // gap in the drift report. Any other error is propagated normally.
    let (subscriptions_rows, unreadable_subscriptions) =
        match querier.fetch(CatalogQuery::Subscriptions, &[]) {
            Ok(rows) => (rows, false),
            Err(CatalogError::QueryFailed { message, .. })
                if message.contains("42501") || message.contains("insufficient_privilege") =>
            {
                (vec![], true)
            }
            Err(e) => return Err(e),
        };

    // Statistics — schema-scoped. Attribute rows resolve stxkeys attnums to
    // column names. Expression rows are bulk-fetched for all managed schemas.
    let statistics_rows = querier.fetch(CatalogQuery::Statistics, &managed)?;
    let statistic_attributes_rows = querier.fetch(CatalogQuery::StatisticAttributes, &managed)?;
    let statistic_expressions_rows = querier.fetch(CatalogQuery::StatisticExpressions, &managed)?;

    // Collations — schema-scoped. User-defined only; built-ins and extension-
    // owned collations filtered at the SQL layer.
    let collations_rows = querier.fetch(CatalogQuery::Collations, &managed)?;

    // Casts — database-global. System / extension-owned casts excluded at the
    // SQL layer. Takes no schema param.
    let casts_rows = querier.fetch(CatalogQuery::Casts, &[])?;

    // Text-search dictionaries — schema-scoped. Extension-owned excluded at SQL.
    let ts_dictionaries_rows = querier.fetch(CatalogQuery::TsDictionaries, &managed)?;

    // Text-search configurations + their token-type→dict mappings.
    // Two separate queries; the assembler groups them by config qname.
    let ts_configurations_rows = querier.fetch(CatalogQuery::TsConfigurations, &managed)?;
    let ts_config_mappings_rows = querier.fetch(CatalogQuery::TsConfigMappings, &managed)?;

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
        user_types: user_types_rows,
        enum_values: enum_values_rows,
        domain_details: domain_details_rows,
        domain_checks: domain_checks_rows,
        composite_attributes: composite_attributes_rows,
        functions: functions_rows,
        aggregates: aggregates_rows,
        extensions: extensions_rows,
        triggers: triggers_rows,
        partitioned_tables: partitioned_tables_rows,
        partitions: partitions_rows,
        default_privileges: default_privileges_rows,
        policies: policies_rows,
        publications: publications_rows,
        publication_rels: publication_rels_rows,
        publication_namespaces: publication_namespaces_rows,
        publication_attributes: publication_attributes_rows,
        event_triggers: event_triggers_rows,
        subscriptions: subscriptions_rows,
        ts_dictionaries: ts_dictionaries_rows,
        ts_configurations: ts_configurations_rows,
        ts_config_mappings: ts_config_mappings_rows,
    };
    let (mut catalog, mut drift) = assemble::assemble(raw, filter)?;
    drift.unreadable_subscriptions = unreadable_subscriptions;

    // Assemble statistics after the main assemble pass. All three row sets
    // (base rows, attribute rows, expression rows) are already bulk-fetched.
    catalog.statistics = assemble::statistics::assemble_statistics(
        &statistics_rows,
        &statistic_attributes_rows,
        &statistic_expressions_rows,
    )?;

    // Assemble collations from the bulk-fetched rows.
    catalog.collations = assemble::collations::build_collations(&collations_rows)?;

    // Assemble casts from the bulk-fetched rows.
    catalog.casts = assemble::casts::assemble_casts(&casts_rows, &mut drift)?;

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
        fn fetch(
            &self,
            q: CatalogQuery,
            _text_array_param: &[&str],
        ) -> Result<Vec<Row>, CatalogError> {
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
