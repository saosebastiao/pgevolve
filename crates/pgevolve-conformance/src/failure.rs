//! Failure-fixture runner.
//!
//! Invokes the appropriate pipeline stage for the fixture's declared
//! `[expect.failure].stage`, then asserts the failure shape (stage name,
//! error-message substrings).
//!
//! Supported stages:
//! - `"parse"` — runs `parse_directory`; expects a `ParseError`.
//! - `"ast_resolution"` — same binary as `parse`, but the error must be the
//!   `AstResolution` variant (asserted via the `"AST resolution failed"` prefix
//!   that the `ParseError` `Display` implementation renders).
//! - `"order"` — runs `parse` + `diff` + `order`; expects a `PlanError`.
//! - `"lint_at_plan"` — deferred; requires CLI orchestration (see below).

use anyhow::Result;

use crate::fixture::{ExpectFailure, Fixture};

/// Run the failure contract declared by `fixture`.
///
/// Returns `Ok(())` when the expected failure was observed with all declared
/// `stderr_contains` substrings present.  Returns `Err` when the pipeline
/// *succeeded* unexpectedly, when the wrong stage failed, or when a required
/// substring was absent.
pub fn run_failure_fixture(fixture: &Fixture) -> Result<()> {
    let exp = fixture
        .expect
        .failure
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("failure fixture missing [expect.failure]"))?;
    match exp.stage.as_str() {
        "parse" => run_parse_stage(fixture, exp),
        "ast_resolution" => run_ast_resolution_stage(fixture, exp),
        "order" => run_order_stage(fixture, exp),
        "lint_at_plan" => run_lint_at_plan_stage(fixture, exp),
        other => anyhow::bail!("unknown failure stage: {other}"),
    }
}

fn run_parse_stage(fixture: &Fixture, exp: &ExpectFailure) -> Result<()> {
    let tmp = stage_source(fixture)?;
    let err = pgevolve_core::parse::parse_directory(tmp.path(), &[])
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected parse-stage failure, but parse succeeded"))?;
    let msg = err.to_string();
    assert_substrings(&msg, &exp.stderr_contains)
}

fn run_ast_resolution_stage(fixture: &Fixture, exp: &ExpectFailure) -> Result<()> {
    let tmp = stage_source(fixture)?;
    let err = pgevolve_core::parse::parse_directory(tmp.path(), &[])
        .err()
        .ok_or_else(|| {
            anyhow::anyhow!("expected ast_resolution-stage failure, but parse succeeded")
        })?;
    let msg = err.to_string();
    // The AstResolution variant renders as "AST resolution failed:\n  - ..."
    if !msg.contains("AST resolution failed") {
        anyhow::bail!("expected AST resolution error, got: {msg}");
    }
    assert_substrings(&msg, &exp.stderr_contains)
}

fn run_order_stage(fixture: &Fixture, exp: &ExpectFailure) -> Result<()> {
    use pgevolve_core::catalog::DriftReport;
    use pgevolve_core::diff::diff;

    let tmp = stage_source(fixture)?;
    let source = pgevolve_core::parse::parse_directory(tmp.path(), &[])?;
    let empty = pgevolve_core::ir::catalog::Catalog::default();
    let changes = diff(&empty, &source, &DriftReport::default());
    let policy = pgevolve_core::plan::PlannerPolicy::default();
    let err = pgevolve_core::plan::order(&empty, &source, changes, &policy)
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected order-stage failure, but order succeeded"))?;
    let msg = err.to_string();
    // Body-derived cycles produce PlanError::UnbreakableCycle.
    if !msg.contains("UnbreakableCycle") && !msg.contains("body-derived") {
        anyhow::bail!("expected cycle error, got: {msg}");
    }
    assert_substrings(&msg, &exp.stderr_contains)
}

fn run_lint_at_plan_stage(fixture: &Fixture, exp: &ExpectFailure) -> Result<()> {
    // In-process implementation: stage source (after.sql) and target
    // (before.sql), parse both, run run_drift_lints, and assert that at
    // least one LintAtPlan finding fires. The fixture's `stderr_contains`
    // substrings are matched against finding messages (for this stage the
    // field name is reused to avoid a fixture-schema change — callers
    // document that for `lint_at_plan` the substrings match finding messages,
    // not CLI stderr).
    use pgevolve_core::lint::Severity;
    use pgevolve_core::lint::universal::run_drift_lints;

    // Stage source (after.sql).
    let source_tmp = stage_source(fixture)?;
    let source = pgevolve_core::parse::parse_directory(source_tmp.path(), &[])
        .map_err(|e| anyhow::anyhow!("parse source (after.sql): {e}"))?;

    // Stage target (before.sql). An empty before.sql means an empty catalog.
    let target = {
        let target_tmp = stage_target(fixture)?;
        let text = std::fs::read_to_string(target_tmp.path().join("schema/app/0001.sql"))
            .unwrap_or_default();
        if text.trim().is_empty() {
            pgevolve_core::ir::catalog::Catalog::empty()
        } else {
            pgevolve_core::parse::parse_directory(target_tmp.path(), &[])
                .map_err(|e| anyhow::anyhow!("parse target (before.sql): {e}"))?
        }
    };

    let findings = run_drift_lints(&source, &target);
    let lint_at_plan: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::LintAtPlan))
        .collect();

    if lint_at_plan.is_empty() {
        anyhow::bail!(
            "expected lint-at-plan-stage failure (at least one LintAtPlan finding), \
             but run_drift_lints produced none",
        );
    }

    // Build a combined string that includes each finding's rule ID and message
    // so that fixture `stderr_contains` can match against either:
    //   "[column-position-drift] app.users: column position drift. …"
    let combined = lint_at_plan
        .iter()
        .map(|f| format!("[{}] {}", f.rule, f.message))
        .collect::<Vec<_>>()
        .join("\n");
    assert_substrings(&combined, &exp.stderr_contains)
}

/// Stage the fixture's `after.sql` into a temp directory that `parse_directory`
/// can walk.
fn stage_source(fixture: &Fixture) -> Result<tempfile::TempDir> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("schema").join("app");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("0001.sql"), &fixture.after_sql)?;
    Ok(tmp)
}

/// Stage the fixture's `before.sql` into a temp directory that `parse_directory`
/// can walk.
fn stage_target(fixture: &Fixture) -> Result<tempfile::TempDir> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("schema").join("app");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("0001.sql"), &fixture.before_sql)?;
    Ok(tmp)
}

fn assert_substrings(msg: &str, needles: &[String]) -> Result<()> {
    for n in needles {
        if !msg.contains(n.as_str()) {
            anyhow::bail!("expected substring {n:?} in error message:\n{msg}");
        }
    }
    Ok(())
}
