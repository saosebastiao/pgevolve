//! L5 — minimality.
//!
//! After L4's apply, re-read the catalog into IR and re-plan against
//! the after.sql source IR. Assert both:
//!
//! 1. `diff.is_empty()` — the IR-level change set is empty.
//! 2. `plan.groups.is_empty()` — the planner pipeline produces no steps.
//!
//! Spec §5.1 step 4 requires both checks. v0.2 may produce groups from
//! drift entries that the diff check alone would not catch.

use anyhow::Result;
use pgevolve_core::catalog::DriftReport;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::plan::{PlannerPolicy, group_steps, order, rewrite_with_source};

/// Input for the L5 minimality assertion.
pub struct MinimalityInput<'a> {
    /// Introspected catalog immediately after L4's apply succeeded.
    pub post_apply_catalog: &'a Catalog,
    /// Drift report from the same post-apply introspection.
    pub post_apply_drift: &'a DriftReport,
    /// Parsed source IR (from `after.sql` or `post_apply_equals_to`).
    pub after_source: &'a Catalog,
}

/// Assert that re-planning against the post-apply DB produces an empty change set
/// **and** that the planner pipeline emits no steps.
///
/// A non-empty result in either check means the planner would emit follow-up
/// steps on every run, which violates the minimality property.
pub fn assert_minimal(input: &MinimalityInput<'_>) -> Result<()> {
    // 1. Diff check.
    let changes = pgevolve_core::diff::diff(
        input.post_apply_catalog,
        input.after_source,
        input.post_apply_drift,
    );
    if !changes.is_empty() {
        anyhow::bail!(
            "L5 minimality: re-plan against post-apply state produced a non-empty change set:\n{}",
            render_changeset(&changes),
        );
    }

    // 2. Planner pipeline check.
    // `changes` is empty here; we pass it through the full pipeline to ensure
    // the planner also produces no groups (v0.2 may add drift-driven steps).
    let ordered = order(input.post_apply_catalog, input.after_source, changes)
        .map_err(|e| anyhow::anyhow!("L5 minimality: planner ordering failed: {e}"))?;

    let steps = rewrite_with_source(
        ordered,
        input.post_apply_catalog,
        input.after_source,
        &PlannerPolicy::default(),
    );

    let groups = group_steps(steps);
    if !groups.is_empty() {
        let names: Vec<String> = groups
            .iter()
            .flat_map(|g| g.steps.iter().map(|s| format!("{:?}", s.kind)))
            .collect();
        anyhow::bail!(
            "L5 minimality: planner produced {} unexpected group(s) with steps: [{}]",
            groups.len(),
            names.join(", "),
        );
    }

    Ok(())
}

fn render_changeset(changes: &pgevolve_core::diff::ChangeSet) -> String {
    format!("{changes:#?}")
}
