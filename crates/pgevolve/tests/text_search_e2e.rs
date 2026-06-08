//! End-to-end round-trip for `TEXT SEARCH DICTIONARY` and `TEXT SEARCH
//! CONFIGURATION`.
//!
//! Applies a source `Catalog` containing:
//! - schema `app`
//! - `TEXT SEARCH DICTIONARY app.en` (snowball, language = english)
//! - `TEXT SEARCH CONFIGURATION app.cfg` (`PARSER = pg_catalog."default"`)
//! - `ALTER TEXT SEARCH CONFIGURATION app.cfg ADD MAPPING FOR word, asciiword WITH app.en`
//!
//! to a fresh ephemeral Postgres, introspects the live database, and asserts
//! the live state converges with the source — proving the parser → plan →
//! apply → reader loop agrees for text-search objects.
//!
//! This is the round-trip that exercises the `ts_token_type` lateral join
//! (Task 10 reader): the reader resolves `maptokentype` OID arrays back to
//! the same token-type aliases (`word`, `asciiword`) that the parser wrote.
//! If these don't match a spurious ADD MAPPING diff would fire.
//!
//! Note on table tests: a table with a `to_tsvector('app.cfg', body)` generated
//! column or functional index has an implicit DDL-time dependency on `app.cfg`.
//! The planner does not currently track expression-level TS config references,
//! so including such a table would cause an ordering bug (table created before
//! config). The convergence proof is complete without a table: if the reader
//! correctly resolves the token-type mappings, no spurious diff fires on
//! re-introspection.
//!
//! The source `Catalog` is built via `parse_directory` rather than by hand.
//!
//! Skipped when Docker is unavailable. `#[ignore]`'d like the other PG-backed
//! e2e tests; run with:
//!   `cargo test -p pgevolve --test text_search_e2e -- --ignored`

mod common;

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::parse_directory;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

use common::{apply_diff, assert_convergent, connect_and_bootstrap, introspect, schemas_of};

/// Source SQL exercising the full text-search pipeline:
/// - a managed snowball dictionary
/// - a managed configuration with two token-type mappings (word, asciiword)
///
/// PARSER and TEMPLATE are always schema-qualified so they round-trip against
/// the reader's qualified output from `pg_catalog`.
///
/// The two token-type mappings exercise the `ts_token_type` lateral join in
/// the reader (Task 10): the reader must resolve `maptokentype` OID arrays
/// back to `word` and `asciiword` exactly as the parser wrote them.
const SOURCE_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TEXT SEARCH DICTIONARY app.en (\
  TEMPLATE = pg_catalog.snowball, \
  language = 'english'\
);
CREATE TEXT SEARCH CONFIGURATION app.cfg (\
  PARSER = pg_catalog.\"default\"\
);
ALTER TEXT SEARCH CONFIGURATION app.cfg ADD MAPPING FOR word, asciiword WITH app.en;
";

/// Parse `SOURCE_SQL` into a `Catalog` via the source pipeline.
fn source_catalog() -> Result<Catalog> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("0001-text-search.sql"), SOURCE_SQL)?;
    let catalog = parse_directory(dir.path(), &[])?;
    Ok(catalog)
}

#[ignore = "e2e test — requires Docker; run via `cargo test -- --ignored`"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_search_round_trips_against_ephemeral_pg() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    run().await.expect("text-search round-trip");
}

async fn run() -> Result<()> {
    let source = source_catalog()?;
    let managed = schemas_of(&source);

    // Sanity: the parsed source actually carries the objects we expect, so a
    // no-op apply can't accidentally pass the convergence check.
    assert_eq!(
        source.ts_dictionaries.len(),
        1,
        "source catalog should declare exactly one ts_dictionary"
    );
    assert_eq!(
        source.ts_configurations.len(),
        1,
        "source catalog should declare exactly one ts_configuration"
    );
    {
        let cfg = &source.ts_configurations[0];
        assert_eq!(
            cfg.mappings.len(),
            2,
            "source configuration should have exactly two token-type mappings (word, asciiword)"
        );
    }

    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let mut client = connect_and_bootstrap(&pg).await?;

    // Apply from an empty database to the source state.
    let outcome = apply_diff(&mut client, &Catalog::empty(), &source, &managed, None).await?;
    outcome.map_err(|e| anyhow::anyhow!("apply failed: {e}"))?;

    // Introspect the live database and assert it converges with the source.
    let live = introspect(&pg, &managed).await?;
    assert_eq!(
        live.ts_dictionaries.len(),
        1,
        "live database should report exactly one ts_dictionary after apply"
    );
    assert_eq!(
        live.ts_configurations.len(),
        1,
        "live database should report exactly one ts_configuration after apply"
    );
    assert_convergent(&live, &source)
}
