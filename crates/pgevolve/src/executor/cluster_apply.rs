//! Cluster apply: run a cluster plan against the live Postgres instance.
//!
//! The high-level entry point is [`apply_cluster_plan`], which mirrors the
//! per-DB [`crate::executor::apply_plan`] pipeline (bootstrap, advisory lock,
//! cluster preflight, `apply_log` audit, execute).
//!
//! [`apply_cluster_plan_dir`] is a thin wrapper that reads a plan directory
//! from disk and delegates to [`apply_cluster_plan`].

use std::path::Path;

use tokio_postgres::Client;

use pgevolve_core::plan::Plan;

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

/// Apply a cluster plan directory to a live Postgres connection.
///
/// Reads the plan from disk via [`pgevolve_core::plan::read_plan_dir`] and
/// delegates to [`apply_cluster_plan`].
///
/// Use [`apply_cluster_plan`] directly when you already have a [`Plan`] value
/// (test harnesses, library callers that built the plan in-process).
///
/// # Errors
///
/// Returns [`ClusterApplyError::PlanLoad`] if the plan directory cannot be
/// read, or [`ClusterApplyError::Connection`] / [`ClusterApplyError::Apply`]
/// for subsequent failures.
pub async fn apply_cluster_plan_dir(
    plan_dir: &Path,
    cfg: &ClusterConfig,
) -> Result<(), ClusterApplyError> {
    let plan = pgevolve_core::plan::read_plan_dir(plan_dir)
        .map_err(|e| ClusterApplyError::PlanLoad(e.to_string()))?;

    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .map_err(|e| ClusterApplyError::Connection(e.to_string()))?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster plan-dir apply connection task ended");
        }
    });

    let overrides = crate::executor::ApplyOverrides::default();
    apply_cluster_plan(&plan, &mut client, overrides)
        .await
        .map_err(|e| ClusterApplyError::Apply(e.to_string()))?;

    Ok(())
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
    /// Could not load the plan directory from disk.
    #[error("plan load error: {0}")]
    PlanLoad(String),
    /// Apply pipeline reported an error.
    #[error("apply error: {0}")]
    Apply(String),
}
