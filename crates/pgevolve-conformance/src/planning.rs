//! Pure in-process diff+plan pipeline.
//!
//! Parses `before.sql` and `after.sql` into IRs and runs them through
//! `diff` → `order` → `rewrite` → `group_steps` → `Plan::from_grouped`
//! with fixed (test) values for everything that would otherwise be
//! nondeterministic (target identity, git rev). The `created=`
//! timestamp on the rendered plan.sql is normalized at compare time
//! (see `crate::normalize`).

use pgevolve_core::catalog::DriftReport;
use pgevolve_core::diff::{ChangeSet, diff};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::lint::Finding;
use pgevolve_core::lint::universal::{check_changeset, check_plan_time_catalog};
use pgevolve_core::parse::{ParseError, parse_directory};
use pgevolve_core::plan::{
    Plan, PlanError, PlanIoError, PlannerPolicy, Strategy, group_steps, order, rewrite_with_source,
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
    // Source-vs-source diff: no live-catalog drift, pass empty report.
    let changes = diff(&target, &source, &DriftReport::default());
    Ok((target, source, changes))
}

/// Run the full pipeline: diff → lint → rewrite → plan → SQL.
///
/// Returns the resulting `Plan`, the rendered `plan.sql` for downstream
/// assertions (step count, rewrites, golden compare), and any advisory
/// findings from `check_changeset` and `check_plan_time_catalog`.
pub fn render_plan(
    before_sql: &str,
    after_sql: &str,
    strategy: Strategy,
) -> Result<(Plan, String, Vec<Finding>), PipelineError> {
    let (target, source, changes) = compute_changes(before_sql, after_sql)?;
    let mut advisory_findings = check_changeset(&changes);
    advisory_findings.extend(check_plan_time_catalog(&source));
    let policy = PlannerPolicy {
        strategy,
        ..PlannerPolicy::default()
    };
    let ordered = order(&target, &source, changes, &policy)?;
    let steps = rewrite_with_source(ordered, &target, &source, &policy);
    let groups = group_steps(steps);
    let plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        TEST_TARGET_IDENTITY.to_string(),
        None,
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    )?;
    let mut buf = Vec::new();
    write_plan_sql(&plan, &mut buf)?;
    let sql = String::from_utf8(buf).expect("plan.sql is utf-8");
    Ok((plan, sql, advisory_findings))
}

/// Output of the cluster pipeline for conformance assertions.
pub struct ClusterPipelineOutput {
    /// Desired cluster state (parsed from `after_sql`).
    pub source: pgevolve_core::ir::cluster::ClusterCatalog,
    /// Current cluster state (parsed from `before_sql`).
    pub target: pgevolve_core::ir::cluster::ClusterCatalog,
    /// Diff between target and source.
    pub changes: pgevolve_core::diff::cluster::ClusterChangeSet,
    /// Emitted DDL steps.
    pub steps: Vec<pgevolve_core::plan::RawStep>,
    /// Advisory lint findings from `check_cluster_changeset`.
    pub advisory_findings: Vec<pgevolve_core::lint::Finding>,
}

/// Build a cluster plan from `before.sql` / `after.sql` fixture files.
///
/// `before_sql` is parsed as the *target* (current live cluster state);
/// `after_sql` is parsed as the *source* (desired state). `diff(target,
/// source)` produces the change list. Lint runs on `(source, changes)`.
pub fn render_cluster_plan(
    before_sql: &str,
    after_sql: &str,
) -> Result<ClusterPipelineOutput, PipelineError> {
    let target = parse_one_cluster_source(before_sql)?;
    let source = parse_one_cluster_source(after_sql)?;
    let changes = pgevolve_core::diff::cluster::diff_cluster(&target, &source);
    let advisory_findings =
        pgevolve_core::lint::universal::check_cluster_changeset(&source, &changes);
    let steps = pgevolve_core::plan::cluster_rewrite::emit_cluster_changes(&changes);
    Ok(ClusterPipelineOutput {
        source,
        target,
        changes,
        steps,
        advisory_findings,
    })
}

fn parse_one_cluster_source(
    sql: &str,
) -> Result<pgevolve_core::ir::cluster::ClusterCatalog, PipelineError> {
    let td = tempfile::tempdir()?;
    let roles_dir = td.path().join("roles");
    std::fs::create_dir(&roles_dir)?;
    std::fs::write(roles_dir.join("a.sql"), sql)?;
    pgevolve_core::parse::cluster::parse_cluster_directory(&roles_dir).map_err(|source| {
        PipelineError::Parse {
            label: "cluster",
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_diff_produces_empty_plan() {
        let sql = "-- @pgevolve schema=app\nCREATE SCHEMA app;\n";
        let (plan, rendered, advisory) = render_plan(sql, sql, Strategy::Online).unwrap();
        let step_count: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
        assert_eq!(step_count, 0, "no diff → no steps");
        assert!(rendered.len() < 4096, "header-only plan should be short");
        assert!(advisory.is_empty(), "no-op → no advisory findings");
    }

    #[test]
    fn add_column_produces_at_least_one_step() {
        let before = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n";
        let after = "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n";
        let (plan, rendered, _advisory) = render_plan(before, after, Strategy::Online).unwrap();
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
