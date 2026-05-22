//! `cargo xtask` — workspace developer tooling.
//!
//! Subcommands:
//!
//! - `bless` — regenerate Tier-3 catalog round-trip goldens by spinning up
//!   ephemeral Postgres containers, applying each `source.sql`, introspecting
//!   the resulting catalog, and writing the canonical JSON to `expected.json`.
//! - `bless --conformance` — walk every fixture under
//!   `crates/pgevolve-conformance/tests/cases/`, render the in-process planner
//!   pipeline, normalize the output, and write it to the fixture's golden path.
//! - `coverage [--check | --gaps]` — cross-check `docs/spec/*.md` capability
//!   rows against the (capability × change-kind × PG-major) fixture matrix.
//!   `--check` fails if any required cell is uncovered; `--gaps` lists gaps.
//! - `capture-regression --seed <hex> --issue <n>` — scaffold a regression
//!   fixture from a proptest seed.
//! - `verify-regression <fixture-dir>` — confirm a fixture fails on the current
//!   branch (proving the bug it captures is still present).
//! - `property-status [--max-age-days N]` — list open property-test GitHub
//!   issues; fail if any exceed the age threshold.
//! - `diagnose-pg-version <fixture-dir> --pg-major N` — run a fixture against
//!   a specific PG major and report suggested fixture.toml edits.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod capture_regression;
mod coverage;
mod diagnose_pg_version;
mod fixture_cost;
mod property_status;
mod verify_regression;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_testkit::catalog_snapshotter;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "bless" => {
            let kind = std::env::args().nth(2);
            match kind.as_deref() {
                Some("--conformance") => bless_conformance(),
                _ => bless().await,
            }
        }
        "coverage" => {
            let flag = std::env::args().nth(2);
            let mode = match flag.as_deref() {
                Some("--gaps") => coverage::CoverageMode::Gaps,
                // default: --check
                _ => coverage::CoverageMode::Check,
            };
            coverage::run(mode, &workspace_root()?)
        }
        "fixture-cost" => fixture_cost::run(),
        "capture-regression" => {
            let args: Vec<String> = std::env::args().collect();
            let seed = flag_value(&args, "--seed")
                .ok_or_else(|| anyhow!("capture-regression requires --seed <hex>"))?;
            let issue_str = flag_value(&args, "--issue")
                .ok_or_else(|| anyhow!("capture-regression requires --issue <n>"))?;
            let issue: u64 = issue_str
                .parse()
                .map_err(|_| anyhow!("--issue must be a positive integer"))?;
            capture_regression::run(&seed, issue)
        }
        "verify-regression" => {
            let fixture_dir = std::env::args()
                .nth(2)
                .ok_or_else(|| anyhow!("verify-regression requires <fixture-dir>"))?;
            verify_regression::run(std::path::Path::new(&fixture_dir))
        }
        "property-status" => {
            let args: Vec<String> = std::env::args().collect();
            let max_age_days: u64 = flag_value(&args, "--max-age-days")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            property_status::run(max_age_days)
        }
        "diagnose-pg-version" => {
            let fixture_dir = std::env::args()
                .nth(2)
                .ok_or_else(|| anyhow!("diagnose-pg-version requires <fixture-dir>"))?;
            let args: Vec<String> = std::env::args().collect();
            let pg_major_str = flag_value(&args, "--pg-major")
                .ok_or_else(|| anyhow!("diagnose-pg-version requires --pg-major <n>"))?;
            let pg_major: u32 = pg_major_str
                .parse()
                .map_err(|_| anyhow!("--pg-major must be a positive integer"))?;
            diagnose_pg_version::run(std::path::Path::new(&fixture_dir), pg_major)
        }
        "" | "help" | "--help" | "-h" => {
            eprintln!(
                "usage: cargo xtask <bless | bless --conformance | coverage [--check | --gaps] | fixture-cost |\n\
                 \t capture-regression --seed <hex> --issue <n> |\n\
                 \t verify-regression <fixture-dir> |\n\
                 \t property-status [--max-age-days N] |\n\
                 \t diagnose-pg-version <fixture-dir> --pg-major N>"
            );
            Ok(())
        }
        other => Err(anyhow!("unknown subcommand: {other}")),
    }
}

async fn bless() -> Result<()> {
    if !docker_available() {
        return Err(anyhow!(
            "`cargo xtask bless` requires Docker; PGEVOLVE_DISABLE_DOCKER_TESTS unset"
        ));
    }

    let root = workspace_root()?.join("crates/pgevolve-core/tests/fixtures/catalog");
    if !root.exists() {
        return Err(anyhow!("fixtures dir not found: {}", root.display()));
    }

    for version in [
        PgVersion::Pg14,
        PgVersion::Pg15,
        PgVersion::Pg16,
        PgVersion::Pg17,
    ] {
        let dir = root.join(version.as_tag());
        if !dir.exists() {
            continue;
        }
        for fixture in walkdir::WalkDir::new(&dir).min_depth(1).max_depth(1) {
            let fixture = fixture?;
            if !fixture.file_type().is_dir() {
                continue;
            }
            let case_dir = fixture.path().to_path_buf();
            let source_sql = case_dir.join("source.sql");
            if !source_sql.exists() {
                continue;
            }
            tracing::info!(
                version = ?version,
                fixture = %case_dir.file_name().unwrap_or_default().to_string_lossy(),
                "blessing"
            );
            let snapshot = run_one(version, &source_sql)
                .await
                .with_context(|| format!("fixture {}", case_dir.display()))?;
            let expected = case_dir.join("expected.json");
            std::fs::write(&expected, snapshot)
                .with_context(|| format!("writing {}", expected.display()))?;
        }
    }
    Ok(())
}

async fn run_one(version: PgVersion, source_sql: &Path) -> Result<String> {
    let pg = EphemeralPostgres::start(version).await?;
    let sql = std::fs::read_to_string(source_sql)?;
    pg.exec_sql(&sql).await?;

    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let mut managed = vec![Identifier::from_unquoted("app").map_err(|e| anyhow!(e))?];
    if sql.contains("CREATE SCHEMA billing") {
        managed.push(Identifier::from_unquoted("billing").map_err(|e| anyhow!(e))?);
    }
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    catalog_snapshotter::to_canonical_json(&catalog)
}

fn bless_conformance() -> Result<()> {
    let cases = workspace_root()?.join("crates/pgevolve-conformance/tests/cases");
    if !cases.exists() {
        return Err(anyhow!(
            "conformance cases dir not found: {}",
            cases.display()
        ));
    }

    let mut blessed = 0usize;
    let mut skipped = 0usize;

    for entry in walkdir::WalkDir::new(&cases) {
        let entry = entry?;
        if entry.file_name() != "fixture.toml" {
            continue;
        }
        let dir = entry
            .path()
            .parent()
            .ok_or_else(|| anyhow!("fixture.toml has no parent"))?
            .to_path_buf();
        let fixture = pgevolve_conformance::fixture::Fixture::load(&dir)
            .with_context(|| format!("load fixture {}", dir.display()))?;

        if fixture.meta.authoring == "failure" {
            skipped += 1;
            tracing::info!(fixture = %dir.display(), "skipping failure fixture");
            continue;
        }
        if fixture.meta.authoring == "cluster" {
            bless_cluster_fixture(&dir, &fixture, &mut blessed, &mut skipped)?;
            continue;
        }
        bless_objects_fixture(&dir, &fixture, &mut blessed, &mut skipped)?;
    }

    eprintln!("conformance: blessed {blessed} golden(s); skipped {skipped}");
    Ok(())
}

/// Bless a cluster fixture: write `expected/plan.sql` from the cluster pipeline.
fn bless_cluster_fixture(
    dir: &Path,
    fixture: &pgevolve_conformance::fixture::Fixture,
    blessed: &mut usize,
    skipped: &mut usize,
) -> Result<()> {
    use pgevolve_conformance::normalize::normalize;
    use pgevolve_conformance::planning::render_cluster_plan;

    if let Some(rel) = fixture.expect.plan.golden.as_ref() {
        let out = render_cluster_plan(&fixture.before_sql, &fixture.after_sql)
            .with_context(|| format!("render cluster plan for {}", dir.display()))?;
        let normalized = normalize(&bless_render_cluster_steps(&out.steps));
        bless_write(dir, rel, &normalized)?;
        *blessed += 1;
        tracing::info!(fixture = %dir.display(), "blessed cluster plan.sql");
    } else {
        *skipped += 1;
        tracing::info!(fixture = %dir.display(), "skipping cluster plan.sql: goldening opted out");
    }
    Ok(())
}

/// Bless an objects/scenarios/intent fixture: write `expected/plan.sql` and
/// `expected/dep-graph.dot`.
fn bless_objects_fixture(
    dir: &Path,
    fixture: &pgevolve_conformance::fixture::Fixture,
    blessed: &mut usize,
    skipped: &mut usize,
) -> Result<()> {
    use pgevolve_conformance::assertions::dep_graph::render_dot;
    use pgevolve_conformance::normalize::normalize;
    use pgevolve_conformance::planning::{parse_sql, render_plan};
    use pgevolve_core::plan::{Strategy, build_create_graph};

    if let Some(rel) = fixture.expect.plan.golden.as_ref() {
        let strategy = fixture
            .passthrough
            .planner
            .get("strategy")
            .and_then(|v| v.as_str())
            .map_or(Strategy::Online, |s| match s {
                "atomic" => Strategy::Atomic,
                _ => Strategy::Online,
            });
        let (_plan, rendered_sql, _advisory) =
            render_plan(&fixture.before_sql, &fixture.after_sql, strategy)
                .with_context(|| format!("render plan for {}", dir.display()))?;
        bless_write(dir, rel, &normalize(&rendered_sql))?;
        *blessed += 1;
        tracing::info!(fixture = %dir.display(), "blessed plan.sql");
    } else {
        *skipped += 1;
        tracing::info!(fixture = %dir.display(), "skipping plan.sql: goldening opted out");
    }

    if fixture.expect.dep_graph.enabled {
        let source_catalog = parse_sql(&fixture.after_sql, "after")
            .with_context(|| format!("parse after.sql for dep-graph in {}", dir.display()))?;
        let graph = build_create_graph(&source_catalog);
        let edges: Vec<_> = graph.dep_edges().collect();
        let dot = render_dot(&edges);
        bless_write(dir, &fixture.expect.dep_graph.golden, &dot)?;
        *blessed += 1;
        tracing::info!(fixture = %dir.display(), "blessed dep-graph.dot");
    }
    Ok(())
}

/// Write `content` to `dir/rel`, creating parent directories as needed.
fn bless_write(dir: &Path, rel: &str, content: &str) -> Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

/// Render cluster `RawStep`s to a plain SQL string for golden comparison.
///
/// Each step is rendered on its own line. Mirrors the logic in
/// `tests/run.rs:render_cluster_steps_sql` — keep the two in sync.
fn bless_render_cluster_steps(steps: &[pgevolve_core::plan::RawStep]) -> String {
    if steps.is_empty() {
        return String::new();
    }
    steps
        .iter()
        .map(|s| s.sql.trim_end_matches(';').to_string() + ";")
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn workspace_root() -> Result<PathBuf> {
    // CARGO_MANIFEST_DIR for xtask is `<workspace>/xtask`.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    Ok(p)
}

/// Return the value for a `--flag value` pair in an argument list.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1).cloned()
}
