//! Docker-gated round-trip tests for STATISTICS catalog reading.
//!
//! Each test spins up an ephemeral Postgres container, creates one or more
//! `CREATE STATISTICS` objects, reads the catalog back, and asserts the
//! `Statistic` IR fields.
//!
//! Expression statistics (`CREATE STATISTICS … ON (expr)`) are decoded via
//! `pg_get_statisticsobjdef_expressions`, which is available on all supported
//! PG versions (14–17).
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is set.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::statistic::{StatisticColumn, StatisticKinds};
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn read_catalog_from_sql(sql: &str) -> Result<pgevolve_core::ir::catalog::Catalog> {
    let version = default_pg_version();
    let pg = EphemeralPostgres::start(version)
        .await
        .context("start ephemeral postgres")?;
    pg.exec_sql(sql).await.context("exec setup SQL")?;
    let client = pg.connect().await.context("connect for catalog read")?;
    let querier = PgCatalogQuerier::new(client).map_err(|e| anyhow!(e))?;
    let managed = vec![Identifier::from_unquoted("app").map_err(|e| anyhow!(e))?];
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    catalog.canonicalize().map_err(|e| anyhow!(e.to_string()))
}

// ---------------------------------------------------------------------------
// Test 1: basic ndistinct + dependencies statistic on two columns
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_statistic_basic() {
    if !docker_available() {
        eprintln!("skipping read_statistic_basic: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint PRIMARY KEY, a integer NOT NULL, b integer NOT NULL);
        CREATE STATISTICS app.t_corr (ndistinct, dependencies) ON a, b FROM app.t;
        ",
    )
    .await
    .expect("read catalog");

    assert_eq!(cat.statistics.len(), 1, "expected exactly one statistic");
    let s = &cat.statistics[0];

    assert_eq!(s.qname.schema.as_str(), "app");
    assert_eq!(s.qname.name.as_str(), "t_corr");
    assert_eq!(s.target.schema.as_str(), "app");
    assert_eq!(s.target.name.as_str(), "t");

    assert_eq!(
        s.kinds,
        StatisticKinds {
            ndistinct: true,
            dependencies: true,
            mcv: false,
        },
        "expected ndistinct + dependencies only"
    );

    assert_eq!(s.columns.len(), 2, "expected two column entries");

    // Canon sorts Column entries alphabetically.
    assert!(
        matches!(&s.columns[0], StatisticColumn::Column(id) if id.as_str() == "a"),
        "first column should be 'a', got: {:?}",
        s.columns[0]
    );
    assert!(
        matches!(&s.columns[1], StatisticColumn::Column(id) if id.as_str() == "b"),
        "second column should be 'b', got: {:?}",
        s.columns[1]
    );

    assert!(
        s.statistics_target.is_none(),
        "no explicit STATISTICS target"
    );
}

// ---------------------------------------------------------------------------
// Test 2: expression-form statistic
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_statistic_with_expression() {
    if !docker_available() {
        eprintln!("skipping read_statistic_with_expression: Docker not available");
        return;
    }

    // Expression statistics (CREATE STATISTICS ON (expr)) are supported on all
    // maintained PG versions (PG 14+). No version gate needed.
    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.t (id bigint PRIMARY KEY, name text NOT NULL);
        CREATE STATISTICS app.t_lower ON (lower(name)) FROM app.t;
        ",
    )
    .await
    .expect("read catalog");

    assert_eq!(cat.statistics.len(), 1, "expected exactly one statistic");
    let s = &cat.statistics[0];

    assert_eq!(s.qname.name.as_str(), "t_lower");

    assert_eq!(
        s.columns.len(),
        1,
        "expected one expression column, got {:?}",
        s.columns
    );
    assert!(
        matches!(&s.columns[0], StatisticColumn::Expression(_)),
        "expected Expression variant, got: {:?}",
        s.columns[0]
    );
}
