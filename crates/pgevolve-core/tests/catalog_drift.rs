//! Catalog drift detection — NOT VALID constraints and INVALID indexes.
//!
//! Docker-gated: only runs when an ephemeral Postgres can boot. Skipped
//! (not failed) when Docker is unavailable.

use pgevolve_core::catalog::{CatalogFilter, DriftReport, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;
use pgevolve_core::catalog::PgVersion;

/// Build a `CatalogFilter` that manages the `app` schema.
fn app_filter() -> CatalogFilter {
    CatalogFilter::new(
        vec![Identifier::from_unquoted("app").expect("valid")],
        vec![],
    )
    .expect("filter")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_valid_constraint_surfaces_as_pending_validation() {
    if !docker_available() {
        eprintln!("skipping catalog_drift::not_valid_constraint: Docker unavailable");
        return;
    }

    let pg = EphemeralPostgres::start(PgVersion::Pg17)
        .await
        .expect("start PG17");

    pg.exec_sql(
        "CREATE SCHEMA app;
         CREATE TABLE app.users (id bigint PRIMARY KEY);
         CREATE TABLE app.orders (
             id bigint PRIMARY KEY,
             user_id bigint NOT NULL
         );
         ALTER TABLE app.orders
             ADD CONSTRAINT fk_user
             FOREIGN KEY (user_id) REFERENCES app.users (id)
             NOT VALID;",
    )
    .await
    .expect("setup");

    let client = pg.connect().await.expect("connect");
    let querier = PgCatalogQuerier::new(client).expect("querier");
    let filter = app_filter();

    let (_catalog, drift) =
        tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
            .await
            .expect("join")
            .expect("read_catalog");

    assert!(
        drift
            .pending_validation
            .iter()
            .any(|(_, name)| name.as_str() == "fk_user"),
        "drift.pending_validation should mention fk_user; got {drift:?}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_index_surfaces_as_invalid_indexes() {
    if !docker_available() {
        eprintln!("skipping catalog_drift::invalid_index: Docker unavailable");
        return;
    }

    let pg = EphemeralPostgres::start(PgVersion::Pg17)
        .await
        .expect("start PG17");

    pg.exec_sql(
        "CREATE SCHEMA app;
         CREATE TABLE app.users (id bigint PRIMARY KEY, email text);
         CREATE INDEX users_email_idx ON app.users (email);",
    )
    .await
    .expect("setup tables and index");

    // Mark the index as INVALID directly in pg_index. Only superuser can do
    // this; the ephemeral container runs as the postgres superuser.
    pg.exec_sql(
        "UPDATE pg_index SET indisvalid = false
             WHERE indexrelid = 'app.users_email_idx'::regclass;",
    )
    .await
    .expect("mark index invalid");

    let client = pg.connect().await.expect("connect");
    let querier = PgCatalogQuerier::new(client).expect("querier");
    let filter = app_filter();

    let (_catalog, drift) =
        tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
            .await
            .expect("join")
            .expect("read_catalog");

    assert!(
        drift
            .invalid_indexes
            .iter()
            .any(|q| q.name.as_str() == "users_email_idx"),
        "drift.invalid_indexes should mention users_email_idx; got {drift:?}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validated_constraint_does_not_appear_in_drift() {
    if !docker_available() {
        eprintln!("skipping catalog_drift::validated_constraint: Docker unavailable");
        return;
    }

    let pg = EphemeralPostgres::start(PgVersion::Pg17)
        .await
        .expect("start PG17");

    pg.exec_sql(
        "CREATE SCHEMA app;
         CREATE TABLE app.users (id bigint PRIMARY KEY);
         CREATE TABLE app.orders (
             id bigint PRIMARY KEY,
             user_id bigint NOT NULL,
             CONSTRAINT fk_user FOREIGN KEY (user_id) REFERENCES app.users (id)
         );",
    )
    .await
    .expect("setup");

    let client = pg.connect().await.expect("connect");
    let querier = PgCatalogQuerier::new(client).expect("querier");
    let filter = app_filter();

    let (_catalog, drift) =
        tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
            .await
            .expect("join")
            .expect("read_catalog");

    assert!(
        drift.pending_validation.is_empty(),
        "fully-validated constraint should not appear in drift; got {drift:?}",
    );
}

#[test]
fn drift_report_default_is_empty() {
    let drift = DriftReport::default();
    assert!(drift.pending_validation.is_empty());
    assert!(drift.invalid_indexes.is_empty());
}
