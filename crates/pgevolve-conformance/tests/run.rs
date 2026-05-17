//! Conformance suite entry point.
//!
//! Walks `tests/cases/**/fixture.toml` via `walk::discover`, routes each
//! fixture by its `authoring` key, and aggregates failures. Each fixture
//! failure is reported with its directory path so failures are immediately
//! actionable.
//!
//! Driven by env var `PGEVOLVE_TEST_PG_VERSION` (default: 17) so the
//! same suite runs once per major in the CI matrix.
//!
//! Per-version overrides in `fixture.toml`:
//! - `[pg.expect]."<major>" = "skip"` — skip this fixture on that major.
//! - `[pg.expect]."<major>" = "failure"` — expect the fixture to fail on that major.
//! - `[expect.plan.per_pg].pg<major>` — override plan structural expectations.

use std::path::{Path, PathBuf};

use pgevolve_conformance::assertions::{apply, dep_graph, diff, intent_shape, minimality, plan, topological_order, touches_only};
use pgevolve_conformance::fixture::{ExpectPlan, Fixture};
use pgevolve_conformance::planning::parse_sql;
use pgevolve_conformance::walk::{self, Authoring};

fn cases_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cases")
}

fn active_pg_major() -> u32 {
    std::env::var("PGEVOLVE_TEST_PG_VERSION")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(17)
}

/// The outcome of consulting `[pg.expect]` for a given major.
#[derive(Debug, PartialEq)]
enum PgExpect {
    /// Run the fixture normally.
    Success,
    /// The fixture is expected to fail on this major (currently treated as skip).
    ExpectFailure,
    /// Skip the fixture entirely on this major.
    Skip,
}

/// Consult `[pg.expect]` for the given major.
fn pg_expect_for_major(fixture: &Fixture, major: u32) -> PgExpect {
    match fixture.pg.expect.0.get(&major.to_string()).map(String::as_str) {
        Some("skip") => PgExpect::Skip,
        Some("failure") => PgExpect::ExpectFailure,
        _ => PgExpect::Success,
    }
}

/// Resolve the effective `ExpectPlan` for the given major, applying any
/// `[expect.plan.per_pg].pg<major>` overrides.
fn resolve_plan_for_major(fixture: &Fixture, major: u32) -> ExpectPlan {
    let mut plan = fixture.expect.plan.clone();
    let key = format!("pg{major}");
    if let Some(ov) = fixture.expect.plan.per_pg.get(&key) {
        if let Some(s) = ov.steps {
            plan.steps = Some(s);
        }
        if !ov.rewrites_used.is_empty() {
            plan.rewrites_used.clone_from(&ov.rewrites_used);
        }
        if !ov.touches_only.is_empty() {
            plan.touches_only.clone_from(&ov.touches_only);
        }
        if !ov.order.is_empty() {
            plan.order.clone_from(&ov.order);
        }
    }
    plan
}

#[derive(Debug, Default)]
struct Report {
    failures: Vec<String>,
}

impl Report {
    fn fail(&mut self, fixture: &std::path::Path, layer: &str, detail: impl AsRef<str>) {
        self.failures.push(format!(
            "[{}] {}: {}",
            layer,
            fixture
                .strip_prefix(cases_root())
                .unwrap_or(fixture)
                .display(),
            detail.as_ref()
        ));
    }
}

/// Append one timing row to `target/conformance-timings.tsv`.
/// Best-effort: never panics; errors are silently dropped.
fn append_timing(path: &Path, dir: &Path, elapsed: std::time::Duration) {
    use std::io::Write;
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{}\t{:.6}", dir.display(), elapsed.as_secs_f64())
    })();
    // Ignore write errors — timing is best-effort.
    let _ = result;
}

/// The result of running one fixture: either skipped or ran (with collected layer failures).
enum FixtureResult {
    /// Fixture skipped (version-gated or pg.expect = skip).
    Skipped,
    /// Fixture ran; `failures` contains `(layer, detail)` pairs.
    Ran { failures: Vec<(String, String)> },
}

/// Run one fixture through all assertion layers for `Objects` authoring.
#[allow(clippy::too_many_lines)]
async fn run_objects(fixture: &Fixture, pg_major: u32) -> FixtureResult {
    if !fixture.applies_to(pg_major) {
        return FixtureResult::Skipped;
    }
    match pg_expect_for_major(fixture, pg_major) {
        PgExpect::Skip => return FixtureResult::Skipped,
        PgExpect::ExpectFailure | PgExpect::Success => {}
    }

    let mut failures: Vec<(String, String)> = Vec::new();
    let effective_plan = resolve_plan_for_major(fixture, pg_major);

    // Layer 1.
    match diff::check(fixture) {
        Ok(out) if out.is_ok() => {}
        Ok(out) => failures.push(("diff".into(), format!(
            "missing substrings {:?}; rendered diff:\n{}",
            out.missing, out.rendered
        ))),
        Err(e) => failures.push(("diff".into(), e.to_string())),
    }

    // Layer 2.
    let plan_outcome = match plan::check(fixture) {
        Ok(o) => o,
        Err(e) => {
            failures.push(("plan".into(), e.to_string()));
            return FixtureResult::Ran { failures };
        }
    };
    if let Some(expected) = effective_plan.steps
        && expected != plan_outcome.actual_steps
    {
        failures.push(("plan".into(), format!(
            "expected {} step(s), got {}",
            expected, plan_outcome.actual_steps
        )));
    }
    // Check rewrites from the effective plan.
    // When per_pg overrides the rewrite list, recheck against rendered SQL.
    // Otherwise reuse what plan::check() already computed.
    let effective_missing_rewrites: Vec<String> =
        if effective_plan.rewrites_used == fixture.expect.plan.rewrites_used {
            plan_outcome.missing_rewrites.clone()
        } else {
            effective_plan
                .rewrites_used
                .iter()
                .filter(|kind| !plan_outcome.rendered_sql.contains(kind.as_str()))
                .cloned()
                .collect()
        };
    if !effective_missing_rewrites.is_empty() {
        failures.push(("plan".into(), format!("missing rewrites {effective_missing_rewrites:?}")));
    }

    // Layer 7: intent-shape (mandatory on destructive).
    if let Err(e) = intent_shape::assert_intent_shape(
        &plan_outcome.plan,
        &fixture.expect.intent,
    ) {
        failures.push(("intent_shape".into(), e.to_string()));
    }

    // Layer 6: no-collateral-damage (opt-in via touches_only).
    // assert_touches_only is a no-op when the list is empty.
    if let Err(e) = touches_only::assert_touches_only(
        &plan_outcome.plan,
        &effective_plan.touches_only,
    ) {
        failures.push(("touches_only".into(), e.to_string()));
    }

    // Layer 3.
    match plan::check_golden(fixture, &plan_outcome.rendered_sql, pg_major) {
        Ok(out) if out.is_ok() => {}
        Ok(out) => {
            let detail = out.mismatch.unwrap_or_default();
            let extras = match (out.expected_normalized.as_ref(), out.golden_path.as_ref()) {
                (Some(expected), Some(_path)) => format!(
                    "\n--- expected (normalized) ---\n{}\n--- actual (normalized) ---\n{}",
                    expected, out.actual_normalized
                ),
                _ => String::new(),
            };
            failures.push(("golden".into(), format!("{detail}{extras}")));
        }
        Err(e) => failures.push(("golden".into(), e.to_string())),
    }

    // Layer 4.
    let apply_outcome = match apply::check(fixture, pg_major).await {
        Ok(o) => o,
        Err(e) => {
            failures.push(("apply".into(), e.to_string()));
            return FixtureResult::Ran { failures };
        }
    };
    match &apply_outcome {
        apply::ApplyOutcome::Ok(_)
        | apply::ApplyOutcome::OkExpectedFailure
        | apply::ApplyOutcome::Skipped => {}
        apply::ApplyOutcome::ApplyFailed { stderr, stage } => {
            failures.push(("apply".into(), format!("{stage} failed:\n{stderr}")));
        }
        apply::ApplyOutcome::IrMismatch(diff_str) => {
            failures.push(("apply".into(), format!("post-apply IR diverged:\n{diff_str}")));
        }
        apply::ApplyOutcome::UnexpectedSuccess => {
            failures.push(("apply".into(), "fixture expected apply.succeeds=false but apply succeeded".into()));
        }
    }

    // Layer 5: minimality.
    if effective_plan.minimality
        && let apply::ApplyOutcome::Ok(state) = &apply_outcome
    {
        let input = minimality::MinimalityInput {
            post_apply_catalog: &state.catalog,
            post_apply_drift: &state.drift,
            after_source: &state.after_source,
        };
        if let Err(e) = minimality::assert_minimal(&input) {
            failures.push(("minimality".into(), e.to_string()));
        }
    }

    // Layer 8: dep-graph golden (default-on, opt-out via
    // expect.dep_graph.enabled = false).
    if fixture.expect.dep_graph.enabled {
        match parse_sql(&fixture.after_sql, "after") {
            Err(e) => failures.push(("dep_graph".into(), e.to_string())),
            Ok(source_catalog) => {
                if let Err(e) = dep_graph::assert_dep_graph_golden(
                    &source_catalog,
                    &fixture.dir,
                    &fixture.expect.dep_graph.golden,
                ) {
                    failures.push(("dep_graph".into(), e.to_string()));
                }
            }
        }
    }

    // Layer 9: topological order (opt-in via expect.plan.order).
    if let Err(e) = topological_order::assert_order(
        &plan_outcome.plan,
        &effective_plan.order,
    ) {
        failures.push(("topological_order".into(), e.to_string()));
    }

    FixtureResult::Ran { failures }
}

/// Run one fixture through assertion layers for `Scenarios` authoring.
fn run_scenarios(fixture: &Fixture, pg_major: u32) -> FixtureResult {
    if !fixture.applies_to(pg_major) {
        return FixtureResult::Skipped;
    }
    match pg_expect_for_major(fixture, pg_major) {
        PgExpect::Skip => return FixtureResult::Skipped,
        PgExpect::ExpectFailure | PgExpect::Success => {}
    }

    let mut failures: Vec<(String, String)> = Vec::new();
    let effective_plan = resolve_plan_for_major(fixture, pg_major);

    // Layer 2.
    let plan_outcome = match plan::check(fixture) {
        Ok(o) => o,
        Err(e) => {
            failures.push(("plan".into(), e.to_string()));
            return FixtureResult::Ran { failures };
        }
    };
    if let Some(expected) = effective_plan.steps
        && expected != plan_outcome.actual_steps
    {
        failures.push(("plan".into(), format!(
            "expected {} step(s), got {}",
            expected, plan_outcome.actual_steps
        )));
    }

    // Layer 7: intent-shape (mandatory on destructive).
    if let Err(e) = intent_shape::assert_intent_shape(
        &plan_outcome.plan,
        &fixture.expect.intent,
    ) {
        failures.push(("intent_shape".into(), e.to_string()));
    }

    // Layer 8: dep-graph golden (opt-out via expect.dep_graph.enabled = false).
    if fixture.expect.dep_graph.enabled {
        match parse_sql(&fixture.after_sql, "after") {
            Err(e) => failures.push(("dep_graph".into(), e.to_string())),
            Ok(source_catalog) => {
                if let Err(e) = dep_graph::assert_dep_graph_golden(
                    &source_catalog,
                    &fixture.dir,
                    &fixture.expect.dep_graph.golden,
                ) {
                    failures.push(("dep_graph".into(), e.to_string()));
                }
            }
        }
    }

    // Layer 9: topological order (opt-in via expect.plan.order).
    if let Err(e) = topological_order::assert_order(
        &plan_outcome.plan,
        &effective_plan.order,
    ) {
        failures.push(("topological_order".into(), e.to_string()));
    }

    FixtureResult::Ran { failures }
}

/// Run one fixture through assertion layers for `Intent` authoring.
fn run_intent(fixture: &Fixture, pg_major: u32) -> FixtureResult {
    if !fixture.applies_to(pg_major) {
        return FixtureResult::Skipped;
    }
    match pg_expect_for_major(fixture, pg_major) {
        PgExpect::Skip => return FixtureResult::Skipped,
        PgExpect::ExpectFailure | PgExpect::Success => {}
    }

    let mut failures: Vec<(String, String)> = Vec::new();
    let effective_plan = resolve_plan_for_major(fixture, pg_major);

    // Layer 1: diff substrings.
    match diff::check(fixture) {
        Ok(out) if out.is_ok() => {}
        Ok(out) => failures.push(("diff".into(), format!(
            "missing substrings {:?}; rendered diff:\n{}",
            out.missing, out.rendered
        ))),
        Err(e) => failures.push(("diff".into(), e.to_string())),
    }

    // Layer 2: plan structural invariants.
    let plan_outcome = match plan::check(fixture) {
        Ok(o) => o,
        Err(e) => {
            failures.push(("plan".into(), e.to_string()));
            return FixtureResult::Ran { failures };
        }
    };
    if let Some(expected) = effective_plan.steps
        && expected != plan_outcome.actual_steps
    {
        failures.push(("plan".into(), format!(
            "expected {} step(s), got {}",
            expected, plan_outcome.actual_steps
        )));
    }

    // Layer 7: intent-shape (mandatory on destructive).
    if let Err(e) = intent_shape::assert_intent_shape(
        &plan_outcome.plan,
        &fixture.expect.intent,
    ) {
        failures.push(("intent_shape".into(), e.to_string()));
    }

    // Layer 4: apply — deferred for intent/ fixtures until runner-side
    // intent auto-approval is wired. The plan has unapproved destructive
    // intents, so apply would fail. L1 + L7 fire above; L4 success for
    // intent/ fixtures is a stretch goal.
    // TODO(T3-followup): set approved=true on each generated intent row
    // before invoking apply::check so that L4 also passes for intent/ fixtures.

    FixtureResult::Ran { failures }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn conformance_suite() {
    let pg_major = active_pg_major();
    let root = cases_root();

    // Suite-total budget (default 300s; override via env var for Docker-gated CI jobs).
    let suite_budget_secs: u64 = std::env::var("PGEVOLVE_CONFORMANCE_SUITE_BUDGET_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);

    // Timings TSV — written to target/ at the workspace root.
    let timings_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/conformance-timings.tsv");

    let fixtures = walk::discover(&root).expect("fixture discovery failed");
    assert!(
        !fixtures.is_empty(),
        "no fixtures discovered under {}",
        root.display()
    );

    let mut report = Report::default();
    let mut ran = 0usize;
    let mut skipped = 0usize;
    let mut total_elapsed = std::time::Duration::ZERO;

    for discovered in &fixtures {
        let fixture = &discovered.fixture;
        let dir = &fixture.dir;

        let budget = std::time::Duration::from_secs(fixture.budget.seconds);
        let start = std::time::Instant::now();

        // Per-fixture wall-clock budget enforced via tokio::time::timeout.
        // Each per-authoring helper returns a FixtureResult collecting layer failures.
        let timeout_result = tokio::time::timeout(budget, async {
            match discovered.authoring {
                Authoring::Objects => run_objects(fixture, pg_major).await,
                Authoring::Scenarios => run_scenarios(fixture, pg_major),
                Authoring::Intent => run_intent(fixture, pg_major),
                Authoring::Failure => {
                    let failures = pgevolve_conformance::failure::run_failure_fixture(fixture)
                        .err()
                        .map(|e| vec![("failure".to_string(), e.to_string())])
                        .unwrap_or_default();
                    FixtureResult::Ran { failures }
                }
                Authoring::Regressions => {
                    // T0: regressions subtree discovered but not yet wired; skip cleanly.
                    eprintln!(
                        "skip {}: authoring {:?} not yet wired",
                        dir.display(),
                        discovered.authoring
                    );
                    FixtureResult::Skipped
                }
            }
        })
        .await;

        let elapsed = start.elapsed();
        append_timing(&timings_path, dir, elapsed);
        total_elapsed += elapsed;

        // Per-fixture budget overrun — hard fail immediately.
        assert!(
            timeout_result.is_ok(),
            "fixture {} exceeded {}s budget (elapsed {:.2}s)",
            dir.display(),
            fixture.budget.seconds,
            elapsed.as_secs_f64(),
        );
        match timeout_result.unwrap() {
            FixtureResult::Skipped => {
                skipped += 1;
            }
            FixtureResult::Ran { failures } => {
                ran += 1;
                for (layer, detail) in failures {
                    report.fail(dir, &layer, detail);
                }
            }
        }
    }

    // Suite-total budget check.
    assert!(
        total_elapsed.as_secs() <= suite_budget_secs,
        "conformance suite exceeded {}s budget (total elapsed {:.2}s)",
        suite_budget_secs,
        total_elapsed.as_secs_f64(),
    );

    eprintln!(
        "conformance: {} fixtures discovered, {} ran (pg{}), {} skipped (version-gated); total {:.2}s",
        fixtures.len(),
        ran,
        pg_major,
        skipped,
        total_elapsed.as_secs_f64(),
    );

    assert!(
        report.failures.is_empty(),
        "{} conformance failure(s):\n\n{}",
        report.failures.len(),
        report.failures.join("\n\n")
    );
}
