//! End-to-end round-trip for `EVENT TRIGGER`.
//!
//! Applies a source `Catalog` containing a schema, a `RETURNS event_trigger`
//! function, and a (disabled, tag-filtered) `EVENT TRIGGER` to a fresh
//! ephemeral Postgres, introspects the live database, and asserts the live
//! state converges with the source — proving the parser → plan → apply →
//! reader loop agrees for event triggers.
//!
//! The source `Catalog` is built via the project's SQL parse entry point
//! (`parse_directory`) rather than by hand: constructing a valid
//! `RETURNS event_trigger` `Function` value (with a normalized plpgsql body)
//! by hand is fiddly, and parsing the canonical source SQL exercises the same
//! source pipeline the conformance fixtures use.
//!
//! Skipped when Docker is unavailable. `#[ignore]`'d like the other PG-backed
//! property / e2e tests; run with:
//!   `cargo test -p pgevolve --test event_trigger_e2e -- --ignored`

mod common;

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::parse_directory;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

use common::{apply_diff, assert_convergent, connect_and_bootstrap, introspect, schemas_of};

/// Source SQL declaring a schema, an event-trigger function, and a disabled,
/// tag-filtered event trigger referencing it.
///
/// The `ALTER EVENT TRIGGER … DISABLE` exercises the follow-up ALTER path
/// (the trigger is created enabled, then disabled), and the `WHEN TAG IN (…)`
/// clause exercises the tag-filter round-trip.
const SOURCE_SQL: &str = "\
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.audit() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN END $$;
CREATE EVENT TRIGGER et_audit ON ddl_command_start \
  WHEN TAG IN ('CREATE TABLE', 'ALTER TABLE') EXECUTE FUNCTION app.audit();
ALTER EVENT TRIGGER et_audit DISABLE;
";

/// Parse `SOURCE_SQL` into a `Catalog` via the source pipeline.
fn source_catalog() -> Result<Catalog> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("0001-event-trigger.sql"), SOURCE_SQL)?;
    let catalog = parse_directory(dir.path(), &[])?;
    Ok(catalog)
}

#[ignore = "e2e test — requires Docker; run via `cargo test -- --ignored`"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn event_trigger_round_trips_against_ephemeral_pg() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    run().await.expect("event trigger round-trip");
}

async fn run() -> Result<()> {
    let source = source_catalog()?;
    let managed = schemas_of(&source);

    // Sanity: the parsed source actually carries the event trigger we expect,
    // so a no-op apply can't accidentally pass the convergence check.
    assert_eq!(
        source.event_triggers.len(),
        1,
        "source catalog should declare exactly one event trigger"
    );

    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let mut client = connect_and_bootstrap(&pg).await?;

    // Apply from an empty database to the source state.
    let outcome = apply_diff(&mut client, &Catalog::empty(), &source, &managed, None).await?;
    outcome.map_err(|e| anyhow::anyhow!("apply failed: {e}"))?;

    // Introspect the live database and assert it converges with the source.
    let live = introspect(&pg, &managed).await?;
    assert_eq!(
        live.event_triggers.len(),
        1,
        "live database should report exactly one event trigger after apply"
    );
    assert_convergent(&live, &source)
}
