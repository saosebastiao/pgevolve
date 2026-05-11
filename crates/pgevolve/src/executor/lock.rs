//! Singleton advisory lock for the pgevolve executor.
//!
//! Spec §8: only one apply may run against a database at a time. We use a
//! Postgres session-scoped advisory lock plus a row in `pgevolve.lock` so
//! other sessions can see who holds it.
//!
//! The advisory lock is released automatically when the session disconnects;
//! the audit row is cleared explicitly via [`release_lock`]. Apply paths
//! should call [`release_lock`] at the end to leave the lock table clean.

use tokio_postgres::Client;

use super::error::ApplyError;

/// 64-bit advisory-lock key derived from the ASCII bytes `b"PGEVOLVE"`.
///
/// Stable across builds and machines; treated as opaque by Postgres.
pub const PGEVOLVE_LOCK_KEY: i64 = i64::from_be_bytes(*b"PGEVOLVE");

/// Try to acquire the singleton lock. Updates the `pgevolve.lock` audit row
/// on success. Returns `Err(ApplyError::LockHeld)` if another session holds it.
///
/// The lock is session-scoped; if this session disconnects without calling
/// [`release_lock`], Postgres releases the advisory lock automatically and
/// the next acquirer's UPDATE clears the audit row.
pub async fn try_acquire_lock(client: &Client, actor: &str) -> Result<(), ApplyError> {
    let row = client
        .query_one("SELECT pg_try_advisory_lock($1)", &[&PGEVOLVE_LOCK_KEY])
        .await?;
    let acquired: bool = row.get(0);
    if !acquired {
        return Err(ApplyError::LockHeld);
    }
    client
        .execute(
            "UPDATE pgevolve.lock
             SET held_by=$1, held_since=now(), pgevolve_version=$2
             WHERE singleton=true",
            &[&actor, &pgevolve_core::VERSION],
        )
        .await?;
    Ok(())
}

/// Release the advisory lock and clear the audit row. Idempotent.
pub async fn release_lock(client: &Client) -> Result<(), ApplyError> {
    client
        .execute(
            "UPDATE pgevolve.lock
             SET held_by=NULL, held_since=NULL, pgevolve_version=NULL
             WHERE singleton=true",
            &[],
        )
        .await?;
    client
        .execute("SELECT pg_advisory_unlock($1)", &[&PGEVOLVE_LOCK_KEY])
        .await?;
    Ok(())
}
