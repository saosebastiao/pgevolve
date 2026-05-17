//! L6 — no collateral damage.
//!
//! Asserts every step's primary target is in `expect.plan.touches_only`.
//! Catches the class of bug where changing one object recreates
//! unrelated objects.

use std::collections::BTreeSet;

use anyhow::Result;
use pgevolve_core::plan::plan::Plan;

/// Assert that every step touches only targets declared in `allowed`.
///
/// The layer is opt-in: when `allowed` is empty the function returns `Ok(())`
/// immediately so existing fixtures that do not declare `touches_only` are
/// unaffected.
///
/// For each step, *all* entries in `step.targets` are checked. A violation is
/// recorded when any target is absent from the allow-list.
pub fn assert_touches_only(plan: &Plan, allowed: &[String]) -> Result<()> {
    if allowed.is_empty() {
        return Ok(()); // layer skipped when no allow-list declared
    }
    let allowed: BTreeSet<_> = allowed.iter().cloned().collect();
    let mut violations = Vec::new();
    for group in &plan.groups {
        for step in &group.steps {
            for target in &step.targets {
                let target_str = target.to_string();
                if !allowed.contains(&target_str) {
                    violations.push(format!("step {} → {}", step.step_no, target_str));
                }
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "L6 no-collateral-damage: {} target(s) outside the allowed set [{}]:\n  {}",
            violations.len(),
            allowed.iter().cloned().collect::<Vec<_>>().join(", "),
            violations.join("\n  "),
        )
    }
}
