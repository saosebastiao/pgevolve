//! L9 — topological-order assertion.
//!
//! Asserts declared partial orders in `expect.plan.order` are respected
//! by the emitted step sequence.

use anyhow::Result;
use pgevolve_core::plan::plan::Plan;

/// Assert that every "A < B" entry in `declared` is satisfied by the plan.
///
/// For each entry both A and B must be targets of steps in the plan; if either
/// is absent the partial order is vacuously satisfied. When both are present
/// the step containing A must come before the step containing B.
pub fn assert_order(plan: &Plan, declared: &[String]) -> Result<()> {
    if declared.is_empty() {
        return Ok(());
    }

    // Flat ordered list of all target strings, preserving step order.
    let step_targets: Vec<String> = plan
        .groups
        .iter()
        .flat_map(|g| g.steps.iter().flat_map(|s| s.targets.iter().map(std::string::ToString::to_string)))
        .collect();

    let position = |target: &str| step_targets.iter().position(|t| t == target);

    for decl in declared {
        let (a, b) = decl
            .split_once('<')
            .ok_or_else(|| anyhow::anyhow!("bad order entry: {decl} (expected 'A < B')"))?;
        let (a, b) = (a.trim(), b.trim());
        if let (Some(ai), Some(bi)) = (position(a), position(b))
            && ai >= bi
        {
            anyhow::bail!(
                "L9 topological-order: {a} must precede {b}, but {a} at step {} and {b} at step {}",
                ai + 1,
                bi + 1,
            );
        }
        // If either target is absent from the plan, the partial order is
        // vacuously satisfied; do not error.
    }
    Ok(())
}
