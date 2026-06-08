//! End-to-end round-trip for `CREATE TABLE … TABLESPACE`.
//!
//! Provisions a second tablespace backed by a writable on-container directory,
//! applies a source `Catalog` containing a schema and a table placed in that
//! tablespace to a fresh ephemeral Postgres, introspects the live database,
//! and asserts the live state converges with the source — proving the
//! parser → plan → apply → reader loop agrees for table-level `TABLESPACE`.
//!
//! This round-trip validates that:
//! - the parser captures `TABLESPACE ts_e2e` from `CREATE TABLE … TABLESPACE`;
//! - the planner emits `CREATE TABLE … TABLESPACE ts_e2e` in the plan;
//! - the reader reads back the tablespace name from `pg_class.reltablespace`;
//! - `diff(live, source)` is empty after a successful apply.
//!
//! Skipped when Docker is unavailable. `#[ignore]`'d like the other PG-backed
//! property / e2e tests; run with:
//!   `cargo test -p pgevolve --test table_tablespace_e2e -- --ignored`

mod common;

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::parse_directory;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

use common::{apply_diff, assert_convergent, connect_and_bootstrap, introspect, schemas_of};

/// The on-container directory used as the tablespace `LOCATION`.
///
/// Must be empty and owned (or writable) by the postgres system user.
/// We provision it via `COPY … TO PROGRAM 'mkdir -p … && chmod 700 …'`
/// so the directory exists before `CREATE TABLESPACE` is issued.
const TABLESPACE_DIR: &str = "/tmp/pgev_ts_e2e";

/// Source SQL declaring a schema and a table placed in `ts_e2e`.
///
/// The tablespace is pre-provisioned as cluster-level infrastructure before
/// the parser sees this SQL; the parser only needs to capture the identifier.
const SOURCE_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.items (
    id    bigint NOT NULL,
    label text   NOT NULL,
    CONSTRAINT items_pkey PRIMARY KEY (id)
) TABLESPACE ts_e2e;
";

/// Parse `SOURCE_SQL` into a `Catalog` via the source pipeline.
fn source_catalog() -> Result<Catalog> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("0001-table-tablespace.sql"), SOURCE_SQL)?;
    let catalog = parse_directory(dir.path(), &[])?;
    Ok(catalog)
}

/// Provision the tablespace directory and create the tablespace in `pg`.
///
/// Uses `COPY … TO PROGRAM` (a superuser operation available in ephemeral
/// Postgres containers) to create the on-container directory, then
/// `CREATE TABLESPACE` to register it with Postgres.
async fn provision_tablespace(pg: &EphemeralPostgres) -> Result<()> {
    let client = pg.connect().await?;
    client
        .batch_execute(&format!(
            "COPY (SELECT 1) TO PROGRAM 'mkdir -p {TABLESPACE_DIR} && chmod 700 {TABLESPACE_DIR}';"
        ))
        .await?;
    client
        .batch_execute(&format!(
            "CREATE TABLESPACE ts_e2e LOCATION '{TABLESPACE_DIR}';"
        ))
        .await?;
    Ok(())
}

#[ignore = "e2e test — requires Docker; run via `cargo test -- --ignored`"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn table_tablespace_round_trips_against_ephemeral_pg() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    run().await.expect("table TABLESPACE round-trip");
}

async fn run() -> Result<()> {
    let source = source_catalog()?;
    let managed = schemas_of(&source);

    // Sanity: the parsed source carries the table we expect.
    assert_eq!(
        source.tables.len(),
        1,
        "source catalog should declare exactly one table"
    );
    assert!(
        source.tables[0].tablespace.is_some(),
        "parsed table must carry tablespace = Some(\"ts_e2e\")"
    );

    let pg = EphemeralPostgres::start(default_pg_version()).await?;

    // Provision the tablespace before apply so `CREATE TABLE … TABLESPACE`
    // finds the tablespace in `pg_tablespace`.
    provision_tablespace(&pg).await?;

    let mut client = connect_and_bootstrap(&pg).await?;

    // Apply from an empty database to the source state.
    let outcome = apply_diff(&mut client, &Catalog::empty(), &source, &managed, None).await?;
    outcome.map_err(|e| anyhow::anyhow!("apply failed: {e}"))?;

    // Introspect the live database and assert it converges with the source.
    let live = introspect(&pg, &managed).await?;
    assert_eq!(
        live.tables.len(),
        1,
        "live database should report exactly one table after apply"
    );
    assert_eq!(
        live.tables[0]
            .tablespace
            .as_ref()
            .map(pgevolve_core::identifier::Identifier::as_str),
        Some("ts_e2e"),
        "live table tablespace must be ts_e2e after apply"
    );

    assert_convergent(&live, &source)
}
