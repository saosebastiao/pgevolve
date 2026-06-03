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

/// Compute the cluster target identity for the cluster `client` is connected to.
///
/// Format: `cluster:{system_identifier_lower_hex_zero_padded_to_16_chars}`.
/// The prefix distinguishes cluster identities from per-DB identities at a
/// glance in `apply_log` queries; the hex encoding is fixed-width so identity
/// strings sort predictably.
///
/// `system_identifier` comes from `pg_control_system()` (PG 9.6+). It is unique
/// per `initdb` and stable across replicas of the same physical cluster.
///
/// # Errors
///
/// Returns `ApplyError` if the `pg_control_system()` query fails or the
/// returned column cannot be parsed as a u64.
pub async fn compute_cluster_target_identity(client: &Client) -> Result<String, ApplyError> {
    let row = client
        .query_one(
            "SELECT system_identifier::text FROM pg_control_system()",
            &[],
        )
        .await?;
    let s: String = row.try_get(0)?;
    let n: u64 = s
        .parse()
        .map_err(|_| ApplyError::Internal(format!("unparseable system_identifier: {s}")))?;
    Ok(format_cluster_identity(n))
}

/// Format a system identifier as the cluster identity string.
fn format_cluster_identity(system_identifier: u64) -> String {
    format!("cluster:{system_identifier:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_identity_format() {
        // Hex of decimal 12345 = "3039"; format prefixed with "cluster:".
        let id = format_cluster_identity(12345u64);
        assert_eq!(id, "cluster:0000000000003039");
    }

    #[test]
    fn cluster_identity_max_value() {
        let id = format_cluster_identity(u64::MAX);
        assert_eq!(id, "cluster:ffffffffffffffff");
    }

    #[test]
    fn cluster_identity_zero() {
        let id = format_cluster_identity(0);
        assert_eq!(id, "cluster:0000000000000000");
    }
}
