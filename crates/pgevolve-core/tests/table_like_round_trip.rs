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

/// Sorted list of CHECK constraint names on the table whose `name` component equals `table_name`.
fn check_constraint_names(catalog: &Catalog, table_name: &str) -> Vec<String> {
    let mut names: Vec<String> = catalog
        .tables
        .iter()
        .find(|t| t.qname.name.as_str() == table_name)
        .map(|t| {
            t.constraints
                .iter()
                .filter(|c| {
                    matches!(
                        c.kind,
                        pgevolve_core::ir::constraint::ConstraintKind::Check { .. }
                    )
                })
                .map(|c| c.qname.name.as_str().to_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Sorted list of extended-statistics object names (from `catalog.statistics`)
/// whose target table `name` component equals `table_name`.
fn statistic_names(catalog: &Catalog, table_name: &str) -> Vec<String> {
    let mut names: Vec<String> = catalog
        .statistics
        .iter()
        .filter(|s| s.target.name.as_str() == table_name)
        .map(|s| s.qname.name.as_str().to_owned())
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

// ---------------------------------------------------------------------------
// Test 4 — LIKE INCLUDING INDEXES: long column name forces name2 truncation
// ---------------------------------------------------------------------------
//
// This test exercises the `name2` truncation path added by #49, where the
// column-name addition (not the table name) is the long component.  PG's
// `makeObjectName` alternating-shrink algorithm must trim name2 when name1
// alone cannot absorb all the excess.
//
// Name-length math (all ASCII, 1 byte/char):
//   source col: "very_long_column_name_that_forces_name2_truncation_xxxxxxxxxx"
//               ↳ 61 bytes
//   clone table: "c"  (1 byte, deliberately short so name1 can't absorb all)
//   would-be index: "c_<61-char-col>_idx"
//                 = 1 + 1 + 61 + 1 + 3 = 67 bytes  > 63  → truncation required
//
//   overhead = 1("_" before name2) + 1("_" before label) + 3("idx") = 5
//   availchars = 63 - 5 = 58
//   n1=1, n2=61; sum=62 > 58; must shrink by 4.
//   n2 > n1 every step → all 4 reductions go to n2 → n2=57.
//   Final: "c_" + col[..57] + "_idx" = 1+1+57+1+3 = 63 bytes.
//
// Old (buggy) algorithm only truncated name1, leaving "c" + "_" + (61-char
// col) + "_idx" = 67 bytes, which exceeds NAMEDATALEN.

const LIKE_LONG_COL_SQL: &str = r"
CREATE SCHEMA app;
CREATE TABLE app.src (
    very_long_column_name_that_forces_name2_truncation_xxxxxxxxxx int
);
CREATE INDEX ON app.src (very_long_column_name_that_forces_name2_truncation_xxxxxxxxxx);
CREATE TABLE app.c (LIKE app.src INCLUDING INDEXES);
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn like_long_column_index_name_matches_live_pg() {
    if !docker_available() {
        eprintln!("skipping like_long_column_index_name_matches_live_pg: Docker not available");
        return;
    }

    // Verify our byte-length assumptions at runtime so a rename doesn't
    // silently invalidate the test.
    let col_name = "very_long_column_name_that_forces_name2_truncation_xxxxxxxxxx";
    let clone_name = "c";
    let would_be = format!("{clone_name}_{col_name}_idx");
    assert!(
        would_be.len() > 63,
        "test setup: would-be index name must exceed 63 bytes (got {})",
        would_be.len()
    );
    // Also assert col is the long part (> 58 = availchars) so name2 must be truncated.
    assert!(
        col_name.len() > 58,
        "test setup: col name ({} bytes) must exceed availchars (58) to force name2 truncation",
        col_name.len()
    );

    let version = default_pg_version();

    let live = live_catalog_for(LIKE_LONG_COL_SQL, version)
        .await
        .expect("live catalog");

    let parsed = parsed_catalog_for(LIKE_LONG_COL_SQL).expect("parsed catalog");

    let live_indexes = index_names(&live, clone_name);
    let parsed_indexes = index_names(&parsed, clone_name);
    assert_eq!(
        live_indexes,
        parsed_indexes,
        "long-column truncated index name must match live Postgres\n  live:   {live_indexes:?}\n  parsed: {parsed_indexes:?}\n  (would-be untruncated: {would_be:?}, {} bytes)",
        would_be.len(),
    );

    eprintln!(
        "like_long_column_index_name_matches_live_pg [{version:?}] passed — indexes: {live_indexes:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — LIKE INCLUDING ALL: extended-statistics object name
// ---------------------------------------------------------------------------
//
// SQL applied:
//
//   CREATE SCHEMA app;
//   CREATE TABLE app.base (a int, b int);
//   CREATE STATISTICS app.base_stat (ndistinct) ON a, b FROM app.base;
//   CREATE TABLE app.clone (LIKE app.base INCLUDING ALL);
//
// `INCLUDING ALL` copies the extended statistics object onto the clone with a
// freshly-generated name. This verifies that pgevolve's `choose_name`
// `IndexNameKind::Stat` naming (`{table}_{cols}_stat`) matches Postgres's
// `ChooseExtendedStatisticName` for a LIKE-copied stat. If CI surfaces a
// mismatch, that indicates a real `choose_name` Stat-naming bug to fix.

const LIKE_STATISTICS_SQL: &str = r"
CREATE SCHEMA app;
CREATE TABLE app.base (a int, b int);
CREATE STATISTICS app.base_stat (ndistinct) ON a, b FROM app.base;
CREATE TABLE app.clone (LIKE app.base INCLUDING ALL);
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn like_statistics_name_matches_live_pg() {
    if !docker_available() {
        eprintln!("skipping like_statistics_name_matches_live_pg: Docker not available");
        return;
    }

    let version = default_pg_version();

    let live = live_catalog_for(LIKE_STATISTICS_SQL, version)
        .await
        .expect("live catalog");

    let parsed = parsed_catalog_for(LIKE_STATISTICS_SQL).expect("parsed catalog");

    let live_stats = statistic_names(&live, "clone");
    let parsed_stats = statistic_names(&parsed, "clone");

    // Exactly one stat is expected on the clone; asserting this first means a
    // "both empty" pair can't trivially satisfy the equality check below.
    assert_eq!(
        live_stats.len(),
        1,
        "live Postgres must report exactly one extended-statistics object on app.clone, got {live_stats:?}",
    );

    assert_eq!(
        live_stats, parsed_stats,
        "extended-statistics name sets for app.clone must match live Postgres\n  live:   {live_stats:?}\n  parsed: {parsed_stats:?}",
    );

    eprintln!(
        "like_statistics_name_matches_live_pg [{version:?}] passed — statistics: {live_stats:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — unnamed column CHECK: name fidelity vs live PG
// ---------------------------------------------------------------------------

const UNNAMED_COL_CHECK_SQL: &str = r"
CREATE SCHEMA app;
CREATE TABLE app.t (n int, CHECK (n > 0));
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unnamed_column_check_name_matches_live_pg() {
    if !docker_available() {
        eprintln!("skipping unnamed_column_check_name_matches_live_pg: Docker not available");
        return;
    }
    let version = default_pg_version();
    let live = live_catalog_for(UNNAMED_COL_CHECK_SQL, version)
        .await
        .expect("live catalog");
    let parsed = parsed_catalog_for(UNNAMED_COL_CHECK_SQL).expect("parsed catalog");
    let live_checks = check_constraint_names(&live, "t");
    let parsed_checks = check_constraint_names(&parsed, "t");
    assert!(
        !live_checks.is_empty(),
        "live PG must report at least one CHECK constraint on app.t"
    );
    assert_eq!(
        live_checks, parsed_checks,
        "CHECK constraint names for app.t must match live Postgres\n  live:   {live_checks:?}\n  parsed: {parsed_checks:?}",
    );
    eprintln!(
        "unnamed_column_check_name_matches_live_pg [{version:?}] passed — checks: {live_checks:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 6 — LIKE INCLUDING CONSTRAINTS: verbatim CHECK name copy
// ---------------------------------------------------------------------------

const LIKE_UNNAMED_CHECK_SQL: &str = r"
CREATE SCHEMA app;
CREATE TABLE app.base (n int, CHECK (n > 0));
CREATE TABLE app.clone (LIKE app.base INCLUDING CONSTRAINTS);
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn like_unnamed_check_name_matches_live_pg() {
    if !docker_available() {
        eprintln!("skipping like_unnamed_check_name_matches_live_pg: Docker not available");
        return;
    }
    let version = default_pg_version();
    let live = live_catalog_for(LIKE_UNNAMED_CHECK_SQL, version)
        .await
        .expect("live catalog");
    let parsed = parsed_catalog_for(LIKE_UNNAMED_CHECK_SQL).expect("parsed catalog");
    // Verify base table CHECK name matches live PG.
    let live_base = check_constraint_names(&live, "base");
    let parsed_base = check_constraint_names(&parsed, "base");
    assert!(
        !live_base.is_empty(),
        "live PG must report CHECK on app.base"
    );
    assert_eq!(
        live_base, parsed_base,
        "CHECK names for app.base must match live\n  live: {live_base:?}\n  parsed: {parsed_base:?}"
    );
    // Verify clone table CHECK name (verbatim copy from base).
    let live_clone = check_constraint_names(&live, "clone");
    let parsed_clone = check_constraint_names(&parsed, "clone");
    assert!(
        !live_clone.is_empty(),
        "live PG must report CHECK on app.clone"
    );
    assert_eq!(
        live_clone, parsed_clone,
        "CHECK names for app.clone must match live\n  live: {live_clone:?}\n  parsed: {parsed_clone:?}"
    );
    eprintln!(
        "like_unnamed_check_name_matches_live_pg [{version:?}] passed — base: {live_base:?}, clone: {live_clone:?}"
    );
}
