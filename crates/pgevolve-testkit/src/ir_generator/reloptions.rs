//! Reloptions generators (v0.3.3) and column-level storage/compression.
//!
//! Range-bounded strategies prevent generating PG-invalid combinations.

#![allow(clippy::needless_pass_by_value)]

use proptest::prelude::*;

use pgevolve_core::ir::canon::filter_pg_defaults::type_default_storage;
use pgevolve_core::ir::column::{Compression, StorageKind};
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::index::IndexMethod;
use pgevolve_core::ir::reloptions::{
    AutovacuumOptions, IndexStorageOptions, NotNanF64, TableStorageOptions,
};

/// Generate a random `STORAGE` strategy that is type-aware.
///
/// Toastable types (those whose PG default is not `PLAIN`) may be assigned
/// any of the four storage variants. Fixed-width types (default `PLAIN`) only
/// yield `None` or `Some(PLAIN)` — the others are illegal for those types.
pub(super) fn arb_storage(ty: &ColumnType) -> BoxedStrategy<Option<StorageKind>> {
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
pub(super) fn arb_compression(ty: &ColumnType) -> BoxedStrategy<Option<Compression>> {
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
pub(super) fn arb_table_storage() -> impl Strategy<Value = TableStorageOptions> {
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

/// Generate random [`IndexStorageOptions`] with 0–N fields set, gated on
/// the access method.
///
/// Each option is only generated as non-`None` for the access method(s) that
/// Postgres actually recognises it on (PG 14-17):
///
/// | Option                   | Valid for                     |
/// |--------------------------|-------------------------------|
/// | `fillfactor`             | B-tree, hash, `GiST`, `SP-GiST` |
/// | `fastupdate`             | GIN only                         |
/// | `gin_pending_list_limit` | GIN only                         |
/// | `buffering`              | `GiST` only                      |
/// | `deduplicate_items`      | B-tree only                   |
/// | `pages_per_range`        | BRIN only                     |
/// | `autosummarize`          | BRIN only                     |
///
/// For the incompatible combinations the strategy always returns `Just(None)`
/// so the generated IR can round-trip through a live Postgres without the
/// server silently dropping unrecognised options.
pub(super) fn arb_index_storage(method: IndexMethod) -> BoxedStrategy<IndexStorageOptions> {
    // fillfactor: B-tree range 50–100; hash/GiST/SP-GiST share the same
    // valid range so we use 50–100 uniformly for all AM-compatible methods.
    let fillfactor: BoxedStrategy<Option<u32>> = match method {
        IndexMethod::BTree | IndexMethod::Hash | IndexMethod::Gist | IndexMethod::Spgist => {
            prop_oneof![Just(None), (50u32..=100).prop_map(Some)].boxed()
        }
        _ => Just(None).boxed(),
    };

    // fastupdate: GIN only.
    let fastupdate: BoxedStrategy<Option<bool>> = match method {
        IndexMethod::Gin => prop_oneof![Just(None), Just(Some(true)), Just(Some(false))].boxed(),
        _ => Just(None).boxed(),
    };

    // gin_pending_list_limit: GIN only (bytes; 64 kB–2 GB typical, use small range for tests).
    let gin_pending_list_limit: BoxedStrategy<Option<u64>> = match method {
        IndexMethod::Gin => prop_oneof![Just(None), (65536u64..=1_048_576).prop_map(Some)].boxed(),
        _ => Just(None).boxed(),
    };

    // buffering: GiST only.
    let buffering: BoxedStrategy<Option<pgevolve_core::ir::reloptions::BufferingMode>> =
        match method {
            IndexMethod::Gist => prop_oneof![
                Just(None),
                Just(Some(pgevolve_core::ir::reloptions::BufferingMode::On)),
                Just(Some(pgevolve_core::ir::reloptions::BufferingMode::Off)),
                Just(Some(pgevolve_core::ir::reloptions::BufferingMode::Auto)),
            ]
            .boxed(),
            _ => Just(None).boxed(),
        };

    // deduplicate_items: B-tree only (PG 13+).
    let deduplicate_items: BoxedStrategy<Option<bool>> = match method {
        IndexMethod::BTree => prop_oneof![Just(None), Just(Some(true)), Just(Some(false))].boxed(),
        _ => Just(None).boxed(),
    };

    // pages_per_range: BRIN only.
    let pages_per_range: BoxedStrategy<Option<u32>> = match method {
        IndexMethod::Brin => prop_oneof![Just(None), (1u32..=128).prop_map(Some)].boxed(),
        _ => Just(None).boxed(),
    };

    // autosummarize: BRIN only.
    let autosummarize: BoxedStrategy<Option<bool>> = match method {
        IndexMethod::Brin => prop_oneof![Just(None), Just(Some(true)), Just(Some(false))].boxed(),
        _ => Just(None).boxed(),
    };

    (
        fillfactor,
        fastupdate,
        gin_pending_list_limit,
        buffering,
        deduplicate_items,
        pages_per_range,
        autosummarize,
    )
        .prop_map(
            |(
                fillfactor,
                fastupdate,
                gin_pending_list_limit,
                buffering,
                deduplicate_items,
                pages_per_range,
                autosummarize,
            )| IndexStorageOptions {
                fillfactor,
                fastupdate,
                gin_pending_list_limit,
                buffering,
                deduplicate_items,
                pages_per_range,
                autosummarize,
                ..Default::default()
            },
        )
        .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::test_runner::{Config, TestRunner};

    use super::*;

    /// B-tree indexes must never carry GIN-only or BRIN-only storage options.
    ///
    /// `arb_index_storage(BTree)` must always produce `None` for:
    /// `fastupdate`, `gin_pending_list_limit`, `buffering`,
    /// `pages_per_range`, `autosummarize`.
    ///
    /// `fillfactor` and `deduplicate_items` are B-tree-compatible and may be
    /// non-`None`.
    #[test]
    fn btree_storage_only_has_btree_compatible_options() {
        let mut runner = TestRunner::new(Config::with_cases(512));
        runner
            .run(&arb_index_storage(IndexMethod::BTree), |opts| {
                prop_assert_eq!(
                    opts.fastupdate,
                    None,
                    "fastupdate is GIN-only; must be None for B-tree"
                );
                prop_assert_eq!(
                    opts.gin_pending_list_limit,
                    None,
                    "gin_pending_list_limit is GIN-only; must be None for B-tree"
                );
                prop_assert_eq!(
                    opts.buffering,
                    None,
                    "buffering is GiST-only; must be None for B-tree"
                );
                prop_assert_eq!(
                    opts.pages_per_range,
                    None,
                    "pages_per_range is BRIN-only; must be None for B-tree"
                );
                prop_assert_eq!(
                    opts.autosummarize,
                    None,
                    "autosummarize is BRIN-only; must be None for B-tree"
                );
                Ok(())
            })
            .unwrap();
    }
}
