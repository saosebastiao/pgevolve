//! `cargo xtask` — workspace developer tooling.
//!
//! Subcommands:
//!
//! - `bless` — regenerate Tier-3 catalog round-trip goldens by spinning up
//!   ephemeral Postgres containers, applying each `source.sql`, introspecting
//!   the resulting catalog, and writing the canonical JSON to `expected.json`.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

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
        "bless" => bless().await,
        "" | "help" | "--help" | "-h" => {
            eprintln!("usage: cargo xtask <bless>");
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
    let catalog = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    catalog_snapshotter::to_canonical_json(&catalog)
}

fn workspace_root() -> Result<PathBuf> {
    // CARGO_MANIFEST_DIR for xtask is `<workspace>/xtask`.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    Ok(p)
}
