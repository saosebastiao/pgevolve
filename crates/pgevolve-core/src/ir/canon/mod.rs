//! `Catalog` canonicalization pipeline.
//!
//! Every IR-value normalization rule that must apply to both the
//! source-built `Catalog` and the catalog-reader-built `Catalog` lives
//! here, behind a single entry point. The pipeline runs in a fixed
//! documented order; new rules go into the appropriate file in this
//! module (or get a new file if they're a new kind of rule).
//!
//! Today's order:
//!
//! 1. [`filter_pg_defaults`] — values that equal PG's documented
//!    defaults become `None` (sequence min/max, function cost/rows,
//!    column collation `pg_catalog.default`).
//! 2. [`resolve_user_defined_types`] — promote
//!    `ColumnType::Other { raw: "schema.name" }` to
//!    `ColumnType::UserDefined(qname)` when the qname matches a managed
//!    user-defined type. Symmetrises catalog reads with source parses,
//!    which already produce `UserDefined` directly.
//! 3. [`sentinel_view_columns`] — view/MV column types collapse to the
//!    `view_column` sentinel.
//! 4. [`renumber_enum_sort_orders`] — every enum's `sort_order` values
//!    are re-indexed to `1.0, 2.0, 3.0, …` in current order.
//! 5. [`reloptions`] — canonicalize reloption fields (currently a no-op;
//!    `extra` is `BTreeMap` so keys are already ordered).
//! 6. [`sort_and_dedupe`] — every collection is sorted by its canonical
//!    key and duplicates raise [`IrError`]. Runs last so duplicate
//!    detection sees post-normalization values.
//!
//! See `docs/superpowers/specs/2026-05-19-canon-consolidation-design.md`.

pub mod aggregates;
pub mod cluster;
pub mod collations;
pub mod default_privileges;
pub mod event_triggers;
pub mod filter_pg_defaults;
pub mod grants;
pub mod policies;
pub mod publications;
pub mod reloptions;
pub mod renumber_enum_sort_orders;
pub mod resolve_user_defined_types;
pub mod sentinel_view_columns;
pub mod sort_and_dedupe;
pub mod statistics;
pub mod subscriptions;

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

/// Run every canonicalization pass on `cat` in order.
///
/// [`publications`] and [`sort_and_dedupe`] are fallible; the other
/// passes mutate in place and cannot fail.
pub fn canonicalize(cat: &mut Catalog) -> Result<(), IrError> {
    filter_pg_defaults::run(cat);
    resolve_user_defined_types::run(cat);
    sentinel_view_columns::run(cat);
    renumber_enum_sort_orders::run(cat);
    // For every object that carries an `owner` field: strip grants where
    // `grantee == owner` **before** deduplication. This mirrors what the live
    // catalog reader does via `catalog::grants::strip_owner_self_grants`, so
    // that source IR and live IR are normalised identically. Without this pass
    // the IR generator can produce `owner = role_X` together with an explicit
    // `role_X / privilege` grant; the source catalog would retain the grant
    // while the live reader silently discards it, causing `diff(live, source)`
    // to be non-empty and `assert_convergent` to fail (issue #36).
    for s in &mut cat.schemas {
        grants::strip_owner_self_grants(&mut s.grants, s.owner.as_ref());
        grants::run_on_list(&mut s.grants);
    }
    for s in &mut cat.sequences {
        grants::strip_owner_self_grants(&mut s.grants, s.owner.as_ref());
        grants::run_on_list(&mut s.grants);
    }
    for t in &mut cat.tables {
        grants::strip_owner_self_grants(&mut t.grants, t.owner.as_ref());
        grants::run_on_list(&mut t.grants);
    }
    for v in &mut cat.views {
        grants::strip_owner_self_grants(&mut v.grants, v.owner.as_ref());
        grants::run_on_list(&mut v.grants);
    }
    for m in &mut cat.materialized_views {
        grants::strip_owner_self_grants(&mut m.grants, m.owner.as_ref());
        grants::run_on_list(&mut m.grants);
    }
    for f in &mut cat.functions {
        grants::strip_owner_self_grants(&mut f.grants, f.owner.as_ref());
        grants::run_on_list(&mut f.grants);
    }
    for p in &mut cat.procedures {
        grants::strip_owner_self_grants(&mut p.grants, p.owner.as_ref());
        grants::run_on_list(&mut p.grants);
    }
    for t in &mut cat.types {
        grants::strip_owner_self_grants(&mut t.grants, t.owner.as_ref());
        grants::run_on_list(&mut t.grants);
    }
    default_privileges::run(&mut cat.default_privileges);
    for t in &mut cat.tables {
        policies::run_on_table(t);
    }
    publications::run(cat)?;
    event_triggers::run(cat)?;
    aggregates::run(cat)?;
    subscriptions::run(cat)?;
    statistics::run(cat)?;
    collations::run(cat)?;
    reloptions::run(cat);
    sort_and_dedupe::run(cat)?;
    Ok(())
}
