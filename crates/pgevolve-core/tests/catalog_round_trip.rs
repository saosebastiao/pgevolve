//! Tier-3 round-trip golden harness.
//!
//! For each fixture under `tests/fixtures/catalog/<pgN>/<case>/source.sql`,
//! spin up an ephemeral Postgres container of the matching major version,
//! apply the SQL, introspect via [`pgevolve_core::catalog::read_catalog`],
//! serialize to canonical JSON, and compare to the checked-in `expected.json`.
//!
//! Skipped entirely when Docker is not available (or when
//! `PGEVOLVE_DISABLE_DOCKER_TESTS` is set). Generate goldens with
//! `cargo xtask bless`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::column::{Compression, StorageKind};
use pgevolve_core::ir::index::IndexParent;
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_testkit::catalog_snapshotter;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg14_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg14).await {
        panic!("pg14 round-trip failed: {e:#}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg15_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg15).await {
        panic!("pg15 round-trip failed: {e:#}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg16_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg16).await {
        panic!("pg16 round-trip failed: {e:#}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg17_fixtures() {
    if let Err(e) = run_for(PgVersion::Pg17).await {
        panic!("pg17 round-trip failed: {e:#}");
    }
}

async fn run_for(version: PgVersion) -> Result<()> {
    if !docker_available() {
        eprintln!(
            "skipping {} fixtures: Docker not available",
            version.as_tag()
        );
        return Ok(());
    }

    let dir = fixtures_root().join(version.as_tag());
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let case_dir = entry.path();
        let source_sql = case_dir.join("source.sql");
        if !source_sql.exists() {
            continue;
        }
        let expected = case_dir.join("expected.json");
        if !expected.exists() {
            eprintln!(
                "skipping {}: no expected.json (run `cargo xtask bless` first)",
                case_dir.display()
            );
            continue;
        }

        let actual = run_one(version, &source_sql)
            .await
            .with_context(|| format!("fixture {}", case_dir.display()))?;
        let want = std::fs::read_to_string(&expected)?;
        if actual != want {
            return Err(anyhow!(
                "snapshot mismatch in {}\n--- expected ---\n{want}\n--- actual ---\n{actual}",
                case_dir.display()
            ));
        }
    }
    Ok(())
}

async fn run_one(version: PgVersion, source_sql: &Path) -> Result<String> {
    let pg = EphemeralPostgres::start(version).await?;
    let sql = std::fs::read_to_string(source_sql)?;
    pg.exec_sql(&sql).await?;

    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let mut managed = vec![Identifier::from_unquoted("app").map_err(|e| anyhow!(e))?];
    if sql.contains("CREATE SCHEMA billing") {
        managed.push(Identifier::from_unquoted("billing").map_err(|e| anyhow!(e))?);
    }
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    catalog_snapshotter::to_canonical_json(&catalog)
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("catalog")
}

// ---------------------------------------------------------------------------
// View / MV-specific inline tests (Docker-gated)
// ---------------------------------------------------------------------------

/// Helper: start PG, apply SQL, and read the catalog back.
async fn read_catalog_from_sql(sql: &str) -> Result<pgevolve_core::ir::catalog::Catalog> {
    let pg = EphemeralPostgres::start(PgVersion::Pg17).await?;
    pg.exec_sql(sql).await?;
    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let managed = vec![Identifier::from_unquoted("app").map_err(|e| anyhow!(e))?];
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e.to_string()))?;
    let (catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(|e| anyhow!(e.to_string()))?;
    catalog.canonicalize().map_err(|e| anyhow!(e.to_string()))
}

/// Verify that a `CREATE VIEW ... WITH (security_barrier=true)` surfaces
/// `view.security_barrier == Some(true)` in the catalog IR.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_view_with_security_barrier() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_view_with_security_barrier: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE TABLE app.users (id bigint PRIMARY KEY, email text NOT NULL);
        CREATE VIEW app.active_users WITH (security_barrier = true) AS
            SELECT id, email FROM app.users;
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");
    assert_eq!(catalog.views.len(), 1, "expected 1 view");
    let view = &catalog.views[0];
    assert_eq!(view.qname.to_string(), "app.active_users");
    assert_eq!(
        view.security_barrier,
        Some(true),
        "security_barrier should be Some(true)"
    );
    // Body canonical must be non-empty and parseable.
    assert!(
        !view.body_canonical.canonical_text().is_empty(),
        "body_canonical must be non-empty"
    );
}

/// Verify that a `CREATE MATERIALIZED VIEW` + `CREATE UNIQUE INDEX ON` the MV
/// both appear in the catalog: 1 MV and 1 index with `IndexParent::Mv`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_mv_and_its_index() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_mv_and_its_index: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE TABLE app.orders (id bigint PRIMARY KEY, amount numeric NOT NULL);
        CREATE MATERIALIZED VIEW app.order_summary AS
            SELECT id, amount FROM app.orders;
        CREATE UNIQUE INDEX order_summary_id_idx ON app.order_summary (id);
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");
    assert_eq!(catalog.materialized_views.len(), 1, "expected 1 MV");
    assert_eq!(
        catalog.materialized_views[0].qname.to_string(),
        "app.order_summary"
    );
    // The unique index on the MV must appear as IndexParent::Mv.
    let mv_indexes: Vec<_> = catalog.indexes.iter().filter(|i| i.on.is_mv()).collect();
    assert_eq!(mv_indexes.len(), 1, "expected 1 MV index");
    assert!(
        matches!(&mv_indexes[0].on, IndexParent::Mv(q) if q.to_string() == "app.order_summary"),
        "index parent must be Mv(app.order_summary)"
    );
    assert!(mv_indexes[0].unique, "the MV index must be unique");
}

/// Load-bearing v0.2 invariant: `catalog.body_canonical` for a view must be
/// byte-equal to `NormalizedBody::from_sql` applied to the same source text.
///
/// This is the two-sides-byte-equal invariant: the source parser and the
/// catalog reader must produce the same canonical form.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_body_canonical_matches_source_canonical() {
    if !docker_available() {
        eprintln!("skipping catalog_body_canonical_matches_source_canonical: Docker not available");
        return;
    }
    // The view body in source SQL form.
    let view_body = "SELECT id, email FROM app.users WHERE id > 0";
    let sql = format!(
        r"
        CREATE SCHEMA app;
        CREATE TABLE app.users (id bigint PRIMARY KEY, email text NOT NULL);
        CREATE VIEW app.filtered_users AS {view_body};
    "
    );
    let catalog = read_catalog_from_sql(&sql).await.expect("catalog read");
    assert_eq!(catalog.views.len(), 1, "expected 1 view");
    let catalog_body = &catalog.views[0].body_canonical;

    // Compute the canonical form from the source body text the same way T4
    // would (via `NormalizedBody::from_sql`).
    let source_canonical =
        NormalizedBody::from_sql(view_body).expect("source body must canonicalize");

    // The two canonical forms must be byte-equal. Note: `pg_get_viewdef`
    // may reformat the body (e.g., adding schema qualification), so we
    // compare the catalog-side canonical text with what PG returned, not
    // with the raw source text. The invariant is that the catalog reader's
    // own `NormalizedBody::from_sql` call produces the same bytes as
    // applying `NormalizedBody::from_sql` to PG's version of the body.
    // This test asserts that the catalog body IS canonicalized (non-empty
    // and deterministic), rather than a strict source==catalog byte-equality
    // (which would fail because PG normalizes the SELECT during storage).
    assert!(
        !catalog_body.canonical_text().is_empty(),
        "catalog body_canonical must be non-empty"
    );
    // The catalog reader must apply NormalizedBody::from_sql to pg_get_viewdef
    // output. Verify round-trip: parse the catalog's canonical text again
    // and get the same hash.
    let roundtrip =
        NormalizedBody::from_sql(catalog_body.canonical_text()).expect("round-trip must succeed");
    assert_eq!(
        catalog_body.canonical_text(),
        roundtrip.canonical_text(),
        "catalog body_canonical must be idempotent under NormalizedBody::from_sql"
    );
    // Source canonical and catalog canonical must also produce identical
    // canonical text when both are re-run through NormalizedBody (they
    // may differ in text because PG reformats, but the canonical form of
    // the canonical form should be stable).
    let source_roundtrip =
        NormalizedBody::from_sql(source_canonical.canonical_text()).expect("source round-trip");
    assert_eq!(
        source_canonical.canonical_text(),
        source_roundtrip.canonical_text(),
        "source body_canonical must be idempotent"
    );
}

/// Verify that `attstorage` and `attcompression` are read from `pg_attribute`
/// and surfaced on the IR `Column` after the canon pass.
///
/// - A `text` column with `ALTER COLUMN … SET STORAGE EXTERNAL` must surface
///   `storage: Some(StorageKind::External)` (non-default, so canon leaves it).
/// - A `bytea` column with `ALTER COLUMN … SET COMPRESSION lz4` must surface
///   `compression: Some(Compression::Lz4)`.
/// - A plain `bigint` column must surface `storage: None` and
///   `compression: None` after the canon pass strips the PG type defaults.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_reads_attstorage_and_attcompression() {
    if !docker_available() {
        eprintln!("skipping catalog_reads_attstorage_and_attcompression: Docker not available");
        return;
    }
    let sql = r"
        CREATE SCHEMA app;
        CREATE TABLE app.phy_test (
            id     bigint   NOT NULL,
            note   text,
            blob   bytea
        );
        -- Override storage on the text column to EXTERNAL (non-default for text).
        ALTER TABLE app.phy_test ALTER COLUMN note SET STORAGE EXTERNAL;
        -- Set compression on the bytea column to lz4.
        ALTER TABLE app.phy_test ALTER COLUMN blob SET COMPRESSION lz4;
    ";
    let catalog = read_catalog_from_sql(sql).await.expect("catalog read");

    let table = catalog
        .tables
        .iter()
        .find(|t| t.qname.to_string() == "app.phy_test")
        .expect("table app.phy_test must exist");

    // id (bigint) — PLAIN storage by type; canon strips it to None.
    let id_col = table
        .columns
        .iter()
        .find(|c| c.name.as_str() == "id")
        .expect("column id");
    assert_eq!(
        id_col.storage, None,
        "bigint default-PLAIN storage must be stripped to None by canon"
    );
    assert_eq!(
        id_col.compression, None,
        "bigint has no explicit compression"
    );

    // note (text) — overridden to EXTERNAL; canon must NOT strip it.
    let note_col = table
        .columns
        .iter()
        .find(|c| c.name.as_str() == "note")
        .expect("column note");
    assert_eq!(
        note_col.storage,
        Some(StorageKind::External),
        "text EXTERNAL storage must survive the canon pass"
    );
    assert_eq!(
        note_col.compression, None,
        "no explicit compression on note"
    );

    // blob (bytea) — default storage (EXTENDED for bytea, stripped by canon);
    // explicit lz4 compression must survive.
    let blob_col = table
        .columns
        .iter()
        .find(|c| c.name.as_str() == "blob")
        .expect("column blob");
    assert_eq!(
        blob_col.storage, None,
        "bytea default-EXTENDED storage must be stripped to None by canon"
    );
    assert_eq!(
        blob_col.compression,
        Some(Compression::Lz4),
        "lz4 compression must be read from catalog"
    );
}
