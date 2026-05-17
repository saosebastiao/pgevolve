//! L5 — minimality.
//!
//! After L4's apply, re-read the catalog into IR and re-plan against
//! the after.sql source IR. Assert the resulting diff is empty.

use anyhow::Result;
use pgevolve_core::catalog::DriftReport;
use pgevolve_core::ir::catalog::Catalog;

/// Input for the L5 minimality assertion.
pub struct MinimalityInput<'a> {
    /// Introspected catalog immediately after L4's apply succeeded.
    pub post_apply_catalog: &'a Catalog,
    /// Drift report from the same post-apply introspection.
    pub post_apply_drift: &'a DriftReport,
    /// Parsed source IR (from `after.sql` or `post_apply_equals_to`).
    pub after_source: &'a Catalog,
}

/// Assert that re-planning against the post-apply DB produces an empty change set.
///
/// A non-empty change set means the planner would emit follow-up steps on every
/// run, which violates the minimality property.
pub fn assert_minimal(input: &MinimalityInput<'_>) -> Result<()> {
    let changes = pgevolve_core::diff::diff(
        input.post_apply_catalog,
        input.after_source,
        input.post_apply_drift,
    );
    if changes.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "L5 minimality: re-plan against post-apply state produced a non-empty change set:\n{}",
            render_changeset(&changes),
        )
    }
}

fn render_changeset(changes: &pgevolve_core::diff::ChangeSet) -> String {
    format!("{changes:#?}")
}
