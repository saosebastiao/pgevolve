//! Live-Postgres name-fidelity tests for `CREATE TABLE (LIKE … INCLUDING …)`.
//!
//! Each test spins up an ephemeral Postgres container, applies LIKE DDL, reads
//! the catalog back, and simultaneously parses the same DDL through
//! `parse_directory`. The sorted sets of constraint names and index names on the
//! cloned table must be identical — proving that pgevolve's name-generation
//! logic (including `choose_name` truncation) matches what a real Postgres
//! assigns when it executes the literal LIKE syntax.
//!
//! Skipped when Docker is unavailable or `PGEVOLVE_DISABLE_DOCKER_TESTS` is set.
//! Pattern mirrors `catalog_reloptions.rs` and `dump_round_trip.rs`.
//!
//! CI runs these via the `conformance (tier 3 + tier C)` job with
//! `PGEVOLVE_TEST_PG_VERSION` set per Postgres major version (14–18).

use anyhow::{Context, Result, anyhow};
use pgevolve_core::catalog::{CatalogFilter, PgVersion, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::index::IndexParent;
use pgevolve_core::parse::parse_directory;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Apply `sql` to a fresh ephemeral Postgres, introspect, and return a
/// canonicalized [`Catalog`] filtered to the `app` schema.
async fn live_catalog_for(sql: &str, version: PgVersion) -> Result<Catalog> {
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

/// Write `sql` (prefixed with `-- @pgevolve schema=app`) into a tempdir as
/// `app/objects.sql`, run `parse_directory`, and return the canonicalized
/// [`Catalog`].
///
/// The SQL is written as-is after the required directive header, so it must
/// only contain statements that `parse_directory` can handle.
fn parsed_catalog_for(sql: &str) -> Result<Catalog> {
    let tmp = tempfile::tempdir().context("create tempdir")?;
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("app")).context("create app dir")?;
    // The schema declaration and all object DDL live in one file so that
    // inter-object references (FK, LIKE) resolve within a single parse pass.
    let file_contents = format!("-- @pgevolve schema=app\n{sql}");
    std::fs::write(dir.join("app/objects.sql"), &file_contents).context("write objects.sql")?;
    parse_directory(dir, &[]).map_err(|e| anyhow!("parse_directory: {e}"))
    // `parse_directory` already canonicalizes; no second call needed.
}

/// Sorted list of constraint names on the table whose `name` component (not
/// schema) equals `table_name`.
fn constraint_names(catalog: &Catalog, table_name: &str) -> Vec<String> {
    let mut names: Vec<String> = catalog
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == table_name)
        .map(|t| {
            t.constraints
                .iter()
                .map(|c| c.qname.name.as_str().to_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Sorted list of index names (from `catalog.indexes`) whose parent table
/// `name` component equals `table_name`.
fn index_names(catalog: &Catalog, table_name: &str) -> Vec<String> {
    let mut names: Vec<String> = catalog
        .indexes
        .iter()
        .filter(|i| matches!(&i.on, IndexParent::Table(q) if q.name.as_str() == table_name))
        .map(|i| i.qname.name.as_str().to_owned())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// Test 1 — basic LIKE INCLUDING ALL: constraint + index names
// ---------------------------------------------------------------------------
//
// SQL applied:
//
//   CREATE SCHEMA app;
//   CREATE TABLE app.base (
//       id    bigint NOT NULL,
//       email text,
//       data  text,
//       PRIMARY KEY (id),
//       UNIQUE (email),
//       CONSTRAINT base_id_chk CHECK (id > 0)
//   );
//   CREATE INDEX ON app.base (data);
//   CREATE TABLE app.clone (LIKE app.base INCLUDING ALL);
//
// Expected names on `app.clone` assigned by Postgres:
//   Constraints (sorted):
//     base_id_chk        — explicitly-named CHECK, preserved by LIKE
//     clone_email_key    — unnamed UNIQUE → {clone}_{col}_key
//     clone_pkey         — unnamed PK     → {clone}_pkey
//   Indexes (sorted):
//     clone_data_idx     — unnamed index  → {clone}_{col}_idx
//
// Note: only the *explicitly-named* CHECK `base_id_chk` is included here.
// pgevolve's unnamed-CHECK naming `{table}_check` is a known mismatch vs
// Postgres's `{table}_{col}_check` and is tracked separately — including an
// unnamed CHECK would be a known-failing case, not what this test verifies.

const LIKE_INCLUDING_ALL_SQL: &str = r"
CREATE SCHEMA app;
CREATE TABLE app.base (
    id    bigint NOT NULL,
    email text,
    data  text,
    PRIMARY KEY (id),
    UNIQUE (email),
    CONSTRAINT base_id_chk CHECK (id > 0)
);
CREATE INDEX ON app.base (data);
CREATE TABLE app.clone (LIKE app.base INCLUDING ALL);
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn like_including_all_names_match_live_pg() {
    if !docker_available() {
        eprintln!("skipping like_including_all_names_match_live_pg: Docker not available");
        return;
    }

    let version = default_pg_version();

    // Live catalog — real Postgres assigns the names.
    let live = live_catalog_for(LIKE_INCLUDING_ALL_SQL, version)
        .await
        .expect("live catalog");

    // Parsed catalog — pgevolve generates the names from source SQL.
    let parsed = parsed_catalog_for(LIKE_INCLUDING_ALL_SQL).expect("parsed catalog");

    // --- constraint names ---
    let live_constraints = constraint_names(&live, "clone");
    let parsed_constraints = constraint_names(&parsed, "clone");
    assert_eq!(
        live_constraints, parsed_constraints,
        "constraint name sets for app.clone must match live Postgres\n  live:   {live_constraints:?}\n  parsed: {parsed_constraints:?}",
    );

    // --- index names ---
    let live_indexes = index_names(&live, "clone");
    let parsed_indexes = index_names(&parsed, "clone");
    assert_eq!(
        live_indexes, parsed_indexes,
        "index name sets for app.clone must match live Postgres\n  live:   {live_indexes:?}\n  parsed: {parsed_indexes:?}",
    );

    eprintln!(
        "like_including_all_names_match_live_pg [{version:?}] passed — constraints: {live_constraints:?}, indexes: {live_indexes:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — LIKE INCLUDING INDEXES with long names: truncation fidelity
// ---------------------------------------------------------------------------
//
// This test exercises the `choose_name` NAMEDATALEN truncation path by using
// table names long enough that the derived index name for the cloned table
// exceeds 63 bytes (Postgres's NAMEDATALEN - 1).
//
// Name-length math (all ASCII, 1 byte/char):
//   clone table: "a_very_long_clone_table_name_for_truncation_testing_yyyyyyy"
//                 ↳ 59 bytes
//   would-be index: "{clone}_a_b_idx"
//                 = "a_very_long_clone_table_name_for_truncation_testing_yyyyyyy_a_b_idx"
//                 ↳ 67 bytes  > 63  → truncation required
//
// Source table: "a_very_long_source_table_name_for_truncation_testing_xxxx"
//               ↳ 57 bytes
//
// Postgres truncates the name1 (table) component to make the whole name fit
// within NAMEDATALEN. pgevolve's `choose_name` must implement the same
// algorithm. If CI surfaces a mismatch here, it indicates a real truncation
// bug in `choose_name` that must be fixed.

const LIKE_TRUNCATION_SQL: &str = r"
CREATE SCHEMA app;
CREATE TABLE app.a_very_long_source_table_name_for_truncation_testing_xxxx (a int, b int);
CREATE INDEX ON app.a_very_long_source_table_name_for_truncation_testing_xxxx (a, b);
CREATE TABLE app.a_very_long_clone_table_name_for_truncation_testing_yyyyyyy
    (LIKE app.a_very_long_source_table_name_for_truncation_testing_xxxx INCLUDING INDEXES);
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn like_index_name_truncation_matches_live_pg() {
    if !docker_available() {
        eprintln!("skipping like_index_name_truncation_matches_live_pg: Docker not available");
        return;
    }

    // Verify our byte-length assumptions at runtime so a rename doesn't
    // silently invalidate the test.
    let clone_name = "a_very_long_clone_table_name_for_truncation_testing_yyyyyyy";
    let would_be = format!("{clone_name}_a_b_idx");
    assert!(
        would_be.len() > 63,
        "test setup: would-be index name must exceed 63 bytes (got {})",
        would_be.len()
    );

    let version = default_pg_version();

    let live = live_catalog_for(LIKE_TRUNCATION_SQL, version)
        .await
        .expect("live catalog");

    let parsed = parsed_catalog_for(LIKE_TRUNCATION_SQL).expect("parsed catalog");

    let live_indexes = index_names(&live, clone_name);
    let parsed_indexes = index_names(&parsed, clone_name);
    assert_eq!(
        live_indexes,
        parsed_indexes,
        "truncated index name sets for the clone table must match live Postgres\n  live:   {live_indexes:?}\n  parsed: {parsed_indexes:?}\n  (would-be untruncated: {would_be:?}, {} bytes)",
        would_be.len(),
    );

    eprintln!(
        "like_index_name_truncation_matches_live_pg [{version:?}] passed — indexes: {live_indexes:?}"
    );
}
