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
    let err = pgevolve_core::plan::order(&empty, &source, changes)
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected order-stage failure, but order succeeded"))?;
    let msg = err.to_string();
    // Body-derived cycles produce PlanError::UnbreakableCycle.
    if !msg.contains("UnbreakableCycle") && !msg.contains("body-derived") {
        anyhow::bail!("expected cycle error, got: {msg}");
    }
    assert_substrings(&msg, &exp.stderr_contains)
}

fn run_lint_at_plan_stage(_fixture: &Fixture, _exp: &ExpectFailure) -> Result<()> {
    // The lint-at-plan failure path requires running the binary because
    // gating happens in the `pgevolve plan` command, not in core. This
    // is deferred — for v0.2-readiness T4, lint-at-plan failure fixtures
    // are not yet wired. See failure/lint-at-plan/README.md.
    anyhow::bail!(
        "lint-at-plan failure stage is not wired in T4 — requires CLI orchestration; \
         consider running pgevolve plan as a subprocess and checking exit/stderr"
    )
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

fn assert_substrings(msg: &str, needles: &[String]) -> Result<()> {
    for n in needles {
        if !msg.contains(n.as_str()) {
            anyhow::bail!("expected substring {n:?} in error message:\n{msg}");
        }
    }
    Ok(())
}
