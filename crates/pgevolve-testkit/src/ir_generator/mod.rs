//! Proptest strategy producing random valid [`Catalog`]s and
//! [`pgevolve_core::ir::cluster::catalog::ClusterCatalog`]s.
//!
//! Scope for v0.1: schemas + tables + indexes + sequences with `PRIMARY KEY`
//! and a modest set of column types. Foreign-key density is configurable;
//! when FKs are generated, the referenced column always has a unique
//! constraint (the PK) so the produced catalog passes
//! `Catalog::canonicalize()` and the planner's closed-world check.
//!
//! v0.3 additions: `arbitrary_role_attributes`, `arbitrary_role`,
//! `arbitrary_cluster_catalog` — cycle-free role membership via topological
//! ordering (each role's `member_of` references are drawn from earlier roles
//! only, guaranteeing an acyclic graph).
//!
//! v0.3.1 additions: `arb_owner`, `arb_grants`, `arbitrary_default_privileges`
//! — optional owner and grants on all 8 grantable IR types (Schema, Sequence,
//! Table, View, `MaterializedView`, Function, Procedure, `UserType`), plus
//! top-level default-privilege rules on the Catalog.
//!
//! v0.3.2 additions: `arb_policy_command`, `arb_policy` — policy strategies
//! embedded in `arbitrary_table`. `arb_policy` respects the WITH CHECK /
//! FOR SELECT-or-DELETE incompatibility (generated policies never produce
//! invalid PG syntax). `rls_enabled` and `rls_forced` added as independent
//! bool strategies on each generated table.
//!
//! v0.3.3 additions: `arb_autovacuum_extra`, `arb_table_storage`,
//! `arb_index_storage` — reloption configurations plumbed into
//! `arbitrary_table` and `arbitrary_indexes_for_table`. Range-bounded
//! strategies (fillfactor 10..=100 for tables, 50..=100 for indexes) prevent
//! generating PG-invalid combinations.
//!
//! v0.3.4 additions: `arb_publish_kinds`, `arb_publication_scope`,
//! `arb_publication` — publication strategies plumbed into
//! `arbitrary_catalog`. Schema and table targets are drawn from the catalog's
//! actual contents so generated publications always reference real objects.
//! Row filters and column lists are left `None` — deeper variation is a
//! v0.3.4.1 follow-up.
//!
//! v0.3.5 additions: `arb_streaming_mode`, `arb_origin_mode`,
//! `arb_subscription_options`, `arb_subscription` — subscription strategies
//! plumbed into `arbitrary_catalog`. Publication names are drawn from the
//! catalog's actual publications so generated subscriptions always reference
//! real publications. CREATE-only fields (`create_slot`, `copy_data`) and
//! PG-version-gated fields (`password_required`, `run_as_owner`) are left
//! `None` to keep generation simple and lint-clean.
//!
//! v0.3.7 additions: `arb_check_option` — optional `CheckOption` (Local /
//! Cascaded) plumbed into the view constructor inside `arbitrary_view_catalog`.
//! `arb_statistic_kinds`, `arb_statistic` — statistic strategies generating
//! 0–1 statistics per table in `arbitrary_catalog`, drawing columns from the
//! target table's actual column list so generated statistics always reference
//! real columns.
//!
//! v0.3.8 additions: `arb_collations_for_schemas` (libc-only, deterministic,
//! safe locale pool — keeps PG-version gating out of the property soak) and
//! `arb_user_types_for_schemas` (Enum / Composite / Range mix at ~10 % Range
//! weight, covering the new `UserTypeKind::Range` variant). Both produce 0–2
//! objects per managed schema and feed into `arbitrary_catalog` before
//! canonicalization so generated catalogs always reference real schemas.
//!
//! Richer coverage (CHECK constraints, multi-column UNIQUE, generated
//! columns, identity sequences, partial indexes, the full `ColumnType`
//! matrix) is deferred to v0.1.x.
//!
//! ## Module layout
//!
//! The strategies are organised into family-specific sub-modules:
//!
//! - [`schema`] — schema generation.
//! - [`table`] — table + column strategies and [`arbitrary_column_type`].
//! - [`index`] — index strategies and the B-tree opclass whitelist.
//! - [`sequence`] — stand-alone sequence construction.
//! - [`policy`] — RLS policy strategies.
//! - [`reloptions`] — storage / compression / autovacuum / table+index reloptions.
//! - [`grants`] — owner + grant strategies and [`arbitrary_default_privileges`].
//! - [`statistic`] — multi-column statistic strategies including the public
//!   [`arb_statistic`].
//! - [`publication`] — publication strategies.
//! - [`subscription`] — subscription strategies.
//! - [`collation`] — collation strategies (v0.3.8).
//! - [`user_type`] — user-type strategies covering Enum / Composite / Range
//!   (v0.3.8).
//! - [`cluster`] — cluster-level role + [`arbitrary_cluster_catalog`].

// Proptest closures and `prop_map`/`prop_flat_map` chains in this module
// inherently clone moved captures; the pedantic lints fight straight-line
// strategy code.
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::assigning_clones)]
#![allow(clippy::format_push_string)]
// `arbitrary_view_catalog` is long by design — the dep-graph + owner/grant
// generation logic cannot be split without losing the captured state.
#![allow(clippy::too_many_lines)]

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::constraint::ConstraintKind;
use pgevolve_core::ir::index::Index;
use pgevolve_core::ir::table::Table;
use pgevolve_core::ir::view::{CheckOption, View};
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};

pub mod cluster;
pub mod collation;
pub mod event_trigger;
pub mod grants;
pub mod index;
pub mod policy;
pub mod publication;
pub mod reloptions;
pub mod schema;
pub mod sequence;
pub mod statistic;
pub mod subscription;
pub mod table;
pub mod user_type;

pub use cluster::{arbitrary_cluster_catalog, arbitrary_role_attributes};
pub use grants::arbitrary_default_privileges;
pub use statistic::arb_statistic;
pub use table::arbitrary_column_type;

// Internal re-exports for in-module use.
pub(crate) use index::is_btree_indexable;

use collation::arb_collations_for_schemas;
use event_trigger::arb_event_triggers;
use grants::{SEQUENCE_PRIVS, TABLE_PRIVS, arb_object_grants, arb_owner, arb_table_grants};
use index::arbitrary_indexes_for_table;
use publication::arb_publications;
use schema::arbitrary_schemas;
use sequence::stand_alone_sequence;
use statistic::arb_statistics_for_tables;
use subscription::arb_subscriptions;
use table::arbitrary_tables_for_schema;
use user_type::arb_user_types_for_schemas;

/// Knobs controlling [`arbitrary_catalog`] output.
#[derive(Debug, Clone)]
pub struct IRGeneratorConfig {
    /// `(min, max)` number of schemas, inclusive.
    pub schema_count_range: (usize, usize),
    /// `(min, max)` tables per schema, inclusive.
    pub tables_per_schema_range: (usize, usize),
    /// `(min, max)` non-pk columns per table, inclusive.
    pub columns_per_table_range: (usize, usize),
    /// Probability `[0.0..=1.0]` that any given non-PK column becomes the
    /// local side of an FK to some other table's PK.
    pub fk_density: f64,
    /// `(min, max)` indexes per table, inclusive.
    pub index_per_table_range: (usize, usize),
}

impl Default for IRGeneratorConfig {
    fn default() -> Self {
        Self {
            schema_count_range: (1, 2),
            tables_per_schema_range: (1, 3),
            columns_per_table_range: (1, 4),
            fk_density: 0.25,
            index_per_table_range: (0, 2),
        }
    }
}

/// Generate a random valid `Catalog`.
pub fn arbitrary_catalog(cfg: IRGeneratorConfig) -> impl Strategy<Value = Catalog> {
    arbitrary_schemas(&cfg)
        .prop_flat_map(move |schemas| {
            let cfg = cfg.clone();
            let tables_strategies: Vec<_> = schemas
                .iter()
                .map(|s| arbitrary_tables_for_schema(s.name.clone(), &cfg))
                .collect();
            tables_strategies
                .prop_map(move |tables_per_schema| (schemas.clone(), tables_per_schema))
        })
        .prop_flat_map(move |(schemas, tables_per_schema)| {
            // Flatten to a single table vec while preserving the schema mapping
            // (each table already carries qname.schema).
            let tables: Vec<Table> = tables_per_schema.into_iter().flatten().collect();
            let table_pks: Vec<(QualifiedName, Identifier)> = tables
                .iter()
                .filter_map(|t| {
                    t.constraints.iter().find_map(|c| match &c.kind {
                        ConstraintKind::PrimaryKey { columns, .. } if columns.len() == 1 => {
                            Some((t.qname.clone(), columns[0].clone()))
                        }
                        _ => None,
                    })
                })
                .collect();

            // Per-table indexes referencing the table's existing columns.
            let index_strategies: Vec<_> = tables
                .iter()
                .map(|t| arbitrary_indexes_for_table(t, table_pks.len()))
                .collect();

            // Sequence owner + grants per schema (one sequence per schema).
            let seq_owner_grant_strategies: Vec<_> = schemas
                .iter()
                .map(|_| (arb_owner(), arb_object_grants(SEQUENCE_PRIVS)))
                .collect();

            // Shared schema name pool used by several sub-strategies below.
            let schema_names: Vec<Identifier> = schemas.iter().map(|s| s.name.clone()).collect();

            // Default-privilege rules — restrict `IN SCHEMA y` to schemas that
            // are actually declared in this catalog so the DDL never references
            // a schema that does not exist (PG error 3F000).
            let default_privs_strategy = arbitrary_default_privileges(&schema_names);

            // Publications drawing from the catalog's own schemas + tables.
            let schema_pool: Vec<Identifier> = schema_names.clone();
            let table_pool: Vec<QualifiedName> = tables.iter().map(|t| t.qname.clone()).collect();
            let publications_strategy = arb_publications(schema_pool, table_pool);

            // Statistics drawing from the catalog's actual tables + columns.
            let statistics_strategy = arb_statistics_for_tables(&tables);

            // Collations and user types — generated once per schema-set so
            // they share the same name pool and are available before
            // canonicalization.
            let collations_strategy = arb_collations_for_schemas(&schema_names);
            let user_types_strategy = arb_user_types_for_schemas(&schema_names);

            // Event triggers, each paired with its dedicated `RETURNS
            // event_trigger` function. The function's schema is drawn from the
            // managed schema pool. Nothing else in the generator produces
            // functions today, so these are the catalog's only functions.
            let event_triggers_strategy = arb_event_triggers(schema_names);

            (
                index_strategies,
                seq_owner_grant_strategies,
                default_privs_strategy,
                publications_strategy,
                statistics_strategy,
                collations_strategy,
                user_types_strategy,
                event_triggers_strategy,
            )
                .prop_flat_map(
                    move |(
                        idx_per_table,
                        seq_owner_grants,
                        default_privileges,
                        publications,
                        statistics,
                        collations,
                        user_types,
                        event_triggers,
                    )| {
                        // Build the publication-name pool for subscription generation
                        // from the publications just generated for this catalog.
                        let pub_name_pool: Vec<Identifier> =
                            publications.iter().map(|p| p.name.clone()).collect();
                        let subscriptions_strategy = arb_subscriptions(pub_name_pool);

                        let indexes_c = idx_per_table;
                        let schemas_c = schemas.clone();
                        let tables_c = tables.clone();
                        let (et_functions, event_triggers) = event_triggers;

                        subscriptions_strategy.prop_map(move |subscriptions| {
                            let indexes: Vec<Index> =
                                indexes_c.clone().into_iter().flatten().collect();
                            let mut catalog = Catalog::empty();
                            catalog.schemas = schemas_c.clone();
                            catalog.tables = tables_c.clone();
                            catalog.indexes = indexes;
                            // Sprinkle in sequences for variety (one per schema, with
                            // random owner + grants).
                            for (s, (seq_owner, seq_grants)) in
                                catalog.schemas.iter().zip(seq_owner_grants.clone())
                            {
                                let mut seq = stand_alone_sequence(&s.name);
                                seq.owner = seq_owner;
                                seq.grants = seq_grants;
                                catalog.sequences.push(seq);
                            }
                            // Attach default-privilege rules (dedup by key before canon).
                            catalog.default_privileges = default_privileges.clone();
                            // Attach publications (unique names enforced by arb_publications).
                            catalog.publications = publications.clone();
                            // Attach 0–1 subscriptions (names reference actual publications).
                            catalog.subscriptions = subscriptions;
                            // Attach 0–1 statistics per table (deduped by qname in canon).
                            catalog.statistics = statistics.clone();
                            // Attach 0–2 collations per schema (libc-only,
                            // deterministic, safe locale pool).
                            catalog.collations = collations.clone();
                            // Attach 0–2 user types per schema (Enum /
                            // Composite / Range mix at ~10 % Range weight).
                            catalog.types = user_types.clone();
                            // Append the event-trigger backing functions (the
                            // only functions the generator produces) and attach
                            // the 0–3 event triggers that reference them. Done
                            // before canonicalize so the function references
                            // resolve in the closed-world check.
                            catalog.functions.extend(et_functions.clone());
                            catalog.event_triggers = event_triggers.clone();
                            catalog
                                .canonicalize()
                                .expect("generator produced invalid catalog")
                        })
                    },
                )
        })
}

/// Generate a random valid `Catalog` that also includes a topologically-ordered
/// DAG of views over the catalog's tables (and over earlier views).
///
/// The number of views per schema is controlled by the new `views_per_schema`
/// field on `IRViewCatalogConfig`. Each view's `body_dependencies` is set
/// programmatically — no real SQL parsing is required. This makes the generator
/// pure and fast, and keeps the focus on the planner's dep-graph walk rather
/// than the canonicalizer.
pub fn arbitrary_view_catalog() -> impl Strategy<Value = Catalog> {
    // Re-use the existing table generator with a small config.
    let cfg = IRGeneratorConfig {
        schema_count_range: (1, 2),
        tables_per_schema_range: (2, 5),
        columns_per_table_range: (1, 3),
        fk_density: 0.0, // no FKs — keeps graph acyclic and simple
        index_per_table_range: (0, 0),
    };

    arbitrary_catalog(cfg).prop_flat_map(|catalog| {
        // For each schema, generate 1–6 views in topological order.
        // View #i may reference any subset of:
        //   - tables in the same schema
        //   - views [0..i) in the same schema (earlier views only → guarantees DAG)
        let schema_names: Vec<Identifier> =
            catalog.schemas.iter().map(|s| s.name.clone()).collect();

        // Build per-schema table lists.
        let schema_tables: Vec<Vec<QualifiedName>> = schema_names
            .iter()
            .map(|sname| {
                catalog
                    .tables
                    .iter()
                    .filter(|t| &t.qname.schema == sname)
                    .map(|t| t.qname.clone())
                    .collect()
            })
            .collect();

        // We need at least one table per schema to generate views.
        // If a schema somehow has no tables, skip view generation for it.

        // Generate view count per schema (1..=6).
        let schema_count = schema_names.len();
        let view_count_strategy: Vec<_> = (0..schema_count)
            .map(|i| {
                if schema_tables[i].is_empty() {
                    Just(0usize).boxed()
                } else {
                    (1usize..=6usize).boxed()
                }
            })
            .collect();

        view_count_strategy.prop_flat_map(move |view_counts_per_schema| {
            let schema_names = schema_names.clone();
            let schema_tables = schema_tables.clone();
            let catalog = catalog.clone();

            // For each schema, generate exactly `view_counts[i]` views.
            // We use a sequential strategy: we build them one by one in a
            // flat_map chain. But since proptest doesn't natively support
            // sequential generation with shared state, we use a different
            // approach: generate all random bits upfront (a vec of bitmasks),
            // then interpret them deterministically.
            let total_views: usize = view_counts_per_schema.iter().sum();

            // Strategy: generate `total_views` bitmasks (u32), each mask
            // determines which prior refs the view depends on (from the pool
            // of tables + prior views within the same schema).
            proptest_vec(any::<u32>(), total_views..=(total_views.max(1)))
                .prop_map(move |masks| {
                    let mut views: Vec<View> = Vec::new();
                    let mut mask_idx = 0;

                    // Per-schema: we track the views generated so far for
                    // that schema so later views can reference earlier ones.
                    for (schema_idx, &view_count) in view_counts_per_schema.iter().enumerate() {
                        let schema_name = &schema_names[schema_idx];
                        let schema_tbls = &schema_tables[schema_idx];

                        // views in this schema generated so far (by index in `views`).
                        let schema_view_start = views.len();

                        for view_i in 0..view_count {
                            let mask = masks.get(mask_idx).copied().unwrap_or(0);
                            mask_idx += 1;

                            // Pool of eligible deps:
                            //   - schema tables
                            //   - views[schema_view_start .. schema_view_start + view_i]
                            let all_refs: Vec<NodeId> = schema_tbls
                                .iter()
                                .map(|q| NodeId::Table(q.clone()))
                                .chain(
                                    (schema_view_start..schema_view_start + view_i)
                                        .map(|idx| NodeId::View(views[idx].qname.clone())),
                                )
                                .collect();

                            // Name: view_<schema>_<i>
                            let view_name = Identifier::from_unquoted(&format!(
                                "view_{}_{}",
                                schema_name.as_str(),
                                view_i
                            ))
                            .unwrap();
                            let qname = QualifiedName::new(schema_name.clone(), view_name);

                            // Pick deps from all_refs based on bitmask.
                            // Always pick at least one if pool is non-empty.
                            let deps: Vec<DepEdge> = if all_refs.is_empty() {
                                vec![]
                            } else {
                                let mut chosen = Vec::new();
                                for (bit, target_node) in all_refs.iter().enumerate() {
                                    if bit == 0 || (mask >> bit) & 1 == 1 {
                                        // bit 0 is always chosen (guarantees at least one dep).
                                        chosen.push(DepEdge {
                                            from: NodeId::View(qname.clone()),
                                            to: target_node.clone(),
                                            source: DepSource::AstExtracted,
                                        });
                                    }
                                }
                                chosen
                            };

                            views.push(View {
                                qname,
                                columns: vec![],
                                body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
                                body_dependencies: deps,
                                security_barrier: None,
                                security_invoker: None,
                                check_option: None,
                                comment: None,
                                raw_body: String::new(),
                                owner: None,
                                grants: vec![],
                            });
                        }
                    }

                    (catalog.clone(), views)
                })
                .prop_flat_map(|(catalog, views)| {
                    // Sprinkle random owner, grants, and check_option onto each view.
                    let n = views.len();
                    let owner_strategies: Vec<_> = (0..n).map(|_| arb_owner()).collect();
                    let grant_strategies: Vec<_> =
                        // Views don't have a tracked column list in the IR, so
                        // pass an empty column pool → no column-restricted grants.
                        (0..n)
                            .map(|_| arb_table_grants(TABLE_PRIVS, vec![]))
                            .collect();
                    let check_option_strategies: Vec<_> =
                        (0..n).map(|_| arb_check_option()).collect();
                    (owner_strategies, grant_strategies, check_option_strategies).prop_map(
                        move |(owners, grant_vecs, check_opts)| {
                            let mut views_with_grants: Vec<View> = views.clone();
                            for (i, v) in views_with_grants.iter_mut().enumerate() {
                                v.owner = owners[i].clone();
                                v.grants = grant_vecs[i].clone();
                                v.check_option = check_opts[i];
                            }
                            let mut cat = catalog.clone();
                            cat.views = views_with_grants;
                            // canonicalize sorts views and checks for duplicates.
                            cat.canonicalize()
                                .expect("view generator produced invalid catalog")
                        },
                    )
                })
        })
    })
}

/// Generate an optional [`CheckOption`] for a view (`None`, `Local`, or `Cascaded`).
///
/// Only consumed by [`arbitrary_view_catalog`]; kept here so the view
/// construction stays self-contained in `mod.rs`.
fn arb_check_option() -> impl Strategy<Value = Option<CheckOption>> {
    prop_oneof![
        Just(None),
        Just(Some(CheckOption::Local)),
        Just(Some(CheckOption::Cascaded)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    #[test]
    fn generator_produces_valid_catalogs() {
        let mut runner = TestRunner::default();
        for _ in 0..50 {
            let tree = arbitrary_catalog(IRGeneratorConfig::default())
                .new_tree(&mut runner)
                .unwrap();
            let catalog = tree.current();
            // canonicalize already invoked inside the generator; check non-empty.
            assert!(!catalog.schemas.is_empty());
            assert!(!catalog.tables.is_empty());
        }
    }

    #[test]
    fn generator_produces_event_triggers_with_backing_functions() {
        let mut runner = TestRunner::default();
        let mut saw_event_trigger = false;
        for _ in 0..200 {
            let catalog = arbitrary_catalog(IRGeneratorConfig::default())
                .new_tree(&mut runner)
                .unwrap()
                .current();
            // Every event trigger must reference a function that exists in the
            // catalog (the closed-world invariant the soak relies on).
            for et in &catalog.event_triggers {
                assert!(
                    catalog.functions.iter().any(|f| f.qname == et.function),
                    "event trigger '{}' references missing function '{}'",
                    et.name.as_str(),
                    et.function.render_sql(),
                );
            }
            if !catalog.event_triggers.is_empty() {
                saw_event_trigger = true;
            }
        }
        assert!(
            saw_event_trigger,
            "expected at least one generated catalog to contain event triggers",
        );
    }

    #[test]
    fn generator_covers_a_variety_of_column_types() {
        let mut runner = TestRunner::default();
        let mut seen = std::collections::BTreeSet::new();
        // Sample 200 catalogs; collect distinct column types.
        for _ in 0..200 {
            let tree = arbitrary_catalog(IRGeneratorConfig::default())
                .new_tree(&mut runner)
                .unwrap();
            let c = tree.current();
            for t in &c.tables {
                for col in &t.columns {
                    seen.insert(col.ty.render_sql());
                }
            }
        }
        // At least 5 distinct rendered types (boolean, integer, text, …).
        assert!(
            seen.len() >= 5,
            "expected ≥ 5 distinct column types, saw {}: {:?}",
            seen.len(),
            seen,
        );
    }
}
