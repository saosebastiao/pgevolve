//! Tier-3 round-trip golden harness.
//!
//! For each fixture under `tests/fixtures/catalog/<pgN>/<case>/source.sql`,
//! spin up an ephemeral Postgres container of the matching major version,
//! apply the SQL, introspect via [`pgevolve_core::catalog::read_catalog`],
//! serialize to canonical JSON, and compare to the checked-in `expected.json`.
//!
//! Skipped entirely when Docker is not available (or when
//! `PGEVOLVE_DISABLE_DOCKER_TESTS` is set). Generate goldens with
//! `cargo xtask bless`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use pgevolve_core::catalog::{read_catalog, CatalogFilter, PgVersion};
use pgevolve_core::identifier::Identifier;
use pgevolve_testkit::catalog_snapshotter;
use pgevolve_testkit::ephemeral_pg::{docker_available, EphemeralPostgres};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg14_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg14).await {
        panic!("pg14 round-trip failed: {e:#}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg15_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg15).await {
        panic!("pg15 round-trip failed: {e:#}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg16_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg16).await {
        panic!("pg16 round-trip failed: {e:#}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg17_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg17).await {
        panic!("pg17 round-trip failed: {e:#}");
    }
}

async fn run_for(version: PgVersion) -> Result<()> {
    if !docker_available() {
        eprintln!(
            "skipping {} fixtures: Docker not available",
            version.as_tag()
        );
        return Ok(());
    }

    let dir = fixtures_root().join(version.as_tag());
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let case_dir = entry.path();
        let source_sql = case_dir.join("source.sql");
        if !source_sql.exists() {
            continue;
        }
        let expected = case_dir.join("expected.json");
        if !expected.exists() {
            eprintln!(
                "skipping {}: no expected.json (run `cargo xtask bless` first)",
                case_dir.display()
            );
            continue;
        }

        let actual = run_one(version, &source_sql)
            .await
            .with_context(|| format!("fixture {}", case_dir.display()))?;
        let want = std::fs::read_to_string(&expected)?;
        if actual != want {
            return Err(anyhow!(
                "snapshot mismatch in {}\n--- expected ---\n{want}\n--- actual ---\n{actual}",
                case_dir.display()
            ));
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

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("catalog")
}
