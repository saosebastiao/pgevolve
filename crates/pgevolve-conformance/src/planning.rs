//! Pure in-process diff+plan pipeline.
//!
//! Parses `before.sql` and `after.sql` into IRs and runs them through
//! `diff` → `order` → `rewrite` → `group_steps` → `Plan::from_grouped`
//! with fixed (test) values for everything that would otherwise be
//! nondeterministic (target identity, git rev). The `created=`
//! timestamp on the rendered plan.sql is normalized at compare time
//! (see `crate::normalize`).

use pgevolve_core::diff::{ChangeSet, diff};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::{ParseError, parse_directory};
use pgevolve_core::plan::{
    Plan, PlanError, PlanIoError, PlannerPolicy, Strategy, group_steps, order, rewrite,
    write_plan_sql,
};

/// Fixed target identity used by the conformance pipeline. Plan IDs
/// depend on this; using a fixed value keeps plan IDs reproducible
/// across runs.
pub const TEST_TARGET_IDENTITY: &str = "conformance-test-target";

/// Errors produced by the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Parse error in before.sql or after.sql.
    #[error("parse error in {label}: {source}")]
    Parse {
        /// "before" or "after"
        label: &'static str,
        /// underlying parser error
        source: ParseError,
    },
    /// Planner error.
    #[error("plan error: {0}")]
    Plan(#[from] PlanError),
    /// Plan I/O error (serializing plan.sql).
    #[error("plan io error: {0}")]
    PlanIo(#[from] PlanIoError),
    /// Tempdir / IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse `sql` into a `Catalog` via `parse_directory`, using a tempdir
/// so the parser owns the only filesystem walk (matching binary
/// semantics).
pub fn parse_sql(sql: &str, label: &'static str) -> Result<Catalog, PipelineError> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("fixture.sql");
    std::fs::write(&path, sql)?;
    parse_directory(tmp.path(), &[]).map_err(|source| PipelineError::Parse { label, source })
}

/// Compute the diff between `before.sql` and `after.sql`.
pub fn compute_changes(
    before_sql: &str,
    after_sql: &str,
) -> Result<(Catalog, Catalog, ChangeSet), PipelineError> {
    let target = parse_sql(before_sql, "before")?;
    let source = parse_sql(after_sql, "after")?;
    let changes = diff(&target, &source);
    Ok((target, source, changes))
}

/// Run the full pipeline. Returns the resulting `Plan` plus the
/// rendered `plan.sql` for downstream assertions (step count, rewrites,
/// golden compare).
pub fn render_plan(
    before_sql: &str,
    after_sql: &str,
    strategy: Strategy,
) -> Result<(Plan, String), PipelineError> {
    let (target, source, changes) = compute_changes(before_sql, after_sql)?;
    let ordered = order(&target, &source, changes)?;
    let policy = PlannerPolicy {
        strategy,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(ordered, &target, &policy);
    let groups = group_steps(steps);
    let plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        TEST_TARGET_IDENTITY.to_string(),
        None,
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    );
    let mut buf = Vec::new();
    write_plan_sql(&plan, &mut buf)?;
    let sql = String::from_utf8(buf).expect("plan.sql is utf-8");
    Ok((plan, sql))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_diff_produces_empty_plan() {
        let sql = "-- @pgevolve schema=app\nCREATE SCHEMA app;\n";
        let (plan, rendered) = render_plan(sql, sql, Strategy::Online).unwrap();
        let step_count: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
        assert_eq!(step_count, 0, "no diff → no steps");
        assert!(rendered.len() < 4096, "header-only plan should be short");
    }

    #[test]
    fn add_column_produces_at_least_one_step() {
        let before = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n";
        let after = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n";
        let (plan, rendered) = render_plan(before, after, Strategy::Online).unwrap();
        let step_count: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
        assert!(
            step_count >= 1,
            "add column → at least one step, got {step_count}"
        );
        assert!(
            rendered.contains("ADD COLUMN"),
            "plan.sql contains ADD COLUMN; got:\n{rendered}"
        );
    }
}
