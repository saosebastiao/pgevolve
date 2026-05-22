//! Docker-gated read tests for object grants + ownership.
//!
//! Each test spins up an ephemeral Postgres container, sets up objects with
//! explicit ownership and grants, reads the catalog back, and asserts that
//! `owner` and `grants` are populated correctly.
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is
//! set. Pattern mirrors `catalog_round_trip.rs`.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::grant::{GrantTarget, Privilege};
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
// Test 1: table owner + object-level grants + column-level grants
// ---------------------------------------------------------------------------

/// Verify that `owner` and `grants` (object-level + column-level) are
/// populated correctly for tables.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_table_grants_and_owner() {
    if !docker_available() {
        eprintln!("skipping reads_table_grants_and_owner: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE ROLE app_owner;
        CREATE ROLE readers;
        ALTER SCHEMA app OWNER TO app_owner;
        CREATE TABLE app.t (id bigint, name text);
        ALTER TABLE app.t OWNER TO app_owner;
        GRANT SELECT ON app.t TO readers;
        GRANT INSERT (name) ON app.t TO readers;
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
        t.owner.as_ref().map(Identifier::as_str),
        Some("app_owner"),
        "table owner must be app_owner"
    );

    // readers should have an object-level SELECT grant and a column-level INSERT(name) grant.
    // The total grant count includes the owner's explicit ACL entries in relacl, so we
    // assert on presence rather than exact count.
    assert!(
        t.grants.iter().any(|g| g.privilege == Privilege::Select
            && g.columns.is_none()
            && matches!(&g.grantee, GrantTarget::Role(r) if r.as_str() == "readers")),
        "must have readers object-level SELECT grant; grants: {:#?}",
        t.grants
    );
    assert!(
        t.grants.iter().any(|g| g.privilege == Privilege::Insert
            && g.columns.is_some()
            && matches!(&g.grantee, GrantTarget::Role(r) if r.as_str() == "readers")),
        "must have readers column-level INSERT(name) grant; grants: {:#?}",
        t.grants
    );
}

// ---------------------------------------------------------------------------
// Test 2: schema owner + grants (named role + PUBLIC)
// ---------------------------------------------------------------------------

/// Verify schema `owner` and `grants` are correctly populated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_schema_grants_and_owner() {
    if !docker_available() {
        eprintln!("skipping reads_schema_grants_and_owner: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE ROLE app_owner;
        CREATE ROLE readers;
        CREATE SCHEMA app;
        ALTER SCHEMA app OWNER TO app_owner;
        GRANT USAGE ON SCHEMA app TO readers;
        GRANT USAGE, CREATE ON SCHEMA app TO PUBLIC;
        ",
    )
    .await
    .expect("read catalog");

    let s = cat
        .schemas
        .iter()
        .find(|s| s.name.as_str() == "app")
        .expect("schema app must appear in catalog");

    assert_eq!(
        s.owner.as_ref().map(Identifier::as_str),
        Some("app_owner"),
        "schema owner must be app_owner"
    );

    // 1 grant to readers (USAGE) + 2 to PUBLIC (USAGE, CREATE) = 3 after canon.
    assert!(
        s.grants
            .iter()
            .any(|g| g.privilege == Privilege::Usage && matches!(g.grantee, GrantTarget::Public)),
        "must have PUBLIC USAGE grant on schema"
    );
    assert!(
        s.grants
            .iter()
            .any(|g| g.privilege == Privilege::Create && matches!(g.grantee, GrantTarget::Public)),
        "must have PUBLIC CREATE grant on schema"
    );
    assert!(
        s.grants.iter().any(|g| g.privilege == Privilege::Usage
            && matches!(&g.grantee, GrantTarget::Role(r) if r.as_str() == "readers")),
        "must have readers USAGE grant on schema"
    );
}

// ---------------------------------------------------------------------------
// Test 3: sequence owner + grants
// ---------------------------------------------------------------------------

/// Verify sequence `owner` and `grants` are correctly populated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_sequence_grants_and_owner() {
    if !docker_available() {
        eprintln!("skipping reads_sequence_grants_and_owner: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE ROLE app_owner;
        CREATE ROLE readers;
        CREATE SEQUENCE app.seq1;
        ALTER SEQUENCE app.seq1 OWNER TO app_owner;
        GRANT USAGE ON SEQUENCE app.seq1 TO readers;
        ",
    )
    .await
    .expect("read catalog");

    let seq = cat
        .sequences
        .iter()
        .find(|s| s.qname.name.as_str() == "seq1")
        .expect("sequence app.seq1 must appear in catalog");

    assert_eq!(
        seq.owner.as_ref().map(Identifier::as_str),
        Some("app_owner"),
        "sequence owner must be app_owner"
    );
    assert!(
        seq.grants.iter().any(|g| g.privilege == Privilege::Usage
            && matches!(&g.grantee, GrantTarget::Role(r) if r.as_str() == "readers")),
        "must have readers USAGE grant on sequence"
    );
}

// ---------------------------------------------------------------------------
// Test 4: function owner + grants
// ---------------------------------------------------------------------------

/// Verify function `owner` and `grants` are correctly populated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_function_grants_and_owner() {
    if !docker_available() {
        eprintln!("skipping reads_function_grants_and_owner: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE ROLE app_owner;
        CREATE ROLE callers;
        CREATE FUNCTION app.add(a integer, b integer) RETURNS integer
            LANGUAGE sql AS $$ SELECT a + b $$;
        ALTER FUNCTION app.add(integer, integer) OWNER TO app_owner;
        GRANT EXECUTE ON FUNCTION app.add(integer, integer) TO callers;
        ",
    )
    .await
    .expect("read catalog");

    let f = cat
        .functions
        .iter()
        .find(|f| f.qname.name.as_str() == "add")
        .expect("function app.add must appear in catalog");

    assert_eq!(
        f.owner.as_ref().map(Identifier::as_str),
        Some("app_owner"),
        "function owner must be app_owner"
    );
    assert!(
        f.grants.iter().any(|g| g.privilege == Privilege::Execute
            && matches!(&g.grantee, GrantTarget::Role(r) if r.as_str() == "callers")),
        "must have callers EXECUTE grant on function"
    );
}
