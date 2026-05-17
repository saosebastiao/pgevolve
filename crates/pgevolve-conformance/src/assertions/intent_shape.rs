//! L7 — intent shape.
//!
//! Asserts every `[[expect.intent]]` row in fixture.toml matches a
//! generated intent.toml row. Mandatory on destructive fixtures:
//!   - destructive change without `[[expect.intent]]` → fail
//!   - non-destructive change with `[[expect.intent]]` → fail

use anyhow::Result;
use pgevolve_core::plan::plan::Plan;

use crate::fixture::ExpectIntentRow;

/// Run L7: intent shape assertion.
///
/// Called after a plan is built (post-L2) for `Objects`, `Scenarios`,
/// `Intent`, and `Regressions` fixtures.
pub fn assert_intent_shape(plan: &Plan, expected: &[ExpectIntentRow]) -> Result<()> {
    let generated = &plan.intents;
    let is_destructive = !generated.is_empty();

    if is_destructive && expected.is_empty() {
        anyhow::bail!(
            "L7: destructive fixture (plan has {} intent row(s)) must declare at least one [[expect.intent]]",
            generated.len(),
        );
    }
    if !is_destructive && !expected.is_empty() {
        anyhow::bail!(
            "L7: non-destructive fixture declared {} [[expect.intent]] row(s) but plan has no destructive steps",
            expected.len(),
        );
    }
    if expected.len() != generated.len() {
        anyhow::bail!(
            "L7 intent count mismatch: expected {} but planner generated {}",
            expected.len(),
            generated.len(),
        );
    }
    for (i, exp) in expected.iter().enumerate() {
        let matched = generated.iter().find(|g| {
            // kind is already a String (snake_case), so lowercase-contains is safe.
            g.kind.to_lowercase().contains(&exp.kind.to_lowercase())
                && g.target == exp.target
        });
        let matched = matched.ok_or_else(|| {
            anyhow::anyhow!(
                "L7: no generated intent matches expected #{i}: kind={} target={}\nGenerated intents: {:#?}",
                exp.kind,
                exp.target,
                generated
                    .iter()
                    .map(|g| format!("kind={} target={}", g.kind, g.target))
                    .collect::<Vec<_>>(),
            )
        })?;
        for needle in &exp.reason_contains {
            if !matched.reason.contains(needle.as_str()) {
                anyhow::bail!(
                    "L7: intent #{i} reason {:?} missing substring {:?}",
                    matched.reason,
                    needle,
                );
            }
        }
    }
    Ok(())
}
