//! Bootstrap and upgrade the `pgevolve` metadata schema.
//!
//! Idempotent: running [`bootstrap_metadata`] on a fresh database creates the
//! schema and every table; running it again is a no-op. Future schema
//! evolutions append a new `BootstrapMigration` entry rather than editing
//! the existing SQL.

use tokio_postgres::Client;

use super::error::ApplyError;

/// Create the `pgevolve` schema and run any pending bootstrap migrations.
///
/// Takes `&mut Client` because each bootstrap migration runs in its own
/// `tokio_postgres::Transaction`, which requires exclusive access.
pub async fn bootstrap_metadata(client: &mut Client) -> Result<(), ApplyError> {
    client.batch_execute(SQL_BOOTSTRAP_SCHEMA).await?;

    let row = client
        .query_one(
            "SELECT COALESCE(max(version), 0)::int FROM pgevolve.bootstrap_version",
            &[],
        )
        .await?;
    let current: i32 = row.get(0);

    for m in BOOTSTRAP_MIGRATIONS.iter().filter(|m| m.version > current) {
        // Bootstrap migrations run inside a transaction so failure leaves
        // the schema in a consistent state and the version row is never
        // ahead of the actual DDL.
        let tx = client.transaction().await?;
        tx.batch_execute(m.sql).await?;
        tx.execute(
            "INSERT INTO pgevolve.bootstrap_version (version, applied_at)
             VALUES ($1, now())",
            &[&m.version],
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// One bootstrap migration. Append new versions to [`BOOTSTRAP_MIGRATIONS`];
/// never edit an applied version's SQL.
struct BootstrapMigration {
    version: i32,
    sql: &'static str,
}

/// Always-run idempotent setup: schema + the version-tracking table.
const SQL_BOOTSTRAP_SCHEMA: &str = "\
CREATE SCHEMA IF NOT EXISTS pgevolve;
CREATE TABLE IF NOT EXISTS pgevolve.bootstrap_version (
    version    int         PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);
";

/// Append-only list of bootstrap migrations.
const BOOTSTRAP_MIGRATIONS: &[BootstrapMigration] = &[BootstrapMigration {
    version: 1,
    sql: "\
CREATE TABLE pgevolve.apply_log (
    apply_id          uuid        PRIMARY KEY,
    plan_id           text        NOT NULL,
    plan_hash         text        NOT NULL,
    pgevolve_version  text        NOT NULL,
    source_rev        text,
    target_identity   text        NOT NULL,
    actor             text,
    started_at        timestamptz NOT NULL DEFAULT now(),
    finished_at       timestamptz,
    status            text        NOT NULL
                                  CHECK (status IN ('running','succeeded','failed','aborted')),
    error_message     text
);
CREATE INDEX apply_log_started_at_idx ON pgevolve.apply_log (started_at DESC);

CREATE TABLE pgevolve.plan_steps (
    apply_id      uuid    NOT NULL REFERENCES pgevolve.apply_log(apply_id) ON DELETE CASCADE,
    step_no       int     NOT NULL,
    group_no      int     NOT NULL,
    kind          text    NOT NULL,
    destructive   boolean NOT NULL,
    targets       text[]  NOT NULL,
    sql_text      text    NOT NULL,
    started_at    timestamptz,
    finished_at   timestamptz,
    status        text    NOT NULL
                          CHECK (status IN ('pending','running','succeeded','failed','rolled_back','skipped')),
    error_message text,
    PRIMARY KEY (apply_id, step_no)
);

CREATE TABLE pgevolve.lock (
    singleton         boolean     PRIMARY KEY DEFAULT true CHECK (singleton),
    held_by           text,
    held_since        timestamptz,
    pgevolve_version  text
);
INSERT INTO pgevolve.lock (singleton) VALUES (true);
",
}];
