//! Proptest strategies producing random valid mutations of an existing
//! [`Catalog`].
//!
//! v0.1.x scope: simple object-level mutations that exercise every Phase 6
//! rewrite path. Each mutation function takes a [`Catalog`] and returns a
//! new one; the proptest entry point [`arbitrary_mutation`] picks a random
//! mutation kind, applies it, and re-canonicalizes. If the picked mutation
//! cannot apply (e.g., "drop table" when there are no tables) the generator
//! falls back to a no-op clone.
//!
//! Mutations covered:
//! - add a column to a random table
//! - drop a non-PK column
//! - toggle a column's `nullable`
//! - add an index on a random column of a random table
//! - drop a non-PK index
//! - add a new table (with PK only)
//! - drop a table (cascade: dependent indexes + sequences with `OWNED BY`)
//! - add a new schema
//! - drop a schema (cascade: all objects in the schema)
//!
//! FK / CHECK constraint mutations are deferred until the generator
//! produces FKs.

// Mutations clone heavily by design — proptest closures take ownership of
// the seed catalog and return owned new ones. Suppress the pedantic clones
// lint at module scope rather than peppering #[allow] through every arm.
#![allow(clippy::assigning_clones)]
#![allow(clippy::needless_pass_by_value)]

use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::column::Column;
use pgevolve_core::ir::index::{
    Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
};
use pgevolve_core::ir::schema::Schema;
use pgevolve_core::ir::sequence::Sequence;

use crate::ir_generator::{arbitrary_column_type, is_btree_indexable};

/// Produce a random valid mutation of `seed`.
///
/// Picks one mutation at random and applies it. If the picked mutation
/// cannot apply (no candidates to operate on), the strategy returns the
/// original catalog unchanged so the property test still has a valid output
/// to evolve in subsequent rounds.
pub fn arbitrary_mutation(seed: Catalog) -> impl Strategy<Value = Catalog> {
    // 9 mutation kinds, equally weighted.
    (
        0u8..9u8,
        arbitrary_column_type(),
        any::<bool>(),
        any::<usize>(),
    )
        .prop_map(move |(pick, ty, nullable, idx)| apply_mutation(&seed, pick, ty, nullable, idx))
}

fn apply_mutation(
    seed: &Catalog,
    kind: u8,
    ty: pgevolve_core::ir::column_type::ColumnType,
    nullable: bool,
    idx_seed: usize,
) -> Catalog {
    let mut c = seed.clone();
    match kind {
        0 => add_column(&mut c, ty, nullable, idx_seed),
        1 => drop_non_pk_column(&mut c, idx_seed),
        2 => toggle_nullable(&mut c, idx_seed),
        3 => add_index(&mut c, idx_seed),
        4 => drop_non_pk_index(&mut c, idx_seed),
        5 => add_table(&mut c, idx_seed),
        6 => drop_table(&mut c, idx_seed),
        7 => add_schema(&mut c, idx_seed),
        _ => drop_schema(&mut c, idx_seed),
    }
    // Canonicalize; if a mutation produced an invalid catalog (it shouldn't,
    // but defensively) fall back to the seed.
    c.canonicalize().unwrap_or_else(|_| seed.clone())
}

fn add_column(
    c: &mut Catalog,
    ty: pgevolve_core::ir::column_type::ColumnType,
    nullable: bool,
    seed: usize,
) {
    if c.tables.is_empty() {
        return;
    }
    let i = seed % c.tables.len();
    let table = &mut c.tables[i];
    // Synthesize a unique column name within the table.
    let name = unique_column_name(&table.columns, seed);
    table.columns.push(Column {
        name,
        ty,
        nullable,
        default: None,
        identity: None,
        generated: None,
        collation: None,
        storage: None,
        compression: None,
        comment: None,
    });
}

fn drop_non_pk_column(c: &mut Catalog, seed: usize) {
    if c.tables.is_empty() {
        return;
    }
    let i = seed % c.tables.len();
    let table = &mut c.tables[i];
    let pk_cols = pk_columns(table);
    // Indexes might refer to columns we drop; collect those first so we can
    // cascade-drop them.
    let droppable: Vec<usize> = table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, col)| !pk_cols.contains(&col.name))
        .map(|(idx, _)| idx)
        .collect();
    if droppable.is_empty() {
        return;
    }
    let pick = droppable[seed % droppable.len()];
    let dropped_name = table.columns[pick].name.clone();
    table.columns.remove(pick);

    // Cascade-drop non-PK constraints that reference the dropped column.
    // PrimaryKey is excluded (the column was checked non-PK above).
    // Unique, ForeignKey, and Check constraints that name the column are dropped.
    table
        .constraints
        .retain(|con| !constraint_references_column(&con.kind, &dropped_name));

    // Cascade-clean column-level grants: strip the dropped column from each
    // grant's column list; remove grants whose column list becomes empty
    // (an empty column-restriction list is invalid — the grant would target no
    // columns).
    for grant in &mut table.grants {
        if let Some(cols) = &mut grant.columns {
            cols.retain(|c| c != &dropped_name);
        }
    }
    table
        .grants
        .retain(|g| g.columns.as_ref().is_none_or(|cols| !cols.is_empty()));

    let qname = table.qname.clone();
    // Cascade-drop indexes that reference the dropped column.
    c.indexes.retain(|idx| {
        if idx.on.qname() != &qname {
            return true;
        }
        !idx.columns
            .iter()
            .any(|ic| matches!(&ic.expr, IndexColumnExpr::Column(name) if name == &dropped_name))
    });
    // Cascade-clean statistics that reference the dropped column:
    //   1. Remove the dropped column from each statistic's column list.
    //   2. Drop statistics whose column list falls below 2 (PG requires >= 2
    //      for CREATE STATISTICS).
    for stat in &mut c.statistics {
        if stat.target == qname {
            stat.columns.retain(|sc| {
                !matches!(sc, pgevolve_core::ir::statistic::StatisticColumn::Column(n) if n == &dropped_name)
            });
        }
    }
    c.statistics.retain(|stat| {
        if stat.target != qname {
            return true;
        }
        stat.columns.len() >= 2
    });
}

/// Returns `true` if `kind` explicitly names `col` in its column lists.
/// Used by `drop_non_pk_column` to cascade-drop dependent constraints.
fn constraint_references_column(
    kind: &pgevolve_core::ir::constraint::ConstraintKind,
    col: &pgevolve_core::identifier::Identifier,
) -> bool {
    use pgevolve_core::ir::constraint::ConstraintKind;
    match kind {
        ConstraintKind::PrimaryKey { columns, include } => {
            columns.contains(col) || include.contains(col)
        }
        ConstraintKind::Unique {
            columns, include, ..
        } => columns.contains(col) || include.contains(col),
        ConstraintKind::ForeignKey(fk) => fk.columns.contains(col),
        // CHECK expressions are stored as normalized text; we cannot cheaply
        // parse them for column references. In practice the generator only
        // produces `true` as the CHECK body (see `policy.rs`), so there are
        // no column references to clean up here.
        ConstraintKind::Check { .. } => false,
    }
}

fn toggle_nullable(c: &mut Catalog, seed: usize) {
    if c.tables.is_empty() {
        return;
    }
    let i = seed % c.tables.len();
    let table = &mut c.tables[i];
    let pk_cols = pk_columns(table);
    let candidates: Vec<usize> = table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, col)| !pk_cols.contains(&col.name))
        .map(|(idx, _)| idx)
        .collect();
    if candidates.is_empty() {
        return;
    }
    let pick = candidates[seed % candidates.len()];
    table.columns[pick].nullable = !table.columns[pick].nullable;
}

fn add_index(c: &mut Catalog, seed: usize) {
    if c.tables.is_empty() {
        return;
    }
    let i = seed % c.tables.len();
    let table = &c.tables[i];
    // Restrict to columns indexable with the default btree opclass — the
    // mutator emits btree indexes (see `method:` below). `json` lacks a
    // default btree opclass and would produce PG error 42704 at apply time;
    // see `crate::ir_generator::is_btree_indexable`. When no candidates
    // remain, the mutation is a no-op (consistent with other mutators that
    // bail when nothing applies).
    let candidates: Vec<&Column> = table
        .columns
        .iter()
        .filter(|col| is_btree_indexable(&col.ty))
        .collect();
    if candidates.is_empty() {
        return;
    }
    let col_pick = seed % candidates.len();
    let col_name = candidates[col_pick].name.clone();
    let idx_name = unique_index_name(&c.indexes, &table.qname, seed);
    let qname = QualifiedName::new(table.qname.schema.clone(), idx_name);
    c.indexes.push(Index {
        qname,
        on: IndexParent::Table(table.qname.clone()),
        method: IndexMethod::BTree,
        columns: vec![IndexColumn {
            expr: IndexColumnExpr::Column(col_name),
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
        storage: pgevolve_core::ir::reloptions::IndexStorageOptions::default(),
    });
}

fn drop_non_pk_index(c: &mut Catalog, seed: usize) {
    if c.indexes.is_empty() {
        return;
    }
    let pick = seed % c.indexes.len();
    c.indexes.remove(pick);
}

fn add_table(c: &mut Catalog, seed: usize) {
    if c.schemas.is_empty() {
        return;
    }
    let schema = c.schemas[seed % c.schemas.len()].name.clone();
    let name = unique_table_name(&c.tables, &schema, seed);
    let qname = QualifiedName::new(schema.clone(), name.clone());
    let id_col = Column {
        name: Identifier::from_unquoted("id").unwrap(),
        ty: pgevolve_core::ir::column_type::ColumnType::BigInt,
        nullable: false,
        default: None,
        identity: None,
        generated: None,
        collation: None,
        storage: None,
        compression: None,
        comment: None,
    };
    let pk = pgevolve_core::ir::constraint::Constraint {
        qname: QualifiedName::new(
            schema,
            Identifier::from_unquoted(&format!("{name}_pkey")).unwrap(),
        ),
        kind: pgevolve_core::ir::constraint::ConstraintKind::PrimaryKey {
            columns: vec![Identifier::from_unquoted("id").unwrap()],
            include: vec![],
        },
        deferrable: pgevolve_core::ir::constraint::Deferrable::NotDeferrable,
        comment: None,
    };
    c.tables.push(pgevolve_core::ir::table::Table {
        qname,
        columns: vec![id_col],
        constraints: vec![pk],
        partition_by: None,
        partition_of: None,
        comment: None,
        owner: None,
        grants: vec![],
        rls_enabled: false,
        rls_forced: false,
        policies: vec![],
        storage: pgevolve_core::ir::reloptions::TableStorageOptions::default(),
    });
}

fn drop_table(c: &mut Catalog, seed: usize) {
    if c.tables.is_empty() {
        return;
    }
    let pick = seed % c.tables.len();
    let qname = c.tables[pick].qname.clone();
    c.tables.remove(pick);
    cascade_drop_objects_for_table(c, &qname);
}

fn add_schema(c: &mut Catalog, seed: usize) {
    // Try a few candidate names before giving up.
    for n in 0..4usize {
        let name_str = format!("gen_schema_{}", seed.wrapping_add(n) % 1024);
        let Ok(name) = Identifier::from_unquoted(&name_str) else {
            continue;
        };
        if c.schemas.iter().all(|s| s.name != name) {
            c.schemas.push(Schema::new(name));
            return;
        }
    }
}

fn drop_schema(c: &mut Catalog, seed: usize) {
    if c.schemas.is_empty() {
        return;
    }
    let pick = seed % c.schemas.len();
    let dropped = c.schemas[pick].name.clone();
    c.schemas.remove(pick);
    let owned_table_qnames: Vec<QualifiedName> = c
        .tables
        .iter()
        .filter(|t| t.qname.schema == dropped)
        .map(|t| t.qname.clone())
        .collect();
    c.tables.retain(|t| t.qname.schema != dropped);
    c.indexes
        .retain(|i| i.qname.schema != dropped && !owned_table_qnames.contains(i.on.qname()));
    c.sequences.retain(|s| {
        s.qname.schema != dropped
            && !s
                .owned_by
                .as_ref()
                .is_some_and(|o| owned_table_qnames.contains(&o.table))
    });
    // Cascade-drop schema-owned objects not handled by the original code.
    // Without these, objects with qname.schema == dropped would remain in
    // the catalog referencing a schema no longer in `c.schemas`, causing PG
    // error 3F000 (schema does not exist) when the DDL is applied.
    c.types.retain(|ty| ty.qname.schema != dropped);
    c.collations.retain(|col| col.qname.schema != dropped);
    c.statistics.retain(|s| s.qname.schema != dropped);
    c.views.retain(|v| v.qname.schema != dropped);
    c.materialized_views.retain(|m| m.qname.schema != dropped);
    // Strip any default-privilege rules scoped to the dropped schema.
    c.default_privileges
        .retain(|r| r.schema.as_ref() != Some(&dropped));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pk_columns(t: &pgevolve_core::ir::table::Table) -> Vec<Identifier> {
    t.constraints
        .iter()
        .find_map(|c| match &c.kind {
            pgevolve_core::ir::constraint::ConstraintKind::PrimaryKey { columns, .. } => {
                Some(columns.clone())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn unique_column_name(columns: &[Column], seed: usize) -> Identifier {
    for n in 0..16usize {
        let candidate = format!("c{}", seed.wrapping_add(n) % 4096);
        if let Ok(id) = Identifier::from_unquoted(&candidate)
            && !columns.iter().any(|c| c.name == id)
        {
            return id;
        }
    }
    // Fall back to a deterministic guaranteed-fresh name.
    Identifier::from_unquoted(&format!("c{}", columns.len() + 10_000)).unwrap()
}

fn unique_index_name(indexes: &[Index], table: &QualifiedName, seed: usize) -> Identifier {
    for n in 0..16usize {
        let candidate = format!("{}_g{}_idx", table.name, seed.wrapping_add(n) % 4096);
        if let Ok(id) = Identifier::from_unquoted(&candidate)
            && !indexes
                .iter()
                .any(|i| i.qname.name == id && i.qname.schema == table.schema)
        {
            return id;
        }
    }
    Identifier::from_unquoted(&format!("{}_idx{}", table.name, indexes.len() + 10_000)).unwrap()
}

fn unique_table_name(
    tables: &[pgevolve_core::ir::table::Table],
    schema: &Identifier,
    seed: usize,
) -> Identifier {
    for n in 0..16usize {
        let candidate = format!("gen_t_{}", seed.wrapping_add(n) % 4096);
        if let Ok(id) = Identifier::from_unquoted(&candidate)
            && !tables
                .iter()
                .any(|t| t.qname.schema == *schema && t.qname.name == id)
        {
            return id;
        }
    }
    Identifier::from_unquoted(&format!("gen_t_{}", tables.len() + 10_000)).unwrap()
}

fn cascade_drop_objects_for_table(c: &mut Catalog, qname: &QualifiedName) {
    c.indexes.retain(|i| i.on.qname() != qname);
    c.sequences
        .retain(|s: &Sequence| s.owned_by.as_ref().is_none_or(|o| &o.table != qname));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir_generator::{IRGeneratorConfig, arbitrary_catalog};
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    #[test]
    fn mutator_produces_valid_catalogs() {
        let mut runner = TestRunner::default();
        for _ in 0..50 {
            let seed_tree = arbitrary_catalog(IRGeneratorConfig::default())
                .new_tree(&mut runner)
                .unwrap();
            let seed = seed_tree.current();
            let mutated_tree = arbitrary_mutation(seed.clone())
                .new_tree(&mut runner)
                .unwrap();
            let mutated = mutated_tree.current();
            // Re-canonicalize as a defensive check (already done inside apply_mutation).
            mutated
                .canonicalize()
                .expect("mutated catalog must canonicalize");
        }
    }

    #[test]
    fn mutator_actually_diverges_from_seed_often_enough() {
        // Out of 100 mutations, at least 50 should produce a catalog that
        // differs from the seed. Some mutation kinds no-op on certain seeds
        // (e.g., drop_table when tables are empty), so the threshold isn't
        // 100% — but the generator's typical catalogs have enough objects
        // that most mutations land.
        let mut runner = TestRunner::default();
        let mut diverged = 0;
        for _ in 0..100 {
            let seed_tree = arbitrary_catalog(IRGeneratorConfig::default())
                .new_tree(&mut runner)
                .unwrap();
            let seed = seed_tree.current();
            let mutated_tree = arbitrary_mutation(seed.clone())
                .new_tree(&mut runner)
                .unwrap();
            let mutated = mutated_tree.current();
            if mutated != seed {
                diverged += 1;
            }
        }
        assert!(
            diverged >= 50,
            "only {diverged} / 100 mutations diverged from seed",
        );
    }

    /// Regression for issue #19: `drop_column` mutation did not cascade-drop
    /// constraints, grants, and statistics that reference the dropped column,
    /// leaving dangling column-name references in the mutated catalog. PG
    /// then rejects the DDL with `column "X" of relation "Y" does not exist`
    /// across every PG major and every property in the soak matrix.
    ///
    /// This test generates 256 mutated catalogs from random seeds and asserts
    /// that every column name referenced anywhere in a mutated catalog
    /// (constraints, grants, statistics) actually exists in that table's
    /// column list.
    #[test]
    fn drop_column_cleans_all_dependent_references() {
        use std::collections::BTreeSet;

        let mut runner = TestRunner::default();
        for _ in 0..256 {
            let seed_tree = arbitrary_catalog(IRGeneratorConfig::default())
                .new_tree(&mut runner)
                .unwrap();
            let seed = seed_tree.current();
            let mutated_tree = arbitrary_mutation(seed.clone())
                .new_tree(&mut runner)
                .unwrap();
            let mutated = mutated_tree.current();

            for table in &mutated.tables {
                let col_names: BTreeSet<&Identifier> =
                    table.columns.iter().map(|c| &c.name).collect();

                // Constraints must only reference declared columns.
                for con in &table.constraints {
                    use pgevolve_core::ir::constraint::ConstraintKind;
                    match &con.kind {
                        ConstraintKind::PrimaryKey { columns, include } => {
                            for col in columns.iter().chain(include.iter()) {
                                assert!(
                                    col_names.contains(col),
                                    "table '{}' constraint '{}' references undeclared column '{}'",
                                    table.qname.render_sql(),
                                    con.qname.render_sql(),
                                    col.as_str(),
                                );
                            }
                        }
                        ConstraintKind::Unique {
                            columns, include, ..
                        } => {
                            for col in columns.iter().chain(include.iter()) {
                                assert!(
                                    col_names.contains(col),
                                    "table '{}' UNIQUE constraint '{}' references undeclared column '{}'",
                                    table.qname.render_sql(),
                                    con.qname.render_sql(),
                                    col.as_str(),
                                );
                            }
                        }
                        ConstraintKind::ForeignKey(fk) => {
                            for col in &fk.columns {
                                assert!(
                                    col_names.contains(col),
                                    "table '{}' FK constraint '{}' references undeclared column '{}'",
                                    table.qname.render_sql(),
                                    con.qname.render_sql(),
                                    col.as_str(),
                                );
                            }
                        }
                        ConstraintKind::Check { .. } => {}
                    }
                }

                // Column-level grants must only reference declared columns.
                for grant in &table.grants {
                    if let Some(cols) = &grant.columns {
                        assert!(
                            !cols.is_empty(),
                            "table '{}' has a grant with an empty column list (invalid)",
                            table.qname.render_sql(),
                        );
                        for col in cols {
                            assert!(
                                col_names.contains(col),
                                "table '{}' grant references undeclared column '{}'",
                                table.qname.render_sql(),
                                col.as_str(),
                            );
                        }
                    }
                }
            }

            // Statistics must only reference columns that exist in their target table.
            for stat in &mutated.statistics {
                // Find the target table.
                let Some(target_table) = mutated.tables.iter().find(|t| t.qname == stat.target)
                else {
                    continue; // table was dropped — covered by the schema cascade test
                };
                let col_names: BTreeSet<&Identifier> =
                    target_table.columns.iter().map(|c| &c.name).collect();
                let mut col_count = 0usize;
                for sc in &stat.columns {
                    if let pgevolve_core::ir::statistic::StatisticColumn::Column(col) = sc {
                        assert!(
                            col_names.contains(col),
                            "statistic '{}' references undeclared column '{}' on table '{}'",
                            stat.qname.render_sql(),
                            col.as_str(),
                            stat.target.render_sql(),
                        );
                        col_count += 1;
                    }
                }
                // PG requires at least 2 column entries for CREATE STATISTICS.
                assert!(
                    col_count >= 2,
                    "statistic '{}' has fewer than 2 column references (col_count={col_count})",
                    stat.qname.render_sql(),
                );
            }
        }
    }

    /// Regression for issue #13: `drop_schema` did not cascade-drop
    /// `types`, `collations`, and `statistics` that belong to the dropped
    /// schema, leaving orphaned schema-qualified references whose schema no
    /// longer appeared in `catalog.schemas`. PG then rejects the DDL with
    /// error 3F000 (schema does not exist) at apply time.
    ///
    /// Also covers the companion bug in `arbitrary_default_privileges` where
    /// the rule could carry `IN SCHEMA "billing"` even when `billing` was
    /// never declared — that fix is in `ir_generator/grants.rs` and validated
    /// by the generator test.
    ///
    /// This test generates 256 mutated catalogs from random seeds and asserts
    /// that every [`QualifiedName`] referenced in the mutated catalog has a
    /// schema that is declared in `mutated.schemas`.
    #[test]
    fn all_qnames_reference_declared_schemas() {
        use std::collections::BTreeSet;

        let mut runner = TestRunner::default();
        for _ in 0..256 {
            let seed_tree = arbitrary_catalog(IRGeneratorConfig::default())
                .new_tree(&mut runner)
                .unwrap();
            let seed = seed_tree.current();
            let mutated_tree = arbitrary_mutation(seed.clone())
                .new_tree(&mut runner)
                .unwrap();
            let mutated = mutated_tree.current();

            let declared: BTreeSet<&Identifier> = mutated.schemas.iter().map(|s| &s.name).collect();

            // Collect every schema-qualified name used across all catalog lists.
            let mut referenced: Vec<(&str, &Identifier)> = Vec::new();
            for t in &mutated.tables {
                referenced.push(("table", &t.qname.schema));
            }
            for i in &mutated.indexes {
                referenced.push(("index", &i.qname.schema));
            }
            for s in &mutated.sequences {
                referenced.push(("sequence", &s.qname.schema));
            }
            for v in &mutated.views {
                referenced.push(("view", &v.qname.schema));
            }
            for m in &mutated.materialized_views {
                referenced.push(("materialized_view", &m.qname.schema));
            }
            for ty in &mutated.types {
                referenced.push(("type", &ty.qname.schema));
            }
            for c in &mutated.collations {
                referenced.push(("collation", &c.qname.schema));
            }
            for s in &mutated.statistics {
                referenced.push(("statistic", &s.qname.schema));
            }

            for (kind, schema) in &referenced {
                assert!(
                    declared.contains(schema),
                    "{kind} references undeclared schema '{}'  \
                    (declared schemas: {declared:?})",
                    schema.as_str(),
                );
            }
        }
    }

    /// Regression for the property-test failure where `add_index` produced
    /// a btree index on a `json` column. PG rejects this with error 42704
    /// ("data type json has no default operator class for access method
    /// 'btree'"). Confirms the mutator now filters columns by
    /// `is_btree_indexable` before picking.
    #[test]
    fn add_index_skips_non_btree_indexable_columns() {
        use pgevolve_core::ir::column::Column;
        use pgevolve_core::ir::column_type::ColumnType;
        use pgevolve_core::ir::constraint::{Constraint, ConstraintKind};
        use pgevolve_core::ir::table::Table;

        let schema_name = Identifier::from_unquoted("app").unwrap();
        let table_qname = QualifiedName::new(
            schema_name.clone(),
            Identifier::from_unquoted("docs").unwrap(),
        );
        let pk_col_name = Identifier::from_unquoted("id").unwrap();
        let json_col_name = Identifier::from_unquoted("payload").unwrap();
        let pk_constraint_name = Identifier::from_unquoted("docs_pkey").unwrap();

        // Table with one btree-able PK column and one `json` non-PK column.
        // Pre-fix, the mutator would pick the json column with some seeds
        // and produce a btree index that PG rejects at apply time.
        let table = Table {
            qname: table_qname,
            columns: vec![
                Column {
                    name: pk_col_name.clone(),
                    ty: ColumnType::BigInt,
                    nullable: false,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    storage: None,
                    compression: None,
                    comment: None,
                },
                Column {
                    name: json_col_name.clone(),
                    ty: ColumnType::Json,
                    nullable: true,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    storage: None,
                    compression: None,
                    comment: None,
                },
            ],
            constraints: vec![Constraint {
                qname: QualifiedName::new(schema_name.clone(), pk_constraint_name),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![pk_col_name],
                    include: vec![],
                },
                deferrable: pgevolve_core::ir::constraint::Deferrable::NotDeferrable,
                comment: None,
            }],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: pgevolve_core::ir::reloptions::TableStorageOptions::default(),
        };
        let mut catalog = Catalog {
            schemas: vec![Schema {
                name: schema_name,
                comment: None,
                owner: None,
                grants: vec![],
            }],
            extensions: vec![],
            tables: vec![table],
            indexes: vec![],
            sequences: vec![],
            views: vec![],
            materialized_views: vec![],
            types: vec![],
            functions: vec![],
            procedures: vec![],
            triggers: vec![],
            publications: vec![],
            statistics: vec![],
            subscriptions: vec![],
            collations: vec![],
            default_privileges: vec![],
        };

        // Try every seed in 0..16 (covering all column-pick positions and
        // both modular outcomes for two-column tables). Pre-fix, seeds
        // landing on the json column produced an invalid index. Post-fix,
        // every index produced uses the PK column instead, and any seed
        // that would have landed on json is a no-op.
        for seed in 0..16usize {
            let mut c = catalog.clone();
            super::add_index(&mut c, seed);
            // No index should target the json column. The PK column is the
            // only btree-indexable candidate, so any added index uses it.
            for idx in &c.indexes {
                for ic in &idx.columns {
                    if let IndexColumnExpr::Column(name) = &ic.expr {
                        assert_ne!(
                            name, &json_col_name,
                            "seed {seed}: add_index produced a btree index on a json column",
                        );
                    }
                }
            }
            catalog = c; // accumulate added indexes across seeds — they all
            // must target the bigint PK column, never json.
        }
    }
}
