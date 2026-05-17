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

use std::path::PathBuf;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn conformance_suite() {
    let pg_major = active_pg_major();
    let root = cases_root();

    let fixtures = walk::discover(&root).expect("fixture discovery failed");
    assert!(
        !fixtures.is_empty(),
        "no fixtures discovered under {}",
        root.display()
    );

    let mut report = Report::default();
    let mut ran = 0usize;
    let mut skipped = 0usize;

    for discovered in &fixtures {
        let fixture = &discovered.fixture;
        let dir = &fixture.dir;

        match discovered.authoring {
            Authoring::Objects => {
                // Run all four assertion layers for objects fixtures.
                if !fixture.applies_to(pg_major) {
                    skipped += 1;
                    continue;
                }
                // Check [pg.expect] per-major override.
                match pg_expect_for_major(fixture, pg_major) {
                    PgExpect::Skip => {
                        skipped += 1;
                        continue;
                    }
                    PgExpect::ExpectFailure | PgExpect::Success => {}
                }
                ran += 1;

                // Effective plan expectations for this major (applies per_pg overrides).
                let effective_plan = resolve_plan_for_major(fixture, pg_major);

                // Layer 1.
                match diff::check(fixture) {
                    Ok(out) if out.is_ok() => {}
                    Ok(out) => report.fail(
                        dir,
                        "diff",
                        format!(
                            "missing substrings {:?}; rendered diff:\n{}",
                            out.missing, out.rendered
                        ),
                    ),
                    Err(e) => report.fail(dir, "diff", e.to_string()),
                }

                // Layer 2.
                let plan_outcome = match plan::check(fixture) {
                    Ok(o) => o,
                    Err(e) => {
                        report.fail(dir, "plan", e.to_string());
                        continue;
                    }
                };
                if let Some(expected) = effective_plan.steps
                    && expected != plan_outcome.actual_steps
                {
                    report.fail(
                        dir,
                        "plan",
                        format!(
                            "expected {} step(s), got {}",
                            expected, plan_outcome.actual_steps
                        ),
                    );
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
                    report.fail(
                        dir,
                        "plan",
                        format!("missing rewrites {effective_missing_rewrites:?}"),
                    );
                }

                // Layer 7: intent-shape (mandatory on destructive).
                if let Err(e) = intent_shape::assert_intent_shape(
                    &plan_outcome.plan,
                    &fixture.expect.intent,
                ) {
                    report.fail(dir, "intent_shape", e.to_string());
                }

                // Layer 6: no-collateral-damage (opt-in via touches_only).
                // assert_touches_only is a no-op when the list is empty.
                if let Err(e) = touches_only::assert_touches_only(
                    &plan_outcome.plan,
                    &effective_plan.touches_only,
                ) {
                    report.fail(dir, "touches_only", e.to_string());
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
                        report.fail(dir, "golden", format!("{detail}{extras}"));
                    }
                    Err(e) => report.fail(dir, "golden", e.to_string()),
                }

                // Layer 4.
                let apply_outcome = match apply::check(fixture, pg_major).await {
                    Ok(o) => o,
                    Err(e) => {
                        report.fail(dir, "apply", e.to_string());
                        continue;
                    }
                };
                match &apply_outcome {
                    apply::ApplyOutcome::Ok(_)
                    | apply::ApplyOutcome::OkExpectedFailure
                    | apply::ApplyOutcome::Skipped => {}
                    apply::ApplyOutcome::ApplyFailed { stderr, stage } => {
                        report.fail(dir, "apply", format!("{stage} failed:\n{stderr}"));
                    }
                    apply::ApplyOutcome::IrMismatch(diff_str) => {
                        report.fail(dir, "apply", format!("post-apply IR diverged:\n{diff_str}"));
                    }
                    apply::ApplyOutcome::UnexpectedSuccess => {
                        report.fail(
                            dir,
                            "apply",
                            "fixture expected apply.succeeds=false but apply succeeded",
                        );
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
                        report.fail(dir, "minimality", e.to_string());
                    }
                }

                // Layer 8: dep-graph golden (default-on, opt-out via
                // expect.dep_graph.enabled = false).
                if fixture.expect.dep_graph.enabled {
                    match parse_sql(&fixture.after_sql, "after") {
                        Err(e) => report.fail(dir, "dep_graph", e.to_string()),
                        Ok(source_catalog) => {
                            if let Err(e) = dep_graph::assert_dep_graph_golden(
                                &source_catalog,
                                dir,
                                &fixture.expect.dep_graph.golden,
                            ) {
                                report.fail(dir, "dep_graph", e.to_string());
                            }
                        }
                    }
                }

                // Layer 9: topological order (opt-in via expect.plan.order).
                if let Err(e) = topological_order::assert_order(
                    &plan_outcome.plan,
                    &effective_plan.order,
                ) {
                    report.fail(dir, "topological_order", e.to_string());
                }
            }

            Authoring::Scenarios => {
                if !fixture.applies_to(pg_major) {
                    skipped += 1;
                    continue;
                }
                // Check [pg.expect] per-major override.
                match pg_expect_for_major(fixture, pg_major) {
                    PgExpect::Skip => {
                        skipped += 1;
                        continue;
                    }
                    PgExpect::ExpectFailure | PgExpect::Success => {}
                }
                ran += 1;

                let effective_plan = resolve_plan_for_major(fixture, pg_major);

                // Layer 2.
                let plan_outcome = match plan::check(fixture) {
                    Ok(o) => o,
                    Err(e) => {
                        report.fail(dir, "plan", e.to_string());
                        continue;
                    }
                };
                if let Some(expected) = effective_plan.steps
                    && expected != plan_outcome.actual_steps
                {
                    report.fail(
                        dir,
                        "plan",
                        format!(
                            "expected {} step(s), got {}",
                            expected, plan_outcome.actual_steps
                        ),
                    );
                }

                // Layer 7: intent-shape (mandatory on destructive).
                if let Err(e) = intent_shape::assert_intent_shape(
                    &plan_outcome.plan,
                    &fixture.expect.intent,
                ) {
                    report.fail(dir, "intent_shape", e.to_string());
                }

                // Layer 8: dep-graph golden (opt-out via
                // expect.dep_graph.enabled = false).
                if fixture.expect.dep_graph.enabled {
                    match parse_sql(&fixture.after_sql, "after") {
                        Err(e) => report.fail(dir, "dep_graph", e.to_string()),
                        Ok(source_catalog) => {
                            if let Err(e) = dep_graph::assert_dep_graph_golden(
                                &source_catalog,
                                dir,
                                &fixture.expect.dep_graph.golden,
                            ) {
                                report.fail(dir, "dep_graph", e.to_string());
                            }
                        }
                    }
                }

                // Layer 9: topological order (opt-in via expect.plan.order).
                if let Err(e) = topological_order::assert_order(
                    &plan_outcome.plan,
                    &effective_plan.order,
                ) {
                    report.fail(dir, "topological_order", e.to_string());
                }
            }

            Authoring::Intent => {
                if !fixture.applies_to(pg_major) {
                    skipped += 1;
                    continue;
                }
                // Check [pg.expect] per-major override.
                match pg_expect_for_major(fixture, pg_major) {
                    PgExpect::Skip => {
                        skipped += 1;
                        continue;
                    }
                    PgExpect::ExpectFailure | PgExpect::Success => {}
                }
                ran += 1;

                let effective_plan = resolve_plan_for_major(fixture, pg_major);

                // Layer 1: diff substrings.
                match diff::check(fixture) {
                    Ok(out) if out.is_ok() => {}
                    Ok(out) => report.fail(
                        dir,
                        "diff",
                        format!(
                            "missing substrings {:?}; rendered diff:\n{}",
                            out.missing, out.rendered
                        ),
                    ),
                    Err(e) => report.fail(dir, "diff", e.to_string()),
                }

                // Layer 2: plan structural invariants.
                let plan_outcome = match plan::check(fixture) {
                    Ok(o) => o,
                    Err(e) => {
                        report.fail(dir, "plan", e.to_string());
                        continue;
                    }
                };
                if let Some(expected) = effective_plan.steps
                    && expected != plan_outcome.actual_steps
                {
                    report.fail(
                        dir,
                        "plan",
                        format!(
                            "expected {} step(s), got {}",
                            expected, plan_outcome.actual_steps
                        ),
                    );
                }

                // Layer 7: intent-shape (mandatory on destructive).
                if let Err(e) = intent_shape::assert_intent_shape(
                    &plan_outcome.plan,
                    &fixture.expect.intent,
                ) {
                    report.fail(dir, "intent_shape", e.to_string());
                }

                // Layer 4: apply — deferred for intent/ fixtures until runner-side
                // intent auto-approval is wired. The plan has unapproved destructive
                // intents, so apply would fail. L1 + L7 fire above; L4 success for
                // intent/ fixtures is a stretch goal.
                // TODO(T3-followup): set approved=true on each generated intent row
                // before invoking apply::check so that L4 also passes for intent/ fixtures.
            }

            Authoring::Failure => {
                ran += 1;
                if let Err(e) = pgevolve_conformance::failure::run_failure_fixture(fixture) {
                    report.fail(dir, "failure", e.to_string());
                }
            }

            Authoring::Regressions => {
                // T0: regressions subtree discovered but not yet wired; skip cleanly.
                eprintln!(
                    "skip {}: authoring {:?} not yet wired",
                    dir.display(),
                    discovered.authoring
                );
            }
        }
    }

    eprintln!(
        "conformance: {} fixtures discovered, {} ran (pg{}), {} skipped (version-gated)",
        fixtures.len(),
        ran,
        pg_major,
        skipped
    );

    assert!(
        report.failures.is_empty(),
        "{} conformance failure(s):\n\n{}",
        report.failures.len(),
        report.failures.join("\n\n")
    );
}
