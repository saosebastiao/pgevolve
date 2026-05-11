//! Target-identity hashing.
//!
//! The identity is a 16-char hex prefix of a BLAKE3 hash over the fields that
//! distinguish one Postgres instance + database from another:
//! `(current_database, host, port, cluster_name, system_identifier)`. Used by
//! the executor's preflight to ensure a plan targets the database it was
//! planned against.
//!
//! `system_identifier` comes from `pg_control_system()` (PG 9.6+).

use tokio_postgres::Client;

use crate::executor::ApplyError;

/// Compute the 16-char target identity for the database `client` is connected to.
pub async fn compute_target_identity(client: &Client) -> Result<String, ApplyError> {
    let row = client
        .query_one(
            "SELECT
                 current_database(),
                 inet_server_addr()::text,
                 inet_server_port(),
                 current_setting('cluster_name', true),
                 (SELECT system_identifier::text FROM pg_control_system())",
            &[],
        )
        .await?;

    let mut h = blake3::Hasher::new();
    h.update(b"pgevolve-target-id-v1\n");

    // Each column is rendered as text and appended with a NUL separator so
    // that two fields can't accidentally hash-equal across boundary cases
    // (e.g., empty fields).
    for i in 0..5 {
        let s: Option<String> = row.try_get(i).ok();
        if let Some(v) = s {
            h.update(v.as_bytes());
        }
        h.update(&[0]);
    }
    let full = hex::encode(h.finalize().as_bytes());
    Ok(full[..16].to_string())
}
