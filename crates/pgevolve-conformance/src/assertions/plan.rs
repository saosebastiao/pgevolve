//! Layer 2: plan structural invariants — step count and rewrite kinds
//! used. Layer 3 (golden plan.sql compare) lives alongside in this
//! file and is added in a follow-up task.

use pgevolve_core::plan::{Plan, Strategy};

use crate::fixture::Fixture;
use crate::planning::{PipelineError, render_plan};

/// Result of running Layer 2.
#[derive(Debug)]
pub struct PlanInvariantOutcome {
    /// The plan that was rendered.
    pub plan: Plan,
    /// Rendered plan.sql, available for Layer 3 to use.
    pub rendered_sql: String,
    /// Actual step count.
    pub actual_steps: usize,
    /// `Some(expected)` when `[expect.plan].steps` was set and didn't match.
    pub step_mismatch: Option<usize>,
    /// Rewrites named in fixture.expect that did not appear in the plan.
    pub missing_rewrites: Vec<String>,
}

impl PlanInvariantOutcome {
    /// True when every assertion held.
    pub const fn is_ok(&self) -> bool {
        self.step_mismatch.is_none() && self.missing_rewrites.is_empty()
    }
}

/// Run Layer 2.
pub fn check(fixture: &Fixture) -> Result<PlanInvariantOutcome, PipelineError> {
    let strategy = parse_strategy(fixture).unwrap_or(Strategy::Online);
    let (plan, rendered_sql) = render_plan(&fixture.before_sql, &fixture.after_sql, strategy)?;

    let actual_steps: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
    let step_mismatch = match fixture.expect.plan.steps {
        Some(expected) if expected != actual_steps => Some(expected),
        _ => None,
    };

    // Find rewrites used. The current approach matches by SQL substring
    // (cheap, has no dependency on internal RawStep fields). If this
    // becomes brittle, promote to a structured match against
    // RawStep.rewrite_kind.
    let missing_rewrites = fixture
        .expect
        .plan
        .rewrites_used
        .iter()
        .filter(|kind| !plan_uses(&rendered_sql, kind))
        .cloned()
        .collect();

    Ok(PlanInvariantOutcome {
        plan,
        rendered_sql,
        actual_steps,
        step_mismatch,
        missing_rewrites,
    })
}

fn plan_uses(plan_sql: &str, rewrite_kind: &str) -> bool {
    match rewrite_kind {
        "create_index_concurrent" => {
            plan_sql.contains("CREATE INDEX CONCURRENTLY")
                || plan_sql.contains("CREATE UNIQUE INDEX CONCURRENTLY")
        }
        "fk_not_valid_then_validate" | "check_not_valid_then_validate" => {
            plan_sql.contains("NOT VALID") && plan_sql.contains("VALIDATE CONSTRAINT")
        }
        "not_null_via_check_pattern" => {
            plan_sql.contains("CHECK") && plan_sql.contains("NOT VALID")
        }
        // Unknown kinds match nothing; the assertion fails clearly.
        _ => false,
    }
}

fn parse_strategy(fixture: &Fixture) -> Option<Strategy> {
    let raw = fixture.passthrough.planner.get("strategy")?.as_str()?;
    match raw {
        "atomic" => Some(Strategy::Atomic),
        "online" => Some(Strategy::Online),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{
        ExpectApply, ExpectDiff, ExpectPlan, FixtureExpect, FixtureMeta, FixturePassthrough,
        FixturePg,
    };
    use std::path::PathBuf;

    fn fixture(before: &str, after: &str, expect_plan: ExpectPlan) -> Fixture {
        Fixture {
            dir: PathBuf::from("/dev/null"),
            before_sql: before.to_string(),
            after_sql: after.to_string(),
            meta: FixtureMeta {
                title: "test".into(),
                spec_refs: vec![],
                issue: None,
            },
            pg: FixturePg::default(),
            passthrough: FixturePassthrough::default(),
            expect: FixtureExpect {
                diff: ExpectDiff::default(),
                plan: expect_plan,
                apply: ExpectApply::default(),
            },
        }
    }

    #[test]
    fn step_count_mismatch_is_reported() {
        let before =
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n";
        let after =
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n";
        let f = fixture(
            before,
            after,
            ExpectPlan {
                steps: Some(999),
                rewrites_used: vec![],
                golden: None,
            },
        );
        let out = check(&f).unwrap();
        assert!(!out.is_ok());
        assert_eq!(out.step_mismatch, Some(999));
    }

    #[test]
    fn unrecognized_rewrite_id_is_reported_missing() {
        let before = "-- @pgevolve schema=app\nCREATE SCHEMA app;\n";
        let after = "-- @pgevolve schema=app\nCREATE SCHEMA app;\n";
        let f = fixture(
            before,
            after,
            ExpectPlan {
                steps: None,
                rewrites_used: vec!["totally_made_up_rewrite".into()],
                golden: None,
            },
        );
        let out = check(&f).unwrap();
        assert!(!out.is_ok());
        assert_eq!(out.missing_rewrites, vec!["totally_made_up_rewrite"]);
    }
}
