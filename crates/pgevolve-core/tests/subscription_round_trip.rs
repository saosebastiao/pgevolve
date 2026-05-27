//! Docker-gated round-trip tests for SUBSCRIPTION catalog reading.
//!
//! Each test spins up an ephemeral Postgres container, creates a subscription
//! row in `pg_subscription`, reads the catalog back, and asserts the
//! `Subscription` IR fields.
//!
//! Strategy for avoiding a live publisher:
//!   - PG 16+ supports `connect = false` in `CREATE SUBSCRIPTION WITH (...)`,
//!     which tells PG to create the row without making any network connection.
//!   - PG 14/15 require at minimum `enabled = false, create_slot = false`, but
//!     still validate the connection at CREATE time. For those versions we
//!     self-subscribe (connect to the same server) to avoid needing a separate
//!     publisher container.
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is set.
//!
//! Note: `CREATE SUBSCRIPTION` requires superuser. Ephemeral containers run as
//! the `pgevolve` user which is a superuser in the official postgres image.

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Start an ephemeral Postgres, run setup SQL, then read the catalog via a
/// fresh connection. Returns the full `(Catalog, DriftReport)`.
async fn start_and_read(
    version: PgVersion,
    setup_sql: &str,
) -> Result<(
    pgevolve_core::ir::catalog::Catalog,
    pgevolve_core::catalog::DriftReport,
)> {
    let pg = EphemeralPostgres::start(version)
        .await
        .context("start ephemeral postgres")?;
    pg.exec_sql(setup_sql).await.context("exec setup SQL")?;
    let client = pg.connect().await.context("connect for catalog read")?;
    let querier = PgCatalogQuerier::new(client).map_err(|e| anyhow!(e))?;
    // Subscriptions are database-global — the schema list only matters for
    // table/sequence/etc. queries; we need at least one entry so CatalogFilter
    // does not error.
    let managed = vec![Identifier::from_unquoted("public").map_err(|e| anyhow!(e))?];
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    let catalog = catalog.canonicalize().map_err(|e| anyhow!(e.to_string()))?;
    Ok((catalog, drift))
}

/// Build a `CREATE SUBSCRIPTION` statement that does NOT require a live
/// publisher, adapted to the PG version:
///
///   - PG 16+: `connect = false` — row is created without any network I/O.
///   - PG 14/15: Use the container's own loopback connection string plus
///     `enabled = false, create_slot = false`; still connects but to itself.
///
/// The `dsn` argument is the DSN of the ephemeral Postgres (used on PG 14/15
/// as the self-subscription target).
fn create_sub_sql(name: &str, publications: &[&str], dsn: &str, version: PgVersion) -> String {
    let pubs = publications.join(", ");
    if version.major() >= 16 {
        // PG 16+: connect = false skips all network activity.
        format!(
            "CREATE SUBSCRIPTION {name} \
             CONNECTION 'host=127.0.0.1 port=5433 dbname=publisher_db user=rep' \
             PUBLICATION {pubs} \
             WITH (connect = false);"
        )
    } else {
        // PG 14/15: subscribe to the same server (self-subscription) with
        // enabled=false + create_slot=false to avoid slot creation.
        format!(
            "CREATE SUBSCRIPTION {name} \
             CONNECTION '{dsn}' \
             PUBLICATION {pubs} \
             WITH (enabled = false, create_slot = false, copy_data = false);"
        )
    }
}

// ---------------------------------------------------------------------------
// Test 1: basic subscription round-trip
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_subscription_basic() {
    if !docker_available() {
        eprintln!("skipping read_subscription_basic: Docker not available");
        return;
    }

    let version = default_pg_version();

    // PG 14/15 lack the `connect = false` subscription option (added PG 16+).
    // Without it, CREATE SUBSCRIPTION unconditionally tries to connect to
    // the publisher even with `enabled = false, create_slot = false,
    // copy_data = false`. A self-subscription using the host-mapped DSN
    // fails inside the container (the publisher hostname isn't routable
    // from inside the ephemeral PG). The catalog-reader code paths for
    // PG 14/15 are exercised at the unit-test layer (catalog::subscriptions
    // tests); the integration round-trip only needs to verify the
    // serialization shape against a real pg_subscription, which is the same
    // on every supported PG major. Skip on PG < 16.
    if version.major() < 16 {
        eprintln!(
            "skipping read_subscription_basic on PG {}: connect=false requires PG 16+",
            version.major(),
        );
        return;
    }

    let pg = EphemeralPostgres::start(version)
        .await
        .expect("start ephemeral postgres");
    let dsn = pg.dsn().to_string();

    let setup_sql = create_sub_sql("test_sub", &["pub_one"], &dsn, version);

    pg.exec_sql(&setup_sql).await.expect("exec setup SQL");

    let client = pg.connect().await.expect("connect for catalog read");
    let querier = PgCatalogQuerier::new(client).expect("build querier");
    let managed = vec![Identifier::from_unquoted("public").unwrap()];
    let filter = CatalogFilter::new(managed, vec![]).expect("build filter");
    let (catalog, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .expect("spawn_blocking")
        .expect("read_catalog");
    let catalog = catalog.canonicalize().expect("canonicalize");

    assert!(
        !drift.unreadable_subscriptions,
        "superuser connection should read pg_subscription without privilege error"
    );

    assert_eq!(
        catalog.subscriptions.len(),
        1,
        "expected exactly one subscription"
    );
    let sub = &catalog.subscriptions[0];

    assert_eq!(sub.name.as_str(), "test_sub");

    assert_eq!(sub.publications.len(), 1);
    assert_eq!(sub.publications[0].as_str(), "pub_one");

    // With `connect = false` (PG 16+), PG sets `subenabled = false` by
    // default (per PG docs: connect=false changes the default of enabled to
    // false). On PG 14/15 the self-subscription sets enabled=false explicitly.
    assert_eq!(sub.options.enabled, Some(false));

    // CREATE-time-only options are never stored in pg_subscription.
    assert_eq!(
        sub.options.create_slot, None,
        "create_slot is not stored in pg_subscription"
    );
    assert_eq!(
        sub.options.copy_data, None,
        "copy_data is not stored in pg_subscription"
    );
}

// ---------------------------------------------------------------------------
// Test 2: multiple publications in one subscription (PG 16+ only)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_subscription_multiple_publications() {
    if !docker_available() {
        eprintln!("skipping read_subscription_multiple_publications: Docker not available");
        return;
    }

    // connect = false is PG 16+; skip on older versions.
    if default_pg_version().major() < 16 {
        eprintln!(
            "skipping read_subscription_multiple_publications: \
             requires PG 16+ for connect = false"
        );
        return;
    }

    let (cat, _drift) = start_and_read(
        default_pg_version(),
        "CREATE SUBSCRIPTION multi_pub_sub \
             CONNECTION 'host=127.0.0.1 port=5433 dbname=publisher_db user=rep' \
             PUBLICATION pub_alpha, pub_beta, pub_gamma \
             WITH (connect = false);",
    )
    .await
    .expect("read catalog");

    assert_eq!(cat.subscriptions.len(), 1);
    let sub = &cat.subscriptions[0];
    assert_eq!(sub.name.as_str(), "multi_pub_sub");

    // Canon sorts publications alphabetically.
    assert_eq!(sub.publications.len(), 3);
    let pub_names: Vec<&str> = sub.publications.iter().map(Identifier::as_str).collect();
    assert_eq!(pub_names, vec!["pub_alpha", "pub_beta", "pub_gamma"]);
}
