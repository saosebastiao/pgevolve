//! Docker-gated read tests for RLS policies + RLS table flags.
//!
//! Each test spins up an ephemeral Postgres container, sets up tables / policies,
//! reads the catalog back, and asserts that `rls_enabled`, `rls_forced`, and
//! `policies` are populated correctly.
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is set.
//! Pattern mirrors `catalog_grants.rs`.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::grant::GrantTarget;
use pgevolve_core::ir::policy::PolicyCommand;
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
// Test 1: rls_enabled flag
// ---------------------------------------------------------------------------

/// Verify that `rls_enabled` is populated when `ENABLE ROW LEVEL SECURITY` is
/// set on a table, and that `rls_forced` remains false when not forced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_rls_enabled_flag() {
    if !docker_available() {
        eprintln!("skipping reads_rls_enabled_flag: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint, author text);
        ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
        ",
    )
    .await
    .expect("read catalog");

    let t = cat
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == "docs")
        .expect("table app.docs must appear in catalog");

    assert!(
        t.rls_enabled,
        "rls_enabled must be true after ENABLE ROW LEVEL SECURITY"
    );
    assert!(!t.rls_forced, "rls_forced must be false when not forced");
}

// ---------------------------------------------------------------------------
// Test 2: rls_forced flag
// ---------------------------------------------------------------------------

/// Verify that `rls_forced` is populated when `FORCE ROW LEVEL SECURITY` is
/// set. In Postgres, `relforcerowsecurity` and `relrowsecurity` are independent
/// flags — FORCE alone does not implicitly ENABLE, so we also enable explicitly
/// to test the interaction of both flags being true.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_rls_forced_flag() {
    if !docker_available() {
        eprintln!("skipping reads_rls_forced_flag: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint);
        ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
        ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
        ",
    )
    .await
    .expect("read catalog");

    let t = cat
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == "docs")
        .expect("table app.docs must appear in catalog");

    assert!(
        t.rls_forced,
        "rls_forced must be true after FORCE ROW LEVEL SECURITY"
    );
    assert!(
        t.rls_enabled,
        "rls_enabled must be true after ENABLE ROW LEVEL SECURITY"
    );
}

// ---------------------------------------------------------------------------
// Test 3: simple permissive policy
// ---------------------------------------------------------------------------

/// Verify a basic permissive FOR ALL policy with the default `TO PUBLIC`
/// (omitted in the CREATE POLICY source, so PG stores `{public}`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_simple_policy() {
    if !docker_available() {
        eprintln!("skipping reads_simple_policy: Docker not available");
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint, author text);
        CREATE POLICY author_only ON app.docs USING (author = current_user);
        ",
    )
    .await
    .expect("read catalog");

    let t = cat
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == "docs")
        .expect("table app.docs must appear in catalog");

    assert_eq!(
        t.policies.len(),
        1,
        "expected exactly one policy; got: {:#?}",
        t.policies
    );
    let p = &t.policies[0];
    assert_eq!(p.name.as_str(), "author_only");
    assert!(p.permissive, "policy must be permissive (default)");
    assert_eq!(p.command, PolicyCommand::All, "default command must be ALL");
    // Omitting TO clause → PG stores {public} → should decode as [GrantTarget::Public].
    assert_eq!(p.roles.len(), 1, "expected exactly one role (public)");
    assert!(
        matches!(p.roles[0], GrantTarget::Public),
        "omitted TO clause must round-trip as Public; got: {:?}",
        p.roles[0]
    );
    assert!(p.using.is_some(), "USING clause must be present");
    assert!(
        p.with_check.is_none(),
        "WITH CHECK must be absent for permissive ALL policy"
    );
}

// ---------------------------------------------------------------------------
// Test 4: restrictive policy with explicit roles and WITH CHECK
// ---------------------------------------------------------------------------

/// Verify a restrictive FOR INSERT policy with explicit named roles and a
/// WITH CHECK expression (no USING for INSERT-only policies).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_restrictive_policy_with_roles_and_with_check() {
    if !docker_available() {
        eprintln!(
            "skipping reads_restrictive_policy_with_roles_and_with_check: Docker not available"
        );
        return;
    }

    let cat = read_catalog_from_sql(
        r"
        CREATE ROLE readers;
        CREATE ROLE writers;
        CREATE SCHEMA app;
        CREATE TABLE app.docs (id bigint, author text);
        CREATE POLICY only_authors ON app.docs
            AS RESTRICTIVE
            FOR INSERT
            TO writers, readers
            WITH CHECK (author = current_user);
        ",
    )
    .await
    .expect("read catalog");

    let t = cat
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == "docs")
        .expect("table app.docs must appear in catalog");

    let p = t
        .policies
        .iter()
        .find(|p| p.name.as_str() == "only_authors")
        .expect("policy only_authors must appear");

    assert!(!p.permissive, "policy must be restrictive");
    assert_eq!(p.command, PolicyCommand::Insert);

    // Roles: the query orders by policyname, but roles within a policy come
    // back in whatever order pg_policies returns. Collect them and sort by
    // name for a stable assertion.
    let mut role_names: Vec<&str> = p
        .roles
        .iter()
        .filter_map(|r| match r {
            GrantTarget::Role(n) => Some(n.as_str()),
            GrantTarget::Public => None,
        })
        .collect();
    role_names.sort_unstable();
    assert_eq!(
        role_names,
        vec!["readers", "writers"],
        "roles must be readers and writers; got: {:?}",
        p.roles
    );

    // INSERT policies have no USING clause in PG.
    assert!(
        p.using.is_none(),
        "INSERT policy must not have a USING clause"
    );
    assert!(p.with_check.is_some(), "WITH CHECK must be present");
}
