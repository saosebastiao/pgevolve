//! Table + column strategies, including the public [`arbitrary_column_type`].

#![allow(clippy::needless_pass_by_value)]

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;
use proptest::sample::SizeRange;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::column::Column;
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::constraint::{Constraint, ConstraintKind, Deferrable};
use pgevolve_core::ir::table::Table;

use super::IRGeneratorConfig;
use super::grants::{TABLE_PRIVS, arb_owner, arb_table_grants};
use super::policy::arb_policy;
use super::reloptions::{arb_compression, arb_storage, arb_table_storage};

pub(super) fn arbitrary_tables_for_schema(
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
    // Generate the non-PK columns first so the grant strategy can restrict
    // column-level grants to columns that actually exist on the table.
    // Without this, the grant generator used a hardcoded pool (["name",
    // "email", ...]) regardless of the table's actual columns, producing
    // grants that referenced non-existent columns and triggering PG error
    // `column "X" of relation "Y" does not exist` at apply time (issue #19).
    proptest_vec(arbitrary_non_pk_column(), col_count).prop_flat_map(move |non_pk_cols| {
        let schema = schema.clone();
        let name = name.clone();

        // Build the column list upfront (de-duplicating `id` collisions) so
        // we can pass the actual column names to the grant generator.
        let mut seen = std::collections::BTreeSet::new();
        seen.insert("id".to_string());
        let mut cols: Vec<Column> = vec![Column {
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
        }];
        for c in non_pk_cols {
            if seen.insert(c.name.as_str().to_string()) {
                cols.push(c);
            }
        }

        // Column pool for column-level grants: all column names in this table.
        let col_pool: Vec<Identifier> = cols.iter().map(|c| c.name.clone()).collect();

        (
            Just(cols),
            arb_owner(),
            arb_table_grants(TABLE_PRIVS, col_pool), // restricted to real columns
            any::<bool>(),                           // rls_enabled
            any::<bool>(),                           // rls_forced
            proptest_vec(arb_policy(), 0..3),        // policies
            arb_table_storage(),                     // reloptions
        )
            .prop_map(
                move |(cols, owner, grants, rls_enabled, rls_forced, mut policies, storage)| {
                    let qname = QualifiedName::new(schema.clone(), name.clone());
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
                    // Deduplicate policy names (proptest may generate duplicates
                    // across the 0..3 vector; canon requires unique names per table).
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
                        access_method: None,
                        tablespace: None,
                    }
                },
            )
    })
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
        Just(ColumnType::Numeric { precision: None }),
        Just(ColumnType::Timestamp {
            precision: None,
            with_tz: true,
        }),
        Just(ColumnType::Varchar { len: Some(255) }),
    ]
}
