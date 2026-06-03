//! Cluster apply: connect and run each cluster plan step against the live
//! Postgres instance.
//!
//! This is the minimal v0.3.0 implementation. Each step runs in its own
//! `BEGIN; ... COMMIT;` block. Full parity with the per-DB apply path (intent
//! gates, manifest cross-check, advisory-lock, status logging) is left as
//! follow-up scope for Stage 12 / v0.3.0 hardening.
//!
//! Stage 9 of the cluster-roles implementation plan.

use std::path::Path;

use tokio_postgres::Client;

use pgevolve_core::plan::Plan;
use pgevolve_core::plan::raw_step::{RawStep, TransactionConstraint};

use crate::cluster_config::ClusterConfig;
use crate::executor::{
    ApplyError, ApplyOutcome, ApplyOverrides,
    cluster_preflight::{ClusterPreflightOverrides, run_cluster_preflight},
    lock::{release_lock, try_acquire_lock},
};

/// Apply an in-memory cluster [`Plan`] to a live Postgres connection.
///
/// Mirrors [`crate::executor::apply_plan`] but with a cluster-flavoured
/// preflight ([`run_cluster_preflight`]). No drift recheck.
///
/// Steps:
/// 1. Bootstrap or upgrade the `pgevolve` metadata schema.
/// 2. Acquire the singleton advisory lock.
/// 3. Run cluster preflight (identity match + intent approval).
/// 4. Open an `apply_log` row.
/// 5. Execute each group in order.
/// 6. Close the `apply_log` row with the final status.
///
/// # Errors
///
/// Returns [`ApplyError`] on the first failed step. The advisory lock is
/// released before propagating in every failure branch.
pub async fn apply_cluster_plan(
    plan: &Plan,
    client: &mut Client,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    crate::executor::bootstrap::bootstrap_metadata(client).await?;

    let actor = overrides
        .actor
        .clone()
        .unwrap_or_else(crate::executor::default_actor);
    try_acquire_lock(client, &actor).await?;

    let cluster_overrides = ClusterPreflightOverrides {
        allow_different_target: overrides.allow_different_target,
        allow_unapproved_intents: overrides.allow_unapproved_intents,
    };
    let preflight_result = run_cluster_preflight(client, plan, cluster_overrides).await;
    if let Err(e) = preflight_result {
        let _ = release_lock(client).await;
        return Err(e);
    }

    let apply_id = crate::executor::audit::open_apply_log(client, plan, &actor).await?;
    let exec_result =
        crate::executor::execute::execute_plan(client, plan, apply_id, overrides.abort_after_step)
            .await;
    match exec_result {
        Ok(()) => {
            crate::executor::audit::close_apply_log(client, apply_id, "succeeded", None).await?;
            release_lock(client).await?;
            Ok(ApplyOutcome::Succeeded { apply_id })
        }
        Err(ApplyError::AbortedAfterStep { step_no }) => {
            crate::executor::audit::close_apply_log(
                client,
                apply_id,
                "aborted",
                Some(&format!("abort_after_step={step_no}")),
            )
            .await?;
            let _ = release_lock(client).await;
            Err(ApplyError::AbortedAfterStep { step_no })
        }
        Err(e) => {
            let msg = e.to_string();
            crate::executor::audit::close_apply_log(client, apply_id, "failed", Some(&msg)).await?;
            let _ = release_lock(client).await;
            Err(e)
        }
    }
}

/// Apply all steps in `steps` against the Postgres instance named by
/// `cfg.connection.dsn`.
///
/// Each step runs inside a dedicated transaction. If any step fails the
/// transaction is rolled back and the error is returned immediately;
/// subsequent steps are not attempted.
///
/// # Future work
/// - Intent-gate enforcement for `DropRole` steps (Stage 12).
/// - Advisory-lock acquisition / release (Stage 12).
/// - `pgevolve.apply_log` row creation and per-step status tracking (Stage 12).
/// - Manifest / plan-id cross-check (Stage 12).
pub async fn apply_cluster_steps(
    steps: &[RawStep],
    cfg: &ClusterConfig,
) -> Result<(), ClusterApplyError> {
    if steps.is_empty() {
        return Ok(());
    }

    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .map_err(|e| ClusterApplyError::Connection(e.to_string()))?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster apply connection task ended");
        }
    });

    for step in steps {
        execute_step(&mut client, step).await?;
    }

    Ok(())
}

/// Apply a cluster plan directory to a live Postgres connection.
///
/// Reads `plan.sql` from `plan_dir`, parses each step's SQL block, and runs
/// each statement via [`apply_cluster_steps`].
///
/// The `plan_dir` layout matches what the CLI will write in Stage 10:
/// ```text
/// cluster-plans/<id>/plan.sql
/// ```
///
/// # Limitations (tracked in [#7])
/// - `intent.toml` is not read; destructive-step approval is not enforced.
/// - `manifest.toml` cross-check is not performed.
/// - No advisory lock is taken.
/// - No `pgevolve.apply_log` row is created.
///
/// [#7]: https://github.com/saosebastiao/pgevolve/issues/7
pub async fn apply_cluster_plan_dir(
    plan_dir: &Path,
    cfg: &ClusterConfig,
) -> Result<(), ClusterApplyError> {
    let sql_path = plan_dir.join("plan.sql");
    let sql = std::fs::read_to_string(&sql_path).map_err(|e| ClusterApplyError::Io {
        path: sql_path.clone(),
        source: e,
    })?;

    // The plan.sql format uses the same `-- @pgevolve step` directive headers
    // as per-DB plans, but for now we simply split on semicolons and run each
    // non-empty statement. Parsing the structured header (via
    // `pgevolve_core::plan::deserialize::read_plan_sql`) and wiring the same
    // intent / manifest / audit machinery the per-DB executor uses is tracked
    // in GH #7.
    let statements = split_sql_statements(&sql);

    if statements.is_empty() {
        return Ok(());
    }

    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .map_err(|e| ClusterApplyError::Connection(e.to_string()))?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster plan-dir apply connection task ended");
        }
    });

    for stmt in &statements {
        run_in_transaction(&mut client, stmt).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Execute a single [`RawStep`] against an open client.
///
/// Respects the step's [`TransactionConstraint`]: transactional steps are
/// wrapped in `BEGIN; ... COMMIT;`; autocommit steps are executed directly.
/// Both paths are used for correctness even though all cluster ops are
/// currently `InTransaction`.
async fn execute_step(
    client: &mut tokio_postgres::Client,
    step: &RawStep,
) -> Result<(), ClusterApplyError> {
    match step.transactional {
        TransactionConstraint::InTransaction => {
            run_in_transaction(client, &step.sql).await?;
        }
        TransactionConstraint::OutsideTransaction => {
            client.execute(step.sql.as_str(), &[]).await.map_err(|e| {
                ClusterApplyError::StepFailed {
                    sql: step.sql.clone(),
                    source: e,
                }
            })?;
        }
    }
    Ok(())
}

/// Wrap `sql` in `BEGIN; ... COMMIT;` and execute it.
async fn run_in_transaction(
    client: &mut tokio_postgres::Client,
    sql: &str,
) -> Result<(), ClusterApplyError> {
    let tx = client
        .transaction()
        .await
        .map_err(|e| ClusterApplyError::StepFailed {
            sql: sql.to_string(),
            source: e,
        })?;
    tx.execute(sql, &[])
        .await
        .map_err(|e| ClusterApplyError::StepFailed {
            sql: sql.to_string(),
            source: e,
        })?;
    tx.commit()
        .await
        .map_err(|e| ClusterApplyError::StepFailed {
            sql: sql.to_string(),
            source: e,
        })?;
    Ok(())
}

/// Split raw SQL text into individual statements (naively by semicolon).
///
/// Strips SQL line comments (`--`), collapses whitespace, and discards
/// empty fragments. This is intentionally minimal for v0.3.0 — the cluster
/// plan SQL only ever contains role DDL statements which are unambiguous.
fn split_sql_statements(sql: &str) -> Vec<String> {
    // Remove comment lines (lines starting with `--` after trimming).
    let without_comments: String = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");

    without_comments
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Errors raised by cluster apply operations.
#[derive(Debug, thiserror::Error)]
pub enum ClusterApplyError {
    /// Could not open a connection to Postgres.
    #[error("connection error: {0}")]
    Connection(String),
    /// A DDL step failed to execute.
    #[error("step execution failed (sql: {sql:?}): {source}")]
    StepFailed {
        /// The SQL statement that failed.
        sql: String,
        /// The underlying Postgres error.
        #[source]
        source: tokio_postgres::Error,
    },
    /// Could not read the plan SQL file from disk.
    #[error("i/o reading {path}: {source}")]
    Io {
        /// Path to the file that could not be read.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_discards_comment_lines() {
        let sql = "-- @pgevolve step kind=create_role\nCREATE ROLE a;\n-- comment\nCREATE ROLE b;";
        let stmts = split_sql_statements(sql);
        assert_eq!(stmts, vec!["CREATE ROLE a", "CREATE ROLE b"]);
    }

    #[test]
    fn split_discards_empty_fragments() {
        let sql = "CREATE ROLE a;\n\n;";
        let stmts = split_sql_statements(sql);
        assert_eq!(stmts, vec!["CREATE ROLE a"]);
    }

    #[test]
    fn split_single_statement_no_trailing_semicolon() {
        let stmts = split_sql_statements("CREATE ROLE x");
        assert_eq!(stmts, vec!["CREATE ROLE x"]);
    }
}
