//! End-to-end round-trip for `CREATE AGGREGATE`.
//!
//! Applies a source `Catalog` containing a schema, a managed plpgsql state
//! function, a managed plpgsql final function, and an `AGGREGATE` over them
//! (with an `INITCOND`) to a fresh ephemeral Postgres, introspects the live
//! database, and asserts the live state converges with the source — proving the
//! parser → plan → apply → reader loop agrees for aggregates.
//!
//! This is the round-trip that proves the aggregate's argument types, state
//! type, sfunc / finalfunc qnames, and `INITCOND` all survive the
//! parser ↔ reader boundary (the reader resolves types from `pg_aggregate` /
//! `pg_proc` OIDs; the parser resolves them from SQL text — they must agree).
//!
//! The source `Catalog` is built via the project's SQL parse entry point
//! (`parse_directory`) rather than by hand: constructing a valid `Aggregate`
//! value (with normalized arg / state types and resolved sfunc overloads) by
//! hand is fiddly, and parsing the canonical source SQL exercises the same
//! source pipeline the conformance fixtures use.
//!
//! Skipped when Docker is unavailable. `#[ignore]`'d like the other PG-backed
//! property / e2e tests; run with:
//!   `cargo test -p pgevolve --test aggregate_e2e -- --ignored`

mod common;

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::parse_directory;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

use common::{apply_diff, assert_convergent, connect_and_bootstrap, introspect, schemas_of};

/// Source SQL declaring a schema, a managed plpgsql state function, a managed
/// plpgsql final function, and an aggregate over them with an `INITCOND`.
///
/// The sfunc signature is `(state_type, arg_types…)` = `(bigint, integer)` for
/// `my_sum(integer)` with `STYPE = bigint`. The finalfunc signature is
/// `(state_type)` = `(bigint)`. The `INITCOND = '0'` seeds the initial state,
/// exercising the initcond round-trip.
const SOURCE_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.sum_sfunc(bigint, integer) RETURNS bigint \
  LANGUAGE plpgsql AS $$ BEGIN RETURN $1 + $2; END $$;
CREATE FUNCTION app.sum_ffunc(bigint) RETURNS numeric \
  LANGUAGE plpgsql AS $$ BEGIN RETURN $1::numeric; END $$;
CREATE AGGREGATE app.my_sum(integer) ( \
  SFUNC = app.sum_sfunc, \
  STYPE = bigint, \
  FINALFUNC = app.sum_ffunc, \
  INITCOND = '0' \
);
";

/// Parse `SOURCE_SQL` into a `Catalog` via the source pipeline.
fn source_catalog() -> Result<Catalog> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("0001-aggregate.sql"), SOURCE_SQL)?;
    let catalog = parse_directory(dir.path(), &[])?;
    Ok(catalog)
}

#[ignore = "e2e test — requires Docker; run via `cargo test -- --ignored`"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregate_round_trips_against_ephemeral_pg() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    run().await.expect("aggregate round-trip");
}

async fn run() -> Result<()> {
    let source = source_catalog()?;
    let managed = schemas_of(&source);

    // Sanity: the parsed source actually carries the aggregate we expect, so a
    // no-op apply can't accidentally pass the convergence check.
    assert_eq!(
        source.aggregates.len(),
        1,
        "source catalog should declare exactly one aggregate"
    );

    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let mut client = connect_and_bootstrap(&pg).await?;

    // Apply from an empty database to the source state.
    let outcome = apply_diff(&mut client, &Catalog::empty(), &source, &managed, None).await?;
    outcome.map_err(|e| anyhow::anyhow!("apply failed: {e}"))?;

    // Introspect the live database and assert it converges with the source.
    let live = introspect(&pg, &managed).await?;
    assert_eq!(
        live.aggregates.len(),
        1,
        "live database should report exactly one aggregate after apply"
    );
    assert_convergent(&live, &source)
}
