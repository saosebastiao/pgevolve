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

#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod coverage;
mod fixture_cost;

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
        "" | "help" | "--help" | "-h" => {
            eprintln!("usage: cargo xtask <bless | bless --conformance | coverage [--check | --gaps] | fixture-cost>");
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
    use pgevolve_conformance::assertions::dep_graph::render_dot;
    use pgevolve_conformance::fixture::Fixture;
    use pgevolve_conformance::normalize::normalize;
    use pgevolve_conformance::planning::{parse_sql, render_plan};
    use pgevolve_core::plan::{Strategy, build_create_graph};

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
        let fixture = Fixture::load(&dir)
            .with_context(|| format!("load fixture {}", dir.display()))?;

        // --- plan.sql golden ---
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

            let (_plan, rendered_sql) =
                render_plan(&fixture.before_sql, &fixture.after_sql, strategy)
                    .with_context(|| format!("render plan for {}", dir.display()))?;
            let normalized = normalize(&rendered_sql);

            let golden_path = dir.join(rel);
            if let Some(parent) = golden_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&golden_path, normalized)
                .with_context(|| format!("write {}", golden_path.display()))?;
            blessed += 1;
            tracing::info!(fixture = %dir.display(), "blessed plan.sql");
        } else {
            skipped += 1;
            tracing::info!(
                fixture = %dir.display(),
                "skipping plan.sql: goldening opted out"
            );
        }

        // --- dep-graph.dot golden ---
        if fixture.expect.dep_graph.enabled {
            let source_catalog = parse_sql(&fixture.after_sql, "after")
                .with_context(|| format!("parse after.sql for dep-graph in {}", dir.display()))?;
            let graph = build_create_graph(&source_catalog);
            let edges: Vec<_> = graph.dep_edges().collect();
            let dot = render_dot(&edges);

            let dot_path = dir.join(&fixture.expect.dep_graph.golden);
            if let Some(parent) = dot_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dot_path, &dot)
                .with_context(|| format!("write {}", dot_path.display()))?;
            blessed += 1;
            tracing::info!(fixture = %dir.display(), "blessed dep-graph.dot");
        }
    }

    eprintln!("conformance: blessed {blessed} golden(s); skipped {skipped}");
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    // CARGO_MANIFEST_DIR for xtask is `<workspace>/xtask`.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    Ok(p)
}
