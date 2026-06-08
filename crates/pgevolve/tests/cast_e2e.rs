//! End-to-end round-trip for `CREATE CAST`.
//!
//! Applies a source `Catalog` containing a schema, a managed domain, a managed
//! plpgsql conversion function, and a `CAST` over them (WITH FUNCTION) to a
//! fresh ephemeral Postgres, introspects the live database, and asserts the live
//! state converges with the source — proving the parser → plan → apply → reader
//! loop agrees for casts.
//!
//! This is the round-trip that proves the cast's source/target type qualifications,
//! the conversion function's qname and argument types, and the cast context all
//! survive the parser ↔ reader boundary (the reader resolves types from
//! `pg_cast` / `pg_proc` OIDs via `pg_get_function_identity_arguments`; the
//! parser resolves them from SQL text — they must agree).
//!
//! The source `Catalog` is built via the project's SQL parse entry point
//! (`parse_directory`) rather than by hand: constructing a valid `Cast` value
//! (with normalized source/target qnames and resolved conversion function arg
//! types) by hand is fiddly, and parsing the canonical source SQL exercises the
//! same source pipeline the conformance fixtures use.
//!
//! Skipped when Docker is unavailable. `#[ignore]`'d like the other PG-backed
//! property / e2e tests; run with:
//!   `cargo test -p pgevolve --test cast_e2e -- --ignored`

mod common;

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::parse_directory;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

use common::{apply_diff, assert_convergent, connect_and_bootstrap, introspect, schemas_of};

/// Source SQL declaring a schema, a managed domain, a managed plpgsql
/// conversion function, and a cast WITH FUNCTION over them.
///
/// The domain `app.celsius` is over `numeric`. The conversion function
/// `app.celsius_to_text(app.celsius)` takes the domain type as its argument
/// and returns `text`. The cast is explicit (default context).
///
/// This exercises the parser ↔ reader round-trip for:
/// - source type: `app.celsius` (user-defined domain)
/// - target type: `pg_catalog.text` (built-in, schema-qualified by pg_query)
/// - cast function arg type: `app.celsius` (domain, decoded via
///   `pg_get_function_identity_arguments`)
///
/// Note: the function body uses only ASCII characters to avoid encoding
/// discrepancies when Postgres is started in a non-UTF8 locale.
const SOURCE_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.celsius AS numeric;
CREATE FUNCTION app.celsius_to_text(app.celsius) RETURNS text \
  LANGUAGE plpgsql AS $$ BEGIN RETURN $1::text || 'C'; END $$;
CREATE CAST (app.celsius AS pg_catalog.text) WITH FUNCTION app.celsius_to_text(app.celsius);
";

/// Parse `SOURCE_SQL` into a `Catalog` via the source pipeline.
fn source_catalog() -> Result<Catalog> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("0001-cast.sql"), SOURCE_SQL)?;
    let catalog = parse_directory(dir.path(), &[])?;
    Ok(catalog)
}

#[ignore = "e2e test — requires Docker; run via `cargo test -- --ignored`"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cast_round_trips_against_ephemeral_pg() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    run().await.expect("cast round-trip");
}

async fn run() -> Result<()> {
    let source = source_catalog()?;
    let managed = schemas_of(&source);

    // Sanity: the parsed source actually carries the cast we expect, so a
    // no-op apply can't accidentally pass the convergence check.
    assert_eq!(
        source.casts.len(),
        1,
        "source catalog should declare exactly one cast"
    );

    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let mut client = connect_and_bootstrap(&pg).await?;

    // Apply from an empty database to the source state.
    let outcome = apply_diff(&mut client, &Catalog::empty(), &source, &managed, None).await?;
    outcome.map_err(|e| anyhow::anyhow!("apply failed: {e}"))?;

    // Introspect the live database and assert it converges with the source.
    let live = introspect(&pg, &managed).await?;
    assert_eq!(
        live.casts.len(),
        1,
        "live database should report exactly one cast after apply"
    );
    assert_convergent(&live, &source)
}
