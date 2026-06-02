//! Index strategies. The B-tree opclass whitelist `is_btree_indexable` is
//! shared with the IR mutator so both code paths stay in sync.

#![allow(clippy::needless_pass_by_value)]

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::index::{
    Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
};
use pgevolve_core::ir::table::Table;

use super::reloptions::arb_index_storage;

pub(super) fn arbitrary_indexes_for_table(
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
    // All generated indexes use B-tree; pass the method so storage options are
    // gated to only those valid for B-tree (fillfactor and deduplicate_items).
    (
        proptest_vec(0usize..columns.len().max(1), 0..3),
        [
            arb_index_storage(IndexMethod::BTree),
            arb_index_storage(IndexMethod::BTree),
            arb_index_storage(IndexMethod::BTree),
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
