//! Docker-gated read tests for VIEW WITH CHECK OPTION catalog decoding.
//!
//! Each test spins up an ephemeral Postgres container, creates a view with
//! a specific `check_option` setting, reads the catalog back, and asserts that
//! `check_option` is decoded correctly.
//!
//! Skipped when Docker is unavailable. Pattern mirrors `catalog_reloptions.rs`.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::view::CheckOption;
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
// Test 1: WITH LOCAL CHECK OPTION
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_view_with_local_check_option() {
    if !docker_available() {
        eprintln!("skipping read_view_with_local_check_option: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint PRIMARY KEY, deleted boolean NOT NULL DEFAULT false);
        CREATE VIEW app.live AS SELECT * FROM app.t WHERE NOT deleted
            WITH LOCAL CHECK OPTION;
        ",
    )
    .await
    .expect("catalog read should succeed");

    let view = cat
        .views
        .iter()
        .find(|v| v.qname.name.as_str() == "live")
        .expect("view 'live' should be present");
    assert_eq!(
        view.check_option,
        Some(CheckOption::Local),
        "expected Local check option"
    );
}

// ---------------------------------------------------------------------------
// Test 2: WITH CASCADED CHECK OPTION
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_view_with_cascaded_check_option() {
    if !docker_available() {
        eprintln!("skipping read_view_with_cascaded_check_option: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint PRIMARY KEY, deleted boolean NOT NULL DEFAULT false);
        CREATE VIEW app.live AS SELECT * FROM app.t WHERE NOT deleted
            WITH CASCADED CHECK OPTION;
        ",
    )
    .await
    .expect("catalog read should succeed");

    let view = cat
        .views
        .iter()
        .find(|v| v.qname.name.as_str() == "live")
        .expect("view 'live' should be present");
    assert_eq!(
        view.check_option,
        Some(CheckOption::Cascaded),
        "expected Cascaded check option"
    );
}

// ---------------------------------------------------------------------------
// Test 3: no check option → None
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_view_without_check_option_is_none() {
    if !docker_available() {
        eprintln!("skipping read_view_without_check_option_is_none: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint PRIMARY KEY, deleted boolean NOT NULL DEFAULT false);
        CREATE VIEW app.live AS SELECT * FROM app.t WHERE NOT deleted;
        ",
    )
    .await
    .expect("catalog read should succeed");

    let view = cat
        .views
        .iter()
        .find(|v| v.qname.name.as_str() == "live")
        .expect("view 'live' should be present");
    assert_eq!(view.check_option, None, "expected no check option");
}
