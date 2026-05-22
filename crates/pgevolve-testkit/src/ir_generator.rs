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
use pgevolve_core::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
use pgevolve_core::ir::grant::{Grant, GrantTarget, Privilege};
use pgevolve_core::ir::index::{
    Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
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

            (
                index_strategies,
                seq_owner_grant_strategies,
                default_privs_strategy,
            )
                .prop_map(
                    move |(idx_per_table, seq_owner_grants, default_privileges)| {
                        let indexes: Vec<Index> = idx_per_table.into_iter().flatten().collect();
                        let mut catalog = Catalog::empty();
                        catalog.schemas = schemas.clone();
                        catalog.tables = tables.clone();
                        catalog.indexes = indexes;
                        // Sprinkle in sequences for variety (one per schema, with
                        // random owner + grants).
                        for (s, (seq_owner, seq_grants)) in
                            catalog.schemas.iter().zip(seq_owner_grants)
                        {
                            let mut seq = stand_alone_sequence(&s.name);
                            seq.owner = seq_owner;
                            seq.grants = seq_grants;
                            catalog.sequences.push(seq);
                        }
                        // Attach default-privilege rules (dedup by key before canon).
                        catalog.default_privileges = default_privileges;
                        catalog
                            .canonicalize()
                            .expect("generator produced invalid catalog")
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
    )
        .prop_map(move |(non_pk_cols, owner, grants)| {
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
            Table {
                qname,
                columns: cols,
                constraints: vec![pk],
                partition_by: None,
                partition_of: None,
                comment: None,
                owner,
                grants,
            }
        })
}

fn arbitrary_non_pk_column() -> impl Strategy<Value = Column> {
    (
        column_name_strategy(),
        arbitrary_column_type(),
        any::<bool>(),
    )
        .prop_flat_map(|(name, ty, nullable)| {
            (arb_storage(&ty), arb_compression()).prop_map(move |(storage, compression)| Column {
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

/// Generate a random `COMPRESSION` strategy (type-independent).
///
/// Picks uniformly from `{None, Pglz, Lz4}`. The caller is responsible for
/// only applying compression to columns that support it (toastable types);
/// for fixed-width columns this value is ignored by the differ and planner.
fn arb_compression() -> impl Strategy<Value = Option<Compression>> {
    prop_oneof![
        Just(None),
        Just(Some(Compression::Pglz)),
        Just(Some(Compression::Lz4)),
    ]
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
        .prop_map(|(grantee, privilege, with_grant_option, columns)| Grant {
            grantee,
            privilege,
            with_grant_option,
            columns,
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

    proptest_vec(0usize..columns.len().max(1), 0..3).prop_map(move |picks| {
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
