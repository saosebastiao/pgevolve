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
//! 2. [`sentinel_view_columns`] — view/MV column types collapse to the
//!    `view_column` sentinel.
//! 3. [`renumber_enum_sort_orders`] — every enum's `sort_order` values
//!    are re-indexed to `1.0, 2.0, 3.0, …` in current order.
//! 4. [`sort_and_dedupe`] — every collection is sorted by its canonical
//!    key and duplicates raise [`IrError`]. Runs last so duplicate
//!    detection sees post-normalization values.
//!
//! See `docs/superpowers/specs/2026-05-19-canon-consolidation-design.md`.

pub mod sentinel_view_columns;
pub mod sort_and_dedupe;

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

/// Run every canonicalization pass on `cat` in order.
///
/// Only [`sort_and_dedupe`] is fallible; the other passes mutate in
/// place and cannot fail.
pub fn canonicalize(cat: &mut Catalog) -> Result<(), IrError> {
    sentinel_view_columns::run(cat);
    sort_and_dedupe::run(cat)?;
    Ok(())
}
