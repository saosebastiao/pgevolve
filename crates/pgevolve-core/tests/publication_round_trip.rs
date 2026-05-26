//! Docker-gated round-trip tests for PUBLICATION catalog reading.
//!
//! Each test spins up an ephemeral Postgres container, creates a publication,
//! reads the catalog back, and asserts the `Publication` IR fields.
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is set.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::publication::{PublicationScope, PublishKinds};
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn read_catalog_with_pub(
    version: PgVersion,
    sql: &str,
) -> Result<pgevolve_core::ir::catalog::Catalog> {
    let pg = EphemeralPostgres::start(version)
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
// Test 1: FOR ALL TABLES — works on every supported PG version
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_publication_for_all_tables() {
    if !docker_available() {
        eprintln!("skipping read_publication_for_all_tables: Docker not available");
        return;
    }

    let cat = read_catalog_with_pub(
        default_pg_version(),
        r"
        CREATE SCHEMA app;
        CREATE PUBLICATION p FOR ALL TABLES;
        ",
    )
    .await
    .expect("read catalog");

    assert_eq!(
        cat.publications.len(),
        1,
        "expected exactly one publication"
    );
    let pub_ = &cat.publications[0];
    assert_eq!(pub_.name.as_str(), "p");
    assert!(
        matches!(pub_.scope, PublicationScope::AllTables),
        "expected AllTables scope"
    );
    assert_eq!(
        pub_.publish,
        PublishKinds::pg_default(),
        "publish should be the PG default"
    );
    assert!(!pub_.publish_via_partition_root);
}

// ---------------------------------------------------------------------------
// Test 2: FOR TABLE with column list + row filter (PG 15+)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_publication_for_explicit_tables_with_filter() {
    if !docker_available() {
        eprintln!(
            "skipping read_publication_for_explicit_tables_with_filter: Docker not available"
        );
        return;
    }
    // Row filters and column lists require PG 15+.
    if default_pg_version().major() < 15 {
        eprintln!("skipping read_publication_for_explicit_tables_with_filter: requires PG 15+");
        return;
    }

    let cat = read_catalog_with_pub(
        default_pg_version(),
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.orders (id bigint PRIMARY KEY, status text);
        CREATE PUBLICATION p FOR TABLE app.orders (id) WHERE (status = 'active');
        ",
    )
    .await
    .expect("read catalog");

    assert_eq!(
        cat.publications.len(),
        1,
        "expected exactly one publication"
    );
    let pub_ = &cat.publications[0];
    assert_eq!(pub_.name.as_str(), "p");

    let PublicationScope::Selective { tables, schemas } = &pub_.scope else {
        panic!("expected Selective scope, got {:?}", pub_.scope);
    };
    assert!(schemas.is_empty(), "no schema-scope entries expected");
    assert_eq!(tables.len(), 1, "expected exactly one table entry");

    let t = &tables[0];
    assert_eq!(t.qname.schema.as_str(), "app");
    assert_eq!(t.qname.name.as_str(), "orders");

    // Column list should contain "id".
    let cols = t.columns.as_ref().expect("column list should be present");
    assert_eq!(cols.len(), 1, "expected one column in list");
    assert_eq!(cols[0].as_str(), "id");

    // Row filter should be non-empty (canonicalized form).
    assert!(t.row_filter.is_some(), "row filter should be present");
}
