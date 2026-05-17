//! Conformance suite entry point.
//!
//! Walks `tests/cases/**/fixture.toml` via `walk::discover`, routes each
//! fixture by its `authoring` key, and aggregates failures. Each fixture
//! failure is reported with its directory path so failures are immediately
//! actionable.
//!
//! Driven by env var `PGEVOLVE_TEST_PG_VERSION` (default: 17) so the
//! same suite runs once per major in the CI matrix.

use std::path::PathBuf;

use pgevolve_conformance::assertions::{apply, diff, plan};
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
                ran += 1;

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
                if let Some(expected) = plan_outcome.step_mismatch {
                    report.fail(
                        dir,
                        "plan",
                        format!(
                            "expected {} step(s), got {}",
                            expected, plan_outcome.actual_steps
                        ),
                    );
                }
                if !plan_outcome.missing_rewrites.is_empty() {
                    report.fail(
                        dir,
                        "plan",
                        format!("missing rewrites {:?}", plan_outcome.missing_rewrites),
                    );
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
                match apply::check(fixture, pg_major).await {
                    Ok(o) if o.is_ok() => {}
                    Ok(apply::ApplyOutcome::ApplyFailed { stderr, stage }) => {
                        report.fail(dir, "apply", format!("{stage} failed:\n{stderr}"));
                    }
                    Ok(apply::ApplyOutcome::IrMismatch(diff_str)) => {
                        report.fail(dir, "apply", format!("post-apply IR diverged:\n{diff_str}"));
                    }
                    Ok(apply::ApplyOutcome::UnexpectedSuccess) => {
                        report.fail(
                            dir,
                            "apply",
                            "fixture expected apply.succeeds=false but apply succeeded",
                        );
                    }
                    Ok(apply::ApplyOutcome::Ok | apply::ApplyOutcome::Skipped) => {}
                    Err(e) => report.fail(dir, "apply", e.to_string()),
                }
            }

            Authoring::Scenarios | Authoring::Intent | Authoring::Failure | Authoring::Regressions => {
                // T0: subtrees discovered but not yet wired; skip cleanly.
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
