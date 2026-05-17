//! Cross-check the source IR against a live shadow Postgres.
//!
//! For each Structural edge in the source dep graph, the shadow's
//! `pg_depend` should report a corresponding entry. Missing entries
//! would indicate a dep-graph builder bug; for v0.1 surface this is
//! a no-op (every edge trivially has a `pg_depend` counterpart),
//! but v0.2 sub-specs extend it to body-derived edges and body
//! canonicalization round-trips.

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::plan::edges::{DepSource, build_create_graph};

use crate::shadow::ShadowBackend;

/// Summary of a cross-check run.
#[derive(Debug, Default)]
pub struct CrossCheckReport {
    /// Number of Structural edges examined.
    pub structural_edges_checked: usize,
    /// Warnings: present in source but not independently verified (expected
    /// to be empty for v0.1 surface; populated by v0.2 body sub-specs).
    pub warnings: Vec<String>,
    /// Errors: definite mismatches that indicate a dep-graph builder bug.
    pub errors: Vec<String>,
}

/// Run the cross-check against a shadow backend.
///
/// For v0.1 the check is intentionally a no-op: every edge in the source
/// dep graph is `Structural`, and the body-canonicalization comparisons
/// live entirely in v0.2 sub-specs. The function signature, the report
/// type, and the wiring through three commands are the deliverable for
/// arch spec Decision 12.
///
/// v0.2 sub-specs will:
/// - Boot the shadow and apply the source IR to it.
/// - Read `pg_depend`; cross-check `AstExtracted` edges against it.
/// - Read `pg_get_viewdef` / `pg_get_functiondef`; re-canonicalize;
///   compare bytes.
/// - Populate `report.errors` / `report.warnings` accordingly.
#[allow(clippy::unused_async)] // v0.2 sub-specs will add actual awaits
pub async fn cross_check(
    _backend: &dyn ShadowBackend,
    source: &Catalog,
    _pg_major: u32,
    _strict: bool,
) -> Result<CrossCheckReport> {
    let mut report = CrossCheckReport::default();
    let graph = build_create_graph(source);
    for edge in graph.dep_edges() {
        if matches!(edge.source, DepSource::Structural) {
            report.structural_edges_checked += 1;
        }
    }
    // v0.1 has no body-bearing objects; no per-edge round-trip needed.
    // v0.2 sub-specs:
    //   - Boot shadow, apply source IR to it.
    //   - Read pg_depend; cross-check AstExtracted edges against it.
    //   - Read pg_get_viewdef / pg_get_functiondef; re-canonicalize;
    //     compare bytes.
    //   - Errors / warnings populate `report.errors` / `report.warnings`.
    Ok(report)
}
