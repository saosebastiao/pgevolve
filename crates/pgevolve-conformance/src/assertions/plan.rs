//! Layer 2: plan structural invariants — step count and rewrite kinds
//! used. Layer 3 (golden plan.sql compare) lives alongside in this
//! file and is added in a follow-up task.

use pgevolve_core::lint::Finding;
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
    /// Advisory findings from `check_changeset`, for Layer advisory assertions.
    pub advisory_findings: Vec<Finding>,
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
    let (plan, rendered_sql, advisory_findings) =
        render_plan(&fixture.before_sql, &fixture.after_sql, strategy)?;

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
        advisory_findings,
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

/// Layer 3 result.
#[derive(Debug)]
pub struct GoldenOutcome {
    /// Path of the golden compared against. `None` when goldening is
    /// opted out for this fixture.
    pub golden_path: Option<std::path::PathBuf>,
    /// Normalized plan.sql produced from the in-process pipeline.
    pub actual_normalized: String,
    /// Normalized contents of the golden file. `None` when goldening
    /// is opted out or the golden file is missing.
    pub expected_normalized: Option<String>,
    /// Set when actual ≠ expected, or when the golden file is missing.
    pub mismatch: Option<String>,
}

impl GoldenOutcome {
    /// True if the golden compare passed or was opted out.
    pub const fn is_ok(&self) -> bool {
        self.mismatch.is_none()
    }
}

/// Compare the rendered plan.sql against the fixture's golden file.
/// Uses `pg_major` to select `per-pg/pg<N>/plan.sql` when present.
pub fn check_golden(
    fixture: &crate::fixture::Fixture,
    rendered_sql: &str,
    pg_major: u32,
) -> std::io::Result<GoldenOutcome> {
    let golden_path = fixture.golden_path(pg_major);
    let actual_normalized = crate::normalize::normalize(rendered_sql);

    let Some(path) = golden_path else {
        return Ok(GoldenOutcome {
            golden_path: None,
            actual_normalized,
            expected_normalized: None,
            mismatch: None,
        });
    };

    if !path.exists() {
        return Ok(GoldenOutcome {
            golden_path: Some(path.clone()),
            actual_normalized,
            expected_normalized: None,
            mismatch: Some(format!(
                "golden file {} does not exist; run `cargo xtask bless --conformance` to create it",
                path.display()
            )),
        });
    }

    let raw = std::fs::read_to_string(&path)?;
    let expected_normalized = crate::normalize::normalize(&raw);
    let mismatch = if actual_normalized == expected_normalized {
        None
    } else {
        Some(format!(
            "plan.sql mismatch vs {}. Run `cargo xtask bless --conformance` if intentional.",
            path.display()
        ))
    };

    Ok(GoldenOutcome {
        golden_path: Some(path),
        actual_normalized,
        expected_normalized: Some(expected_normalized),
        mismatch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{
        ExpectAdvisory, ExpectApply, ExpectDepGraph, ExpectDiff, ExpectPlan, FixtureBudget,
        FixtureExpect, FixtureMeta, FixturePassthrough, FixturePg, FixtureSettings,
    };
    use std::path::PathBuf;

    fn fixture(before: &str, after: &str, expect_plan: ExpectPlan) -> Fixture {
        Fixture {
            dir: PathBuf::from("/dev/null"),
            before_sql: before.to_string(),
            after_sql: after.to_string(),
            setup_sql: None,
            meta: FixtureMeta {
                title: "test".into(),
                spec_refs: vec![],
                issue: None,
                authoring: "objects".into(),
            },
            pg: FixturePg::default(),
            budget: FixtureBudget::default(),
            fixture: FixtureSettings::default(),
            passthrough: FixturePassthrough::default(),
            expect: FixtureExpect {
                diff: ExpectDiff::default(),
                plan: expect_plan,
                apply: ExpectApply::default(),
                dep_graph: ExpectDepGraph::default(),
                intent: Vec::new(),
                failure: None,
                advisory: ExpectAdvisory::default(),
            },
        }
    }

    #[test]
    fn step_count_mismatch_is_reported() {
        let before = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n";
        let after = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n";
        let f = fixture(
            before,
            after,
            ExpectPlan {
                steps: Some(999),
                rewrites_used: vec![],
                golden: None,
                minimality: true,
                touches_only: vec![],
                order: vec![],
                per_pg: std::collections::HashMap::new(),
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
                minimality: true,
                touches_only: vec![],
                order: vec![],
                per_pg: std::collections::HashMap::new(),
            },
        );
        let out = check(&f).unwrap();
        assert!(!out.is_ok());
        assert_eq!(out.missing_rewrites, vec!["totally_made_up_rewrite"]);
    }
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use crate::fixture::{
        ExpectAdvisory, ExpectApply, ExpectDepGraph, ExpectDiff, ExpectPlan, FixtureBudget,
        FixtureExpect, FixtureMeta, FixturePassthrough, FixturePg, FixtureSettings,
    };
    use std::path::PathBuf;

    fn fixture_with_golden_in(dir: PathBuf, golden_body: &str) -> crate::fixture::Fixture {
        std::fs::create_dir_all(dir.join("expected")).unwrap();
        std::fs::write(dir.join("expected/plan.sql"), golden_body).unwrap();
        crate::fixture::Fixture {
            dir,
            before_sql: String::new(),
            after_sql: String::new(),
            setup_sql: None,
            meta: FixtureMeta {
                title: "g".into(),
                spec_refs: vec![],
                issue: None,
                authoring: "objects".into(),
            },
            pg: FixturePg::default(),
            budget: FixtureBudget::default(),
            fixture: FixtureSettings::default(),
            passthrough: FixturePassthrough::default(),
            expect: FixtureExpect {
                diff: ExpectDiff::default(),
                plan: ExpectPlan {
                    steps: None,
                    rewrites_used: vec![],
                    golden: Some("expected/plan.sql".into()),
                    minimality: true,
                    touches_only: vec![],
                    order: vec![],
                    per_pg: std::collections::HashMap::new(),
                },
                apply: ExpectApply::default(),
                dep_graph: ExpectDepGraph::default(),
                intent: Vec::new(),
                failure: None,
                advisory: ExpectAdvisory::default(),
            },
        }
    }

    #[test]
    fn passes_when_golden_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        let out = check_golden(&f, "SELECT 1;\n", 17).unwrap();
        assert!(out.is_ok(), "{:?}", out.mismatch);
    }

    #[test]
    fn fails_when_golden_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        let out = check_golden(&f, "SELECT 2;\n", 17).unwrap();
        assert!(!out.is_ok());
        assert!(out.mismatch.unwrap().contains("bless"));
    }

    #[test]
    fn missing_golden_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        std::fs::remove_file(tmp.path().join("expected/plan.sql")).unwrap();
        let out = check_golden(&f, "SELECT 1;\n", 17).unwrap();
        assert!(!out.is_ok());
    }

    #[test]
    fn opt_out_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fixture_with_golden_in(tmp.path().to_path_buf(), "SELECT 1;\n");
        f.expect.plan.golden = None;
        let out = check_golden(&f, "anything", 17).unwrap();
        assert!(out.is_ok());
        assert!(out.golden_path.is_none());
    }
}
