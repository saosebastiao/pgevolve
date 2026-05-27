//! Proptest strategy producing random valid [`Catalog`]s and [`ClusterCatalog`]s.
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
//! v0.3.3 additions: `arb_autovacuum_options`, `arb_table_storage`,
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
//! Richer coverage (CHECK constraints, multi-column UNIQUE, generated
//! columns, identity sequences, partial indexes, the full `ColumnType`
//! matrix) is deferred to v0.1.x.

// Proptest closures and `prop_map`/`prop_flat_map` chains in this module
// inherently clone moved captures; the pedantic lints fight straight-line
// strategy code.
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::assigning_clones)]
#![allow(clippy::format_push_string)]
// Single-char binding names in slice-pattern arms are intentional for
// conciseness in `arb_grant_from`'s privilege-slice dispatch.
#![allow(clippy::many_single_char_names)]
// `arbitrary_view_catalog` is long by design — the dep-graph + owner/grant
// generation logic cannot be split without losing the captured state.
#![allow(clippy::too_many_lines)]

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;
use proptest::sample::SizeRange;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::canon::filter_pg_defaults::type_default_storage;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::cluster::catalog::ClusterCatalog;
use pgevolve_core::ir::cluster::role::{Role, RoleAttributes};
use pgevolve_core::ir::column::{Column, Compression, StorageKind};
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::constraint::{Constraint, ConstraintKind, Deferrable};
use pgevolve_core::ir::default_expr::NormalizedExpr;
use pgevolve_core::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
use pgevolve_core::ir::grant::{Grant, GrantTarget, Privilege};
use pgevolve_core::ir::index::{
    Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
};
use pgevolve_core::ir::policy::{Policy, PolicyCommand};
use pgevolve_core::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};
use pgevolve_core::ir::subscription::{OriginMode, StreamingMode, Subscription, SubscriptionOptions};
use pgevolve_core::ir::reloptions::{
    AutovacuumOptions, IndexStorageOptions, NotNanF64, TableStorageOptions,
};
use pgevolve_core::ir::schema::Schema;
use pgevolve_core::ir::sequence::Sequence;
use pgevolve_core::ir::table::Table;
use pgevolve_core::ir::view::View;
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};

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

            // Default-privilege rules for the catalog.
            let default_privs_strategy = arbitrary_default_privileges();

            // Publications drawing from the catalog's own schemas + tables.
            let schema_pool: Vec<Identifier> = schemas.iter().map(|s| s.name.clone()).collect();
            let table_pool: Vec<QualifiedName> = tables.iter().map(|t| t.qname.clone()).collect();
            let publications_strategy = arb_publications(schema_pool, table_pool);

            (
                index_strategies,
                seq_owner_grant_strategies,
                default_privs_strategy,
                publications_strategy,
            )
                .prop_flat_map(
                    move |(idx_per_table, seq_owner_grants, default_privileges, publications)| {
                        // Build the publication-name pool for subscription generation
                        // from the publications just generated for this catalog.
                        let pub_name_pool: Vec<Identifier> =
                            publications.iter().map(|p| p.name.clone()).collect();
                        let subscriptions_strategy = arb_subscriptions(pub_name_pool);

                        let indexes_c = idx_per_table;
                        let schemas_c = schemas.clone();
                        let tables_c = tables.clone();

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
                            catalog
                                .canonicalize()
                                .expect("generator produced invalid catalog")
                        })
                    },
                )
        })
}

fn arbitrary_schemas(cfg: &IRGeneratorConfig) -> impl Strategy<Value = Vec<Schema>> + use<> {
    let count = SizeRange::from(cfg.schema_count_range.0..=cfg.schema_count_range.1);
    proptest_vec(
        (
            schema_name_strategy(),
            arb_owner(),
            arb_object_grants(SCHEMA_PRIVS),
        ),
        count,
    )
    .prop_map(|triples| {
        // Deduplicate while preserving order: HashSet would lose ordering.
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for (name, owner, grants) in triples {
            if seen.insert(name.clone()) {
                let mut schema = Schema::new(name);
                schema.owner = owner;
                schema.grants = grants;
                out.push(schema);
            }
        }
        out
    })
}

fn schema_name_strategy() -> impl Strategy<Value = Identifier> {
    prop_oneof![
        Just("app"),
        Just("billing"),
        Just("audit"),
        Just("inventory"),
        Just("auth"),
        Just("ops"),
    ]
    .prop_map(|s| Identifier::from_unquoted(s).unwrap())
}

fn arbitrary_tables_for_schema(
    schema: Identifier,
    cfg: &IRGeneratorConfig,
) -> impl Strategy<Value = Vec<Table>> + use<> {
    let count = SizeRange::from(cfg.tables_per_schema_range.0..=cfg.tables_per_schema_range.1);
    let cfg = cfg.clone();
    proptest_vec(table_name_strategy(), count).prop_flat_map(move |raw_names| {
        let schema = schema.clone();
        let cfg = cfg.clone();
        // De-dup names within the same schema (canonicalize will reject dups).
        let mut seen = std::collections::BTreeSet::new();
        let names: Vec<Identifier> = raw_names
            .into_iter()
            .filter(|n| seen.insert(n.clone()))
            .collect();
        let strategies: Vec<_> = names
            .into_iter()
            .map(|name| arbitrary_table(schema.clone(), name, &cfg))
            .collect();
        strategies
    })
}

fn table_name_strategy() -> impl Strategy<Value = Identifier> {
    prop_oneof![
        Just("users"),
        Just("orders"),
        Just("items"),
        Just("invoices"),
        Just("widgets"),
        Just("events"),
        Just("payments"),
        Just("settings"),
    ]
    .prop_map(|s| Identifier::from_unquoted(s).unwrap())
}

fn arbitrary_table(
    schema: Identifier,
    name: Identifier,
    cfg: &IRGeneratorConfig,
) -> impl Strategy<Value = Table> + use<> {
    let cfg = cfg.clone();
    let col_count = SizeRange::from(cfg.columns_per_table_range.0..=cfg.columns_per_table_range.1);
    (
        proptest_vec(arbitrary_non_pk_column(), col_count),
        arb_owner(),
        arb_table_grants(TABLE_PRIVS),
        any::<bool>(),                    // rls_enabled
        any::<bool>(),                    // rls_forced
        proptest_vec(arb_policy(), 0..3), // policies
        arb_table_storage(),              // reloptions
    )
        .prop_map(
            move |(non_pk_cols, owner, grants, rls_enabled, rls_forced, mut policies, storage)| {
                let qname = QualifiedName::new(schema.clone(), name.clone());
                // Always include an `id bigint NOT NULL` PK column first.
                let id_col = Column {
                    name: Identifier::from_unquoted("id").unwrap(),
                    ty: ColumnType::BigInt,
                    nullable: false,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    storage: None,
                    compression: None,
                    comment: None,
                };
                // Avoid name collisions with id.
                let mut seen = std::collections::BTreeSet::new();
                seen.insert("id".to_string());
                let mut cols = vec![id_col];
                for c in non_pk_cols {
                    if seen.insert(c.name.as_str().to_string()) {
                        cols.push(c);
                    }
                }
                let pk = Constraint {
                    qname: QualifiedName::new(
                        schema.clone(),
                        Identifier::from_unquoted(&format!("{name}_pkey")).unwrap(),
                    ),
                    kind: ConstraintKind::PrimaryKey {
                        columns: vec![Identifier::from_unquoted("id").unwrap()],
                        include: vec![],
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                };
                // Deduplicate policy names (proptest may generate duplicates across
                // the 0..3 vector; canon requires unique names per table).
                let mut policy_names_seen = std::collections::BTreeSet::new();
                policies.retain(|p| policy_names_seen.insert(p.name.as_str().to_string()));
                Table {
                    qname,
                    columns: cols,
                    constraints: vec![pk],
                    partition_by: None,
                    partition_of: None,
                    comment: None,
                    owner,
                    grants,
                    rls_enabled,
                    rls_forced,
                    policies,
                    storage,
                }
            },
        )
}

// ---------------------------------------------------------------------------
// v0.3.2: RLS policy generators
// ---------------------------------------------------------------------------

/// Small fixed pool of policy names (SQL-safe, short, distinct).
const POLICY_NAMES: &[&str] = &[
    "allow_select",
    "allow_insert",
    "tenant_isolation",
    "owner_policy",
    "admin_bypass",
    "audit_row",
];

/// Generate a random [`PolicyCommand`].
fn arb_policy_command() -> impl Strategy<Value = PolicyCommand> {
    prop_oneof![
        Just(PolicyCommand::All),
        Just(PolicyCommand::Select),
        Just(PolicyCommand::Insert),
        Just(PolicyCommand::Update),
        Just(PolicyCommand::Delete),
    ]
}

/// Generate a random [`Policy`].
///
/// Respects the PG rule that `WITH CHECK` is invalid on `FOR SELECT` and
/// `FOR DELETE` policies — generated policies never produce invalid SQL.
/// Names are drawn from a small fixed pool so deduplication in
/// `arbitrary_table` is effective.
fn arb_policy() -> BoxedStrategy<Policy> {
    let name_strategy = prop_oneof![
        Just(POLICY_NAMES[0]),
        Just(POLICY_NAMES[1]),
        Just(POLICY_NAMES[2]),
        Just(POLICY_NAMES[3]),
        Just(POLICY_NAMES[4]),
        Just(POLICY_NAMES[5]),
    ]
    .prop_map(|s| Identifier::from_unquoted(s).unwrap());

    let roles_strategy = proptest_vec(
        prop_oneof![
            Just(GrantTarget::Public),
            prop_oneof![
                Just(GRANTEE_ROLE_NAMES[0]),
                Just(GRANTEE_ROLE_NAMES[1]),
                Just(GRANTEE_ROLE_NAMES[2]),
                Just(GRANTEE_ROLE_NAMES[3]),
                Just(GRANTEE_ROLE_NAMES[4]),
                Just(GRANTEE_ROLE_NAMES[5]),
            ]
            .prop_map(|s| GrantTarget::Role(Identifier::from_unquoted(s).unwrap())),
        ],
        0..3,
    );

    (
        name_strategy,
        any::<bool>(),
        arb_policy_command(),
        roles_strategy,
    )
        .prop_flat_map(|(name, permissive, command, mut roles)| {
            // Normalize roles: ensure non-empty (PG omission → PUBLIC) and deduplicate.
            if roles.is_empty() {
                roles.push(GrantTarget::Public);
            }
            roles.sort();
            roles.dedup();

            // WITH CHECK is only valid for ALL / INSERT / UPDATE.
            let with_check_strategy: BoxedStrategy<Option<NormalizedExpr>> =
                if command.allows_with_check() {
                    prop_oneof![
                        Just(None),
                        Just(Some(NormalizedExpr::from_canonical_text("true"))),
                    ]
                    .boxed()
                } else {
                    Just(None).boxed()
                };

            let using_strategy: BoxedStrategy<Option<NormalizedExpr>> = prop_oneof![
                Just(None),
                Just(Some(NormalizedExpr::from_canonical_text("true"))),
            ]
            .boxed();

            (
                Just(name),
                Just(permissive),
                Just(command),
                Just(roles),
                using_strategy,
                with_check_strategy,
            )
        })
        .prop_map(
            |(name, permissive, command, roles, using, with_check)| Policy {
                name,
                permissive,
                command,
                roles,
                using,
                with_check,
            },
        )
        .boxed()
}

fn arbitrary_non_pk_column() -> impl Strategy<Value = Column> {
    (
        column_name_strategy(),
        arbitrary_column_type(),
        any::<bool>(),
    )
        .prop_flat_map(|(name, ty, nullable)| {
            (arb_storage(&ty), arb_compression(&ty)).prop_map(move |(storage, compression)| {
                Column {
                    name: name.clone(),
                    ty: ty.clone(),
                    nullable,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    storage,
                    compression,
                    comment: None,
                }
            })
        })
}

/// Generate a random `STORAGE` strategy that is type-aware.
///
/// Toastable types (those whose PG default is not `PLAIN`) may be assigned
/// any of the four storage variants. Fixed-width types (default `PLAIN`) only
/// yield `None` or `Some(PLAIN)` — the others are illegal for those types.
fn arb_storage(ty: &ColumnType) -> BoxedStrategy<Option<StorageKind>> {
    let is_toastable = !matches!(type_default_storage(ty), StorageKind::Plain);
    if is_toastable {
        prop_oneof![
            Just(None),
            Just(Some(StorageKind::Plain)),
            Just(Some(StorageKind::External)),
            Just(Some(StorageKind::Extended)),
            Just(Some(StorageKind::Main)),
        ]
        .boxed()
    } else {
        prop_oneof![Just(None), Just(Some(StorageKind::Plain))].boxed()
    }
}

/// Generate a random `COMPRESSION` strategy that is type-aware.
///
/// Postgres rejects `COMPRESSION` on column types that aren't TOAST-able
/// (`column data type X does not support compression`). Mirrors
/// [`arb_storage`]: toastable types may carry any compression codec or
/// `None`; fixed-width types only yield `None`.
fn arb_compression(ty: &ColumnType) -> BoxedStrategy<Option<Compression>> {
    let is_toastable = !matches!(type_default_storage(ty), StorageKind::Plain);
    if is_toastable {
        prop_oneof![
            Just(None),
            Just(Some(Compression::Pglz)),
            Just(Some(Compression::Lz4)),
        ]
        .boxed()
    } else {
        Just(None).boxed()
    }
}

// ---------------------------------------------------------------------------
// v0.3.3: reloptions generators
// ---------------------------------------------------------------------------

/// Generate 0–3 populated autovacuum option fields.
///
/// Uses `NotNanF64::new` which returns `Ok` for all finite floats; the range
/// `0.0..1.0` never produces NaN, so the `unwrap` carries a justifying comment
/// matching the style used throughout this module for literal-bounded inputs.
fn arb_autovacuum_options() -> impl Strategy<Value = AutovacuumOptions> {
    (
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // enabled
        prop_oneof![Just(None), (0u64..1000).prop_map(Some)],         // vacuum_threshold
        // 0.0..1.0 is never NaN — unwrap is safe.
        prop_oneof![
            Just(None),
            (0.0f64..1.0).prop_map(|f| Some(NotNanF64::new(f).unwrap())),
        ], // vacuum_scale_factor
    )
        .prop_map(
            |(enabled, vacuum_threshold, vacuum_scale_factor)| AutovacuumOptions {
                enabled,
                vacuum_threshold,
                vacuum_scale_factor,
                ..Default::default()
            },
        )
}

/// Generate random [`TableStorageOptions`] with 0–3 fields set.
///
/// fillfactor range 10..=100 matches PG's documented valid range for tables.
fn arb_table_storage() -> impl Strategy<Value = TableStorageOptions> {
    (
        prop_oneof![Just(None), (10u32..=100).prop_map(Some)], // fillfactor
        arb_autovacuum_options(),
        prop_oneof![Just(None), (0u32..=64).prop_map(Some)], // parallel_workers
    )
        .prop_map(
            |(fillfactor, autovacuum, parallel_workers)| TableStorageOptions {
                fillfactor,
                autovacuum,
                parallel_workers,
                ..Default::default()
            },
        )
}

/// Generate random [`IndexStorageOptions`] with 0–2 fields set.
///
/// fillfactor range 50..=100 is B-tree–safe (the only method generated by
/// `arbitrary_indexes_for_table`). fastupdate is randomly toggled.
fn arb_index_storage() -> impl Strategy<Value = IndexStorageOptions> {
    (
        prop_oneof![Just(None), (50u32..=100).prop_map(Some)], // fillfactor (B-tree range)
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // fastupdate
    )
        .prop_map(|(fillfactor, fastupdate)| IndexStorageOptions {
            fillfactor,
            fastupdate,
            ..Default::default()
        })
}

// ---------------------------------------------------------------------------
// v0.3.1: owner + grants helpers
// ---------------------------------------------------------------------------

/// Small fixed pool of role names used for generating owners and grantees.
/// Overlaps with `ROLE_NAMES` so cluster-link lint tests can cross-reference.
const GRANTEE_ROLE_NAMES: &[&str] = &["app_owner", "readers", "writers", "app", "ops", "auditor"];

/// Generate an optional owner — `None` (unmanaged) or `Some(role)` from a
/// small pool.
fn arb_owner() -> impl Strategy<Value = Option<Identifier>> {
    prop_oneof![
        Just(None),
        prop_oneof![
            Just("app_owner"),
            Just("readers"),
            Just("writers"),
            Just("app"),
            Just("ops"),
        ]
        .prop_map(|s| Some(Identifier::from_unquoted(s).unwrap())),
    ]
}

/// Generate a single [`Grant`] for an object-level privilege.
///
/// `privileges` must be the slice of privileges valid for the target object
/// kind. `with_columns` controls whether column restrictions may appear.
fn arb_grant_from(privileges: &'static [Privilege], with_columns: bool) -> BoxedStrategy<Grant> {
    let grantee_strategy = prop_oneof![
        Just(GrantTarget::Public),
        prop_oneof![
            Just(GRANTEE_ROLE_NAMES[0]),
            Just(GRANTEE_ROLE_NAMES[1]),
            Just(GRANTEE_ROLE_NAMES[2]),
            Just(GRANTEE_ROLE_NAMES[3]),
            Just(GRANTEE_ROLE_NAMES[4]),
            Just(GRANTEE_ROLE_NAMES[5]),
        ]
        .prop_map(|s| GrantTarget::Role(Identifier::from_unquoted(s).unwrap())),
    ];

    let priv_strategy: BoxedStrategy<Privilege> = {
        // Build a prop_oneof from the slice by cycling through it.
        // `privileges` is &'static so we can capture it safely.
        match privileges {
            [a] => Just(*a).boxed(),
            [a, b] => prop_oneof![Just(*a), Just(*b)].boxed(),
            [a, b, c] => prop_oneof![Just(*a), Just(*b), Just(*c)].boxed(),
            [a, b, c, d] => prop_oneof![Just(*a), Just(*b), Just(*c), Just(*d)].boxed(),
            [a, b, c, d, e] => {
                prop_oneof![Just(*a), Just(*b), Just(*c), Just(*d), Just(*e)].boxed()
            }
            [a, b, c, d, e, f] => {
                prop_oneof![Just(*a), Just(*b), Just(*c), Just(*d), Just(*e), Just(*f)].boxed()
            }
            [a, b, c, d, e, f, g] => prop_oneof![
                Just(*a),
                Just(*b),
                Just(*c),
                Just(*d),
                Just(*e),
                Just(*f),
                Just(*g),
            ]
            .boxed(),
            _ => Just(privileges[0]).boxed(),
        }
    };

    // Optional column restriction (only meaningful for table/view/mv).
    let col_strategy: BoxedStrategy<Option<Vec<Identifier>>> = if with_columns {
        prop_oneof![
            Just(None),
            prop_oneof![
                Just(vec!["id"]),
                Just(vec!["name"]),
                Just(vec!["email"]),
                Just(vec!["id", "name"]),
            ]
            .prop_map(|cols| {
                Some(
                    cols.into_iter()
                        .map(|c| Identifier::from_unquoted(c).unwrap())
                        .collect(),
                )
            }),
        ]
        .boxed()
    } else {
        Just(None).boxed()
    };

    (grantee_strategy, priv_strategy, any::<bool>(), col_strategy)
        .prop_map(|(grantee, privilege, with_grant_option, columns)| {
            // PG rejects column-level grants for privileges that aren't
            // column-eligible: only SELECT/INSERT/UPDATE/REFERENCES may
            // appear with a column subset. Drop the columns subset when
            // the rolled privilege isn't one of those, instead of
            // re-rolling the strategy.
            let columns = match (columns, privilege) {
                (
                    Some(cols),
                    Privilege::Select
                    | Privilege::Insert
                    | Privilege::Update
                    | Privilege::References,
                ) => Some(cols),
                _ => None,
            };
            Grant {
                grantee,
                privilege,
                with_grant_option,
                columns,
            }
        })
        .boxed()
}

/// Generate a `Vec<Grant>` of 0–3 grants for a non-column-level object.
fn arb_object_grants(privileges: &'static [Privilege]) -> impl Strategy<Value = Vec<Grant>> {
    prop_oneof![
        Just(vec![]),
        arb_grant_from(privileges, false).prop_map(|g| vec![g]),
        (
            arb_grant_from(privileges, false),
            arb_grant_from(privileges, false)
        )
            .prop_map(|(a, b)| vec![a, b]),
        (
            arb_grant_from(privileges, false),
            arb_grant_from(privileges, false),
            arb_grant_from(privileges, false)
        )
            .prop_map(|(a, b, c)| vec![a, b, c]),
    ]
}

/// Generate a `Vec<Grant>` of 0–3 grants for table/view/mv (may include
/// column restrictions).
fn arb_table_grants(privileges: &'static [Privilege]) -> impl Strategy<Value = Vec<Grant>> {
    prop_oneof![
        Just(vec![]),
        arb_grant_from(privileges, true).prop_map(|g| vec![g]),
        (
            arb_grant_from(privileges, true),
            arb_grant_from(privileges, true)
        )
            .prop_map(|(a, b)| vec![a, b]),
        (
            arb_grant_from(privileges, true),
            arb_grant_from(privileges, true),
            arb_grant_from(privileges, true)
        )
            .prop_map(|(a, b, c)| vec![a, b, c]),
    ]
}

/// Table privileges (SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER).
const TABLE_PRIVS: &[Privilege] = &[
    Privilege::Select,
    Privilege::Insert,
    Privilege::Update,
    Privilege::Delete,
    Privilege::Truncate,
    Privilege::References,
    Privilege::Trigger,
];

/// Schema privileges (USAGE, CREATE).
const SCHEMA_PRIVS: &[Privilege] = &[Privilege::Usage, Privilege::Create];

/// Sequence privileges (USAGE, SELECT, UPDATE).
const SEQUENCE_PRIVS: &[Privilege] = &[Privilege::Usage, Privilege::Select, Privilege::Update];

/// Function/procedure privileges (EXECUTE only).
/// Prepared for future function/procedure generators; unused until those ship.
#[allow(dead_code)]
const FUNCTION_PRIVS: &[Privilege] = &[Privilege::Execute];

/// Type privileges (USAGE only).
/// Prepared for future user-type generators; unused until those ship.
#[allow(dead_code)]
const TYPE_PRIVS: &[Privilege] = &[Privilege::Usage];

/// Generate 0–3 [`DefaultPrivilegeRule`]s for arbitrary catalog-level rules.
pub fn arbitrary_default_privileges() -> impl Strategy<Value = Vec<DefaultPrivilegeRule>> {
    let rule_strategy = (
        // target_role
        prop_oneof![Just("app_owner"), Just("app"), Just("ops"),]
            .prop_map(|s| Identifier::from_unquoted(s).unwrap()),
        // schema (None = all schemas)
        prop_oneof![Just(None), Just(Some("app")), Just(Some("billing")),]
            .prop_map(|s| s.map(|n| Identifier::from_unquoted(n).unwrap())),
        // object_type
        prop_oneof![
            Just(DefaultPrivObjectType::Tables),
            Just(DefaultPrivObjectType::Sequences),
            Just(DefaultPrivObjectType::Functions),
            Just(DefaultPrivObjectType::Types),
            Just(DefaultPrivObjectType::Schemas),
        ],
        // grants within the rule (0–2)
        prop_oneof![
            Just(vec![]),
            prop_oneof![
                Just(GrantTarget::Public),
                Just(GrantTarget::Role(
                    Identifier::from_unquoted("readers").unwrap()
                )),
                Just(GrantTarget::Role(
                    Identifier::from_unquoted("writers").unwrap()
                )),
            ]
            .prop_map(|grantee| vec![Grant {
                grantee,
                privilege: Privilege::Select,
                with_grant_option: false,
                columns: None,
            }]),
        ],
    )
        .prop_map(
            |(target_role, schema, object_type, grants)| DefaultPrivilegeRule {
                target_role,
                schema,
                object_type,
                grants,
            },
        );

    prop_oneof![
        Just(vec![]),
        rule_strategy.clone().prop_map(|r| vec![r]),
        (rule_strategy.clone(), rule_strategy.clone()).prop_map(|(a, b)| vec![a, b]),
        (rule_strategy.clone(), rule_strategy.clone(), rule_strategy)
            .prop_map(|(a, b, c)| vec![a, b, c]),
    ]
}

fn column_name_strategy() -> impl Strategy<Value = Identifier> {
    prop_oneof![
        Just("name"),
        Just("email"),
        Just("amount"),
        Just("status"),
        Just("created_at"),
        Just("updated_at"),
        Just("deleted_at"),
        Just("org_id"),
        Just("user_id"),
        Just("notes"),
        Just("count"),
        Just("price"),
        Just("active"),
    ]
    .prop_map(|s| Identifier::from_unquoted(s).unwrap())
}

/// Sample of v0.1 [`ColumnType`] variants. Not exhaustive (no Array,
/// `UserDefined`, Other, Bit, `NetAddress`) — those are added in v0.1.x
/// once the round-trip tests are stable on the common types.
pub fn arbitrary_column_type() -> impl Strategy<Value = ColumnType> {
    prop_oneof![
        Just(ColumnType::Boolean),
        Just(ColumnType::SmallInt),
        Just(ColumnType::Integer),
        Just(ColumnType::BigInt),
        Just(ColumnType::Real),
        Just(ColumnType::DoublePrecision),
        Just(ColumnType::Text),
        Just(ColumnType::Uuid),
        Just(ColumnType::Json),
        Just(ColumnType::Jsonb),
        Just(ColumnType::Date),
        Just(ColumnType::Bytea),
        Just(ColumnType::Numeric {
            precision: None,
            scale: None,
        }),
        Just(ColumnType::Timestamp {
            precision: None,
            with_tz: true,
        }),
        Just(ColumnType::Varchar { len: Some(255) }),
    ]
}

fn arbitrary_indexes_for_table(
    table: &Table,
    _table_count: usize,
) -> impl Strategy<Value = Vec<Index>> + use<> {
    // Build a list of candidate columns; sample 0-2 of them and produce an
    // index per pick. Filter to btree-indexable types: `json` famously has
    // no default btree opclass; the same applies to a few other v0.1 types.
    let qname = table.qname.clone();
    let columns: Vec<Identifier> = table
        .columns
        .iter()
        .filter(|c| is_btree_indexable(&c.ty))
        .map(|c| c.name.clone())
        .collect();

    // Generate up to 3 (pick, storage) pairs — one per candidate index slot.
    // The strategy generates a fixed-length array of 3 pairs and the pick
    // vector selects up to 3 of them; extra pairs are simply unused.
    (
        proptest_vec(0usize..columns.len().max(1), 0..3),
        [
            arb_index_storage(),
            arb_index_storage(),
            arb_index_storage(),
        ],
    )
        .prop_map(move |(picks, storages)| {
            let mut out = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            for (n, pick) in picks.into_iter().enumerate() {
                let Some(col) = columns.get(pick) else {
                    continue;
                };
                let idx_name =
                    Identifier::from_unquoted(&format!("{}_{n}_idx", qname.name.as_str())).unwrap();
                if !seen.insert(idx_name.clone()) {
                    continue;
                }
                let storage = storages[n].clone();
                out.push(Index {
                    qname: QualifiedName::new(qname.schema.clone(), idx_name),
                    on: IndexParent::Table(qname.clone()),
                    method: IndexMethod::BTree,
                    columns: vec![IndexColumn {
                        expr: IndexColumnExpr::Column(col.clone()),
                        collation: None,
                        opclass: None,
                        sort_order: SortOrder::Asc,
                        nulls_order: NullsOrder::NullsLast,
                    }],
                    include: vec![],
                    unique: false,
                    nulls_not_distinct: false,
                    predicate: None,
                    tablespace: None,
                    comment: None,
                    storage,
                });
            }
            out
        })
}

/// Type whitelist for btree-default indexability. `json` is the notable
/// exclusion (use `jsonb` instead); arrays and a few exotic types are also
/// not indexable with the default operator class.
///
/// Used by the IR generator (when seeding new indexes) AND the IR mutator
/// (when adding indexes via `add_index`). Keeping the two in sync is
/// essential — otherwise the mutator can produce btree indexes on `json`
/// columns that PG rejects with error 42704 ("data type json has no default
/// operator class for access method 'btree'"). See
/// `crates/pgevolve-testkit/src/ir_mutator.rs::add_index`.
pub(crate) const fn is_btree_indexable(ty: &ColumnType) -> bool {
    !matches!(ty, ColumnType::Json)
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
                    // Sprinkle random owner + grants onto each view.
                    let n = views.len();
                    let owner_strategies: Vec<_> = (0..n).map(|_| arb_owner()).collect();
                    let grant_strategies: Vec<_> =
                        (0..n).map(|_| arb_table_grants(TABLE_PRIVS)).collect();
                    (owner_strategies, grant_strategies).prop_map(move |(owners, grant_vecs)| {
                        let mut views_with_grants: Vec<View> = views.clone();
                        for (i, v) in views_with_grants.iter_mut().enumerate() {
                            v.owner = owners[i].clone();
                            v.grants = grant_vecs[i].clone();
                        }
                        let mut cat = catalog.clone();
                        cat.views = views_with_grants;
                        // canonicalize sorts views and checks for duplicates.
                        cat.canonicalize()
                            .expect("view generator produced invalid catalog")
                    })
                })
        })
    })
}

// ---------------------------------------------------------------------------
// v0.3.4: publication generators
// ---------------------------------------------------------------------------

/// Small fixed pool of publication names (SQL-safe, short, distinct).
const PUB_NAMES: &[&str] = &["pub_a", "pub_b", "pub_c"];

/// Generate a random [`PublishKinds`] with at least one DML kind enabled.
fn arb_publish_kinds() -> impl Strategy<Value = PublishKinds> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>())
        .prop_filter("at least one DML kind", |(i, u, d, t)| *i || *u || *d || *t)
        .prop_map(|(insert, update, delete, truncate)| PublishKinds {
            insert,
            update,
            delete,
            truncate,
        })
}

/// Generate a random [`PublicationScope`] drawn from the provided pools.
///
/// The `schema_pool` and `table_pool` are the catalog's actual schemas and
/// tables so that generated publications always reference real objects.
fn arb_publication_scope(
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> BoxedStrategy<PublicationScope> {
    if schema_pool.is_empty() && table_pool.is_empty() {
        // Degenerate: no objects to reference — fall back to AllTables.
        return Just(PublicationScope::AllTables).boxed();
    }
    let sp = schema_pool.clone();
    let tp = table_pool.clone();
    prop_oneof![
        Just(PublicationScope::AllTables),
        (
            proptest::sample::subsequence(sp, 0..=(schema_pool.len())),
            proptest::sample::subsequence(tp, 0..=(table_pool.len())),
        )
            .prop_filter("non-empty Selective", |(s, t)| !s.is_empty()
                || !t.is_empty())
            .prop_map(|(schemas, tables)| {
                let schemas = schemas.into_iter().collect();
                let tables = tables
                    .into_iter()
                    .map(|qname| PublishedTable {
                        qname,
                        row_filter: None,
                        columns: None,
                    })
                    .collect();
                PublicationScope::Selective { schemas, tables }
            })
    ]
    .boxed()
}

/// Generate a single [`Publication`] with a name drawn from `pub_name_idx`
/// into `PUB_NAMES`, a random scope drawn from the provided pools, and
/// random publish/via-root flags.
fn arb_publication_inner(
    pub_name_idx: usize,
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> impl Strategy<Value = Publication> {
    let name = Identifier::from_unquoted(PUB_NAMES[pub_name_idx]).unwrap();
    (
        arb_publication_scope(schema_pool, table_pool),
        arb_publish_kinds(),
        any::<bool>(), // publish_via_partition_root
    )
        .prop_map(
            move |(scope, publish, publish_via_partition_root)| Publication {
                name: name.clone(),
                scope,
                publish,
                publish_via_partition_root,
                owner: None,
                comment: None,
            },
        )
}

/// Generate 0–2 [`Publication`]s with distinct names drawn from `PUB_NAMES`.
///
/// Publication names are globally unique (not schema-qualified) in PG.
/// The strategy generates at most `PUB_NAMES.len()` publications and ensures
/// name uniqueness by picking distinct indices from the pool.
fn arb_publications(
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> BoxedStrategy<Vec<Publication>> {
    let max = PUB_NAMES.len().min(2); // 0..=2 publications
    (0usize..=max)
        .prop_flat_map(move |count| {
            let sp = schema_pool.clone();
            let tp = table_pool.clone();
            proptest::sample::subsequence((0..PUB_NAMES.len()).collect::<Vec<_>>(), count..=count)
                .prop_flat_map(move |indices| {
                    let sp = sp.clone();
                    let tp = tp.clone();
                    let strategies: Vec<_> = indices
                        .into_iter()
                        .map(|idx| arb_publication_inner(idx, sp.clone(), tp.clone()))
                        .collect();
                    strategies
                })
        })
        .boxed()
}

// ---------------------------------------------------------------------------
// v0.3.5: subscription generators
// ---------------------------------------------------------------------------

/// Small fixed pool of subscription names (SQL-safe, short, distinct).
const SUB_NAMES: &[&str] = &["sub_a", "sub_b", "sub_c"];

/// Generate a random [`StreamingMode`].
fn arb_streaming_mode() -> impl Strategy<Value = StreamingMode> {
    prop_oneof![
        Just(StreamingMode::Off),
        Just(StreamingMode::On),
        Just(StreamingMode::Parallel),
    ]
}

/// Generate a random [`OriginMode`].
fn arb_origin_mode() -> impl Strategy<Value = OriginMode> {
    prop_oneof![Just(OriginMode::Any), Just(OriginMode::None)]
}

/// Generate random [`SubscriptionOptions`] with selected fields set.
///
/// CREATE-only fields (`create_slot`, `copy_data`) and PG-version-gated
/// fields (`password_required`, `run_as_owner`) are left `None` to keep
/// generation simple and lint-clean. `synchronous_commit` is also left
/// `None` (free-form string; no bounded pool to sample from).
fn arb_subscription_options() -> impl Strategy<Value = SubscriptionOptions> {
    (
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // enabled
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // binary
        prop_oneof![Just(None), arb_streaming_mode().prop_map(Some)], // streaming
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // two_phase
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // disable_on_error
        prop_oneof![Just(None), arb_origin_mode().prop_map(Some)],   // origin
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))], // failover
    )
        .prop_map(
            |(enabled, binary, streaming, two_phase, disable_on_error, origin, failover)| {
                SubscriptionOptions {
                    enabled,
                    slot_name: None,
                    create_slot: None,
                    copy_data: None,
                    synchronous_commit: None,
                    binary,
                    streaming,
                    two_phase,
                    disable_on_error,
                    password_required: None,
                    run_as_owner: None,
                    origin,
                    failover,
                }
            },
        )
}

/// Generate a single [`Subscription`] with a name from `sub_name_idx` into
/// `SUB_NAMES`, 1–3 publications drawn from `publication_pool`, and random
/// options. Connection string uses a `${TEST_PWD}` placeholder so it lints
/// clean (`subscription-password-in-source` only fires on plaintext values).
fn arb_subscription_inner(
    sub_name_idx: usize,
    publication_pool: Vec<Identifier>,
) -> impl Strategy<Value = Subscription> {
    let name = Identifier::from_unquoted(SUB_NAMES[sub_name_idx]).unwrap();
    // Pick 1–3 publications from the pool; fall back to a synthetic name when
    // the pool is empty so the strategy always produces a valid Subscription.
    let pubs_strategy: BoxedStrategy<Vec<Identifier>> = if publication_pool.is_empty() {
        Just(vec![Identifier::from_unquoted("pub_a").unwrap()]).boxed()
    } else {
        let max_pick = 3usize.min(publication_pool.len());
        proptest::sample::subsequence(publication_pool, 1..=max_pick)
            .prop_map(|mut v| {
                v.sort();
                v.dedup();
                v
            })
            .boxed()
    };
    (pubs_strategy, arb_subscription_options()).prop_map(move |(publications, options)| {
        Subscription {
            name: name.clone(),
            // Synthetic connection string with a ${VAR} placeholder for the
            // password. The strategy doesn't vary the connection text — every
            // generated subscription uses a benign placeholder that lints clean
            // (subscription-password-in-source fires only on plaintext values).
            connection: "host=replica.example.com dbname=app user=repl password=${TEST_PWD}"
                .to_string(),
            publications,
            options,
            owner: None,
            comment: None,
        }
    })
}

/// Generate 0–1 [`Subscription`]s with names drawn from `SUB_NAMES`.
///
/// 0–1 (not 0–2+) keeps the generated catalog lightweight and avoids
/// subscription-name collisions with the small `SUB_NAMES` pool when the
/// publication pool is already non-empty.
fn arb_subscriptions(publication_pool: Vec<Identifier>) -> BoxedStrategy<Vec<Subscription>> {
    (0usize..=1usize)
        .prop_flat_map(move |count| {
            let pp = publication_pool.clone();
            proptest::sample::subsequence(
                (0..SUB_NAMES.len()).collect::<Vec<_>>(),
                count..=count,
            )
            .prop_flat_map(move |indices| {
                let pp = pp.clone();
                let strategies: Vec<_> = indices
                    .into_iter()
                    .map(|idx| arb_subscription_inner(idx, pp.clone()))
                    .collect();
                strategies
            })
        })
        .boxed()
}

fn stand_alone_sequence(schema: &Identifier) -> Sequence {
    Sequence {
        qname: QualifiedName::new(
            schema.clone(),
            Identifier::from_unquoted(&format!("{schema}_seq")).unwrap(),
        ),
        data_type: ColumnType::BigInt,
        start: 1,
        increment: 1,
        min_value: None,
        max_value: None,
        cache: 1,
        cycle: false,
        owned_by: None,
        comment: None,
        owner: None,
        grants: vec![],
    }
}

// ---------------------------------------------------------------------------
// v0.3: Cluster catalog generators
// ---------------------------------------------------------------------------

/// Candidate role name pool — short, distinct, SQL-safe identifiers.
const ROLE_NAMES: &[&str] = &[
    "app", "ops", "reader", "writer", "admin", "auditor", "analyst", "deploy",
];

/// Generate a [`RoleAttributes`] with uniformly-random boolean flags and an
/// optional `connection_limit` / `valid_until`.
pub fn arbitrary_role_attributes() -> impl Strategy<Value = RoleAttributes> {
    (
        any::<bool>(),                                               // superuser
        any::<bool>(),                                               // createdb
        any::<bool>(),                                               // createrole
        any::<bool>(),                                               // inherit
        any::<bool>(),                                               // login
        any::<bool>(),                                               // replication
        any::<bool>(),                                               // bypass_rls
        prop_oneof![Just(None), (1i64..=10_000i64).prop_map(Some),], // connection_limit
        prop_oneof![
            Just(None),
            Just(Some("2030-01-01T00:00:00Z".to_string())),
            Just(Some("2035-06-15T12:00:00Z".to_string())),
        ], // valid_until
    )
        .prop_map(
            |(
                superuser,
                createdb,
                createrole,
                inherit,
                login,
                replication,
                bypass_rls,
                connection_limit,
                valid_until,
            )| {
                RoleAttributes {
                    superuser,
                    createdb,
                    createrole,
                    inherit,
                    login,
                    replication,
                    bypass_rls,
                    connection_limit,
                    valid_until,
                }
            },
        )
}

/// Generate a single [`Role`] with a name drawn from `role_name_idx` into
/// `ROLE_NAMES`, random attributes, and `member_of` edges drawn exclusively
/// from `peer_name_indices` (indices into `ROLE_NAMES`).
///
/// The `peer_name_indices` slice contains the indices of roles that were
/// generated *before* this one in topological order — passing only earlier
/// roles' indices guarantees the resulting membership graph is acyclic.
fn arbitrary_role_inner(
    name_idx: usize,
    peer_name_indices: Vec<usize>,
) -> impl Strategy<Value = Role> {
    let name = Identifier::from_unquoted(ROLE_NAMES[name_idx]).unwrap();

    (
        arbitrary_role_attributes(),
        // A bitmask that selects a subset of `peer_name_indices` to add as
        // `member_of` edges.  Using a u16 caps at 16 peers which is well
        // above the pool size (8 roles max).
        any::<u16>(),
        prop_oneof![
            Just(None),
            ".*".prop_map(|s| if s.is_empty() { None } else { Some(s) }),
        ],
    )
        .prop_map(move |(attributes, peer_mask, comment)| {
            let member_of: Vec<Identifier> = peer_name_indices
                .iter()
                .enumerate()
                .filter_map(|(bit, &peer_idx)| {
                    if bit < 16 && (peer_mask >> bit) & 1 == 1 {
                        Some(Identifier::from_unquoted(ROLE_NAMES[peer_idx]).unwrap())
                    } else {
                        None
                    }
                })
                .collect();
            Role {
                name: name.clone(),
                attributes,
                member_of,
                comment,
            }
        })
}

/// Generate a [`ClusterCatalog`] with 0–`ROLE_NAMES.len()` roles.
///
/// Roles are generated in topological order (each role's `member_of`
/// references are drawn from roles earlier in the list only) to guarantee
/// the membership graph is acyclic.  The catalog is canonicalized before
/// being returned.
pub fn arbitrary_cluster_catalog() -> impl Strategy<Value = ClusterCatalog> {
    // First pick how many roles (0..=len) and which distinct names to use.
    let max = ROLE_NAMES.len();
    (0usize..=max)
        .prop_flat_map(move |count| {
            // Sample `count` distinct indices from 0..max.
            proptest_vec(0usize..max, count..=count).prop_map(move |mut indices| {
                // De-duplicate while preserving order.
                let mut seen = std::collections::BTreeSet::new();
                indices.retain(|i| seen.insert(*i));
                // Trim to `count` after dedup (may be shorter).
                indices
            })
        })
        .prop_flat_map(|indices| {
            // For each role at position `i`, its peers are `indices[0..i]`.
            let strategies: Vec<_> = indices
                .iter()
                .enumerate()
                .map(|(i, &name_idx)| {
                    let peers: Vec<usize> = indices[..i].to_vec();
                    arbitrary_role_inner(name_idx, peers)
                })
                .collect();
            strategies.prop_map(|roles| {
                let mut cat = ClusterCatalog { roles };
                cat.canonicalize();
                cat
            })
        })
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
