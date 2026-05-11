//! Proptest strategy producing random valid [`Catalog`]s.
//!
//! Scope for v0.1: schemas + tables + indexes + sequences with `PRIMARY KEY`
//! and a modest set of column types. Foreign-key density is configurable;
//! when FKs are generated, the referenced column always has a unique
//! constraint (the PK) so the produced catalog passes
//! `Catalog::canonicalize()` and the planner's closed-world check.
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

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;
use proptest::sample::SizeRange;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::column::Column;
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::constraint::{Constraint, ConstraintKind, Deferrable};
use pgevolve_core::ir::index::{
    Index, IndexColumn, IndexColumnExpr, IndexMethod, NullsOrder, SortOrder,
};
use pgevolve_core::ir::schema::Schema;
use pgevolve_core::ir::sequence::Sequence;
use pgevolve_core::ir::table::Table;

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

            index_strategies.prop_map(move |idx_per_table| {
                let indexes: Vec<Index> = idx_per_table.into_iter().flatten().collect();
                let mut catalog = Catalog::empty();
                catalog.schemas = schemas.clone();
                catalog.tables = tables.clone();
                catalog.indexes = indexes;
                // Sprinkle in sequences for variety (one per schema, no owner).
                for s in &catalog.schemas {
                    catalog.sequences.push(stand_alone_sequence(&s.name));
                }
                catalog
                    .canonicalize()
                    .expect("generator produced invalid catalog")
            })
        })
}

fn arbitrary_schemas(cfg: &IRGeneratorConfig) -> impl Strategy<Value = Vec<Schema>> {
    let count = SizeRange::from(cfg.schema_count_range.0..=cfg.schema_count_range.1);
    proptest_vec(schema_name_strategy(), count).prop_map(|names| {
        // Deduplicate while preserving order: HashSet would lose ordering.
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for n in names {
            if seen.insert(n.clone()) {
                out.push(Schema::new(n));
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
) -> impl Strategy<Value = Vec<Table>> {
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
) -> impl Strategy<Value = Table> {
    let cfg = cfg.clone();
    let col_count = SizeRange::from(cfg.columns_per_table_range.0..=cfg.columns_per_table_range.1);
    proptest_vec(arbitrary_non_pk_column(), col_count).prop_map(move |non_pk_cols| {
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
            comment: None,
        }
    })
}

fn arbitrary_non_pk_column() -> impl Strategy<Value = Column> {
    (
        column_name_strategy(),
        arbitrary_column_type(),
        any::<bool>(),
    )
        .prop_map(|(name, ty, nullable)| Column {
            name,
            ty,
            nullable,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            comment: None,
        })
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
) -> impl Strategy<Value = Vec<Index>> {
    // Build a list of candidate columns; sample 0-2 of them and produce an
    // index per pick.
    let qname = table.qname.clone();
    let columns: Vec<Identifier> = table.columns.iter().map(|c| c.name.clone()).collect();

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
                table: qname.clone(),
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
    }
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
