//! Docker-gated integration tests for the cluster catalog reader.
//!
//! Each test spins up an ephemeral Postgres container (defaulting to PG 16),
//! sets up roles via SQL, then calls [`read_cluster_catalog`] and asserts on
//! the resulting [`ClusterCatalog`].
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is
//! set. Pattern mirrors `catalog_round_trip.rs`.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::PgVersion;
use pgevolve_core::catalog::cluster::read_cluster_catalog;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Spin up a fresh Postgres container, run `setup_sql`, and return a
/// [`ClusterCatalog`] read back with `bootstrap_roles`.
async fn read_catalog_after(
    version: PgVersion,
    setup_sql: &str,
    bootstrap_roles: &[String],
) -> Result<pgevolve_core::ir::cluster::catalog::ClusterCatalog> {
    let pg = EphemeralPostgres::start(version)
        .await
        .context("start ephemeral postgres")?;
    pg.exec_sql(setup_sql).await.context("exec setup SQL")?;

    let client = pg.connect().await.context("connect")?;
    let querier = PgCatalogQuerier::new(client).map_err(|e| anyhow!(e))?;
    let bootstrap_owned: Vec<String> = bootstrap_roles.to_vec();
    tokio::task::spawn_blocking(move || read_cluster_catalog(&querier, &bootstrap_owned))
        .await?
        .map_err(|e| anyhow!(e.to_string()))
}

// ---------------------------------------------------------------------------
// Test 1: reads a simple role with full attributes
// ---------------------------------------------------------------------------

/// Create a role with every attribute set to a non-default value; read it
/// back and assert each field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_simple_role() {
    if !docker_available() {
        eprintln!("skipping reads_simple_role: Docker not available");
        return;
    }

    let sql = r"
        CREATE ROLE atlas_reader
            NOSUPERUSER
            CREATEDB
            NOCREATEROLE
            NOINHERIT
            LOGIN
            NOREPLICATION
            NOBYPASSRLS
            CONNECTION LIMIT 5;
        COMMENT ON ROLE atlas_reader IS 'read-only atlas role';
    ";

    let bootstrap = vec!["postgres".to_string()];
    let cat = read_catalog_after(default_pg_version(), sql, &bootstrap)
        .await
        .expect("read_cluster_catalog");

    let role = cat
        .roles
        .iter()
        .find(|r| r.name.as_str() == "atlas_reader")
        .expect("atlas_reader must appear in catalog");

    assert!(!role.attributes.superuser, "NOSUPERUSER");
    assert!(role.attributes.createdb, "CREATEDB");
    assert!(!role.attributes.createrole, "NOCREATEROLE");
    assert!(!role.attributes.inherit, "NOINHERIT");
    assert!(role.attributes.login, "LOGIN");
    assert!(!role.attributes.replication, "NOREPLICATION");
    assert!(!role.attributes.bypass_rls, "NOBYPASSRLS");
    assert_eq!(
        role.attributes.connection_limit,
        Some(5),
        "CONNECTION LIMIT 5"
    );
    assert_eq!(
        role.comment.as_deref(),
        Some("read-only atlas role"),
        "comment from pg_shdescription"
    );
}

// ---------------------------------------------------------------------------
// Test 2: reads membership edges
// ---------------------------------------------------------------------------

/// Create two roles and a GRANT; assert the `member_of` edge is surfaced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_membership_edges() {
    if !docker_available() {
        eprintln!("skipping reads_membership_edges: Docker not available");
        return;
    }

    let sql = r"
        CREATE ROLE readers;
        CREATE ROLE app_service LOGIN;
        GRANT readers TO app_service;
    ";

    let bootstrap = vec!["postgres".to_string()];
    let cat = read_catalog_after(default_pg_version(), sql, &bootstrap)
        .await
        .expect("read_cluster_catalog");

    let app_service = cat
        .roles
        .iter()
        .find(|r| r.name.as_str() == "app_service")
        .expect("app_service must appear in catalog");

    assert!(
        app_service
            .member_of
            .iter()
            .any(|id| id.as_str() == "readers"),
        "app_service must be a member of readers; member_of = {:?}",
        app_service.member_of
    );
}

// ---------------------------------------------------------------------------
// Test 3: filters predefined and bootstrap roles
// ---------------------------------------------------------------------------

/// Read a fresh cluster (no setup SQL) and verify that no `pg_*` roles appear
/// and that `postgres` is absent when it is in the bootstrap list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filters_predefined_and_bootstrap_roles() {
    if !docker_available() {
        eprintln!("skipping filters_predefined_and_bootstrap_roles: Docker not available");
        return;
    }

    let bootstrap = vec!["postgres".to_string(), "pgevolve".to_string()];
    let cat = read_catalog_after(default_pg_version(), "", &bootstrap)
        .await
        .expect("read_cluster_catalog");

    // No predefined `pg_*` role should appear.
    for role in &cat.roles {
        assert!(
            !role.name.as_str().starts_with("pg_"),
            "predefined role {0:?} must be filtered out",
            role.name.as_str()
        );
    }

    // The bootstrap roles themselves must be absent.
    for name in &bootstrap {
        assert!(
            !cat.roles.iter().any(|r| r.name.as_str() == name),
            "bootstrap role {name:?} must be filtered out"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4: bootstrap filter excludes membership edges to bootstrap roles
// ---------------------------------------------------------------------------

/// Verify that if `postgres` is in `bootstrap_roles`, membership edges that
/// point to `postgres` do not appear in any role's `member_of` list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_filter_excludes_membership_to_bootstrap() {
    if !docker_available() {
        eprintln!(
            "skipping bootstrap_filter_excludes_membership_to_bootstrap: Docker not available"
        );
        return;
    }

    // Grant the container's `pgevolve` user membership in edge_role (requires
    // superuser; the container user is superuser).
    let sql = r"
        CREATE ROLE edge_role LOGIN;
        GRANT pgevolve TO edge_role;
    ";

    // Both `postgres` and `pgevolve` are bootstrap roles.
    let bootstrap = vec!["postgres".to_string(), "pgevolve".to_string()];

    // Pre-assertion: confirm the GRANT actually landed in pg_auth_members.
    // Without this, a container-user-name change would make the GRANT no-op
    // and the test would pass vacuously.
    {
        let pg = EphemeralPostgres::start(default_pg_version())
            .await
            .expect("start ephemeral postgres for pre-assertion");
        pg.exec_sql(sql).await.expect("exec setup SQL");
        let client = pg.connect().await.expect("connect");
        let rows = client
            .query(
                "SELECT 1 FROM pg_auth_members am \
                 JOIN pg_authid memb   ON memb.oid   = am.member \
                 JOIN pg_authid parent ON parent.oid = am.roleid \
                 WHERE memb.rolname = 'edge_role' \
                   AND parent.rolname = 'pgevolve'",
                &[],
            )
            .await
            .expect("pg_auth_members query");
        assert!(
            !rows.is_empty(),
            "test setup failed: GRANT pgevolve TO edge_role did not produce \
             a pg_auth_members row — the EphemeralPostgres container superuser \
             name may have changed from 'pgevolve'"
        );
    }

    let cat = read_catalog_after(default_pg_version(), sql, &bootstrap)
        .await
        .expect("read_cluster_catalog");

    // `edge_role` exists and its `member_of` list must not contain any
    // bootstrap role name.
    let edge_role = cat
        .roles
        .iter()
        .find(|r| r.name.as_str() == "edge_role")
        .expect("edge_role must appear in catalog");

    for parent in &edge_role.member_of {
        assert!(
            !bootstrap.iter().any(|b| b == parent.as_str()),
            "member_of must not contain bootstrap role {0:?}",
            parent.as_str()
        );
    }
}
