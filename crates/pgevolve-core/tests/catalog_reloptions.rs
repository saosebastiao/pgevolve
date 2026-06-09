//! Docker-gated read tests for storage reloptions.
//!
//! Each test spins up an ephemeral Postgres container, creates objects with
//! explicit storage parameters, reads the catalog back, and asserts that
//! `storage` fields are populated correctly.
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is
//! set. Pattern mirrors `catalog_grants.rs`.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn read_catalog_from_sql(sql: &str) -> Result<pgevolve_core::ir::catalog::Catalog> {
    let pg = EphemeralPostgres::start(PgVersion::Pg17)
        .await
        .context("start ephemeral postgres")?;
    pg.exec_sql(sql).await.context("exec setup SQL")?;
    let client = pg.connect().await.context("connect")?;
    let querier = PgCatalogQuerier::new(client).map_err(|e| anyhow!(e))?;
    let managed = vec![Identifier::from_unquoted("app").map_err(|e| anyhow!(e))?];
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    catalog.canonicalize().map_err(|e| anyhow!(e.to_string()))
}

// ---------------------------------------------------------------------------
// Test 1: table fillfactor
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_table_fillfactor() {
    if !docker_available() {
        eprintln!("skipping reads_table_fillfactor: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint) WITH (fillfactor = 80);
        ",
    )
    .await
    .expect("read catalog");

    let t = cat
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == "t")
        .expect("table app.t must appear in catalog");

    assert_eq!(t.storage.fillfactor, Some(80), "fillfactor must be 80");
}

// ---------------------------------------------------------------------------
// Test 2: autovacuum disabled
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_autovacuum_disabled() {
    if !docker_available() {
        eprintln!("skipping reads_autovacuum_disabled: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint) WITH (autovacuum_enabled = false);
        ",
    )
    .await
    .expect("read catalog");

    let t = cat
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == "t")
        .expect("table app.t must appear in catalog");

    assert_eq!(
        t.storage
            .extra
            .get("autovacuum_enabled")
            .map(String::as_str),
        Some("false"),
        "autovacuum_enabled must round-trip into extra as the string \"false\""
    );
}

// ---------------------------------------------------------------------------
// Test 3: index fillfactor
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_index_fillfactor() {
    if !docker_available() {
        eprintln!("skipping reads_index_fillfactor: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint);
        CREATE INDEX i ON app.t (id) WITH (fillfactor = 70);
        ",
    )
    .await
    .expect("read catalog");

    let i = cat
        .indexes
        .iter()
        .find(|i| i.qname.name.as_str() == "i")
        .expect("index i must appear in catalog");

    assert_eq!(
        i.storage.fillfactor,
        Some(70),
        "index fillfactor must be 70"
    );
}

// ---------------------------------------------------------------------------
// Test 4: GIN index fastupdate
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_gin_fastupdate() {
    if !docker_available() {
        eprintln!("skipping reads_gin_fastupdate: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE EXTENSION btree_gin;
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint);
        CREATE INDEX i ON app.t USING gin (id) WITH (fastupdate = false);
        ",
    )
    .await
    .expect("read catalog");

    let i = cat
        .indexes
        .iter()
        .find(|i| i.qname.name.as_str() == "i")
        .expect("index i must appear in catalog");

    assert_eq!(
        i.storage.fastupdate,
        Some(false),
        "fastupdate must be false"
    );
}
