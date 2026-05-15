//! Apply-time executor.
//!
//! Takes a serialized `Plan` and applies it to a live Postgres database with
//! bootstrap, advisory locking, drift recheck, intent enforcement, per-step
//! audit logging, and rollback handling. Public entry point: [`apply`].

pub mod audit;
pub mod bootstrap;
pub mod error;
pub mod execute;
pub mod lock;
pub mod preflight;
pub mod status;

use std::path::Path;

use tokio_postgres::Client;
use uuid::Uuid;

use pgevolve_core::catalog::CatalogFilter;

pub use bootstrap::bootstrap_metadata;
pub use error::ApplyError;
pub use lock::{release_lock, try_acquire_lock};
pub use preflight::{PreflightOverrides, run_preflight};

/// Caller-supplied overrides for the apply flow.
#[derive(Debug, Clone, Default)]
pub struct ApplyOverrides {
    /// Skip the target-identity match check. Use only when intentionally
    /// applying a plan to a different database.
    pub allow_different_target: bool,
    /// Skip the drift recheck. Use only when intentionally re-applying after
    /// out-of-band changes.
    pub allow_drift: bool,
    /// When true, the apply path bypasses the `check_lint_waivers` preflight.
    /// Set internally by:
    /// - `validate --shadow` (shadow plans have no real drift to waive)
    /// - test harnesses (legacy tests predate the waiver mechanism)
    ///
    /// There is no CLI flag to set this from a user-facing apply invocation.
    /// If you find yourself needing one, the right answer is probably to
    /// re-plan with an appropriate `[[lint_waiver]]` rather than bypass.
    pub allow_unwaived_lint: bool,
    /// Override the actor string written to `pgevolve.apply_log`.
    pub actor: Option<String>,
    /// Testkit / chaos hook: if `Some(n)`, the executor aborts cleanly after
    /// the step whose `step_no == n` succeeds, returning
    /// [`ApplyError::AbortedAfterStep`] and marking the `apply_log` row
    /// `aborted`. Remaining steps stay `pending` so a subsequent
    /// re-plan + re-apply can resume.
    pub abort_after_step: Option<u32>,
}

/// Outcome of a successful [`apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The plan executed successfully end-to-end.
    Succeeded {
        /// UUID assigned in `pgevolve.apply_log` for this run.
        apply_id: Uuid,
    },
}

/// Apply a plan directory to a live Postgres connection.
///
/// Steps (spec §8):
/// 1. Read `plan.sql` + `intent.toml` + `manifest.toml` from `plan_dir`.
/// 2. Bootstrap or upgrade the `pgevolve` metadata schema.
/// 3. Acquire the singleton advisory lock.
/// 4. Run preflight checks (identity match, drift, intent approval).
/// 5. Open an `apply_log` row + pre-populate `plan_steps` as `pending`.
/// 6. Execute each group in order; mark steps `succeeded`, `failed`, or `rolled_back`.
/// 7. Close the `apply_log` row with the final status.
///
/// The advisory lock is released automatically when the returned future
/// completes (success or failure).
pub async fn apply(
    plan_dir: &Path,
    client: &mut Client,
    filter: &CatalogFilter,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    let plan = pgevolve_core::plan::read_plan_dir(plan_dir)?;

    bootstrap_metadata(client).await?;

    let actor = overrides.actor.clone().unwrap_or_else(default_actor);
    try_acquire_lock(client, &actor).await?;

    let preflight = PreflightOverrides {
        allow_different_target: overrides.allow_different_target,
        allow_drift: overrides.allow_drift,
        allow_unwaived_lint: overrides.allow_unwaived_lint,
    };
    let preflight_result = run_preflight(client, &plan, filter, preflight).await;
    if let Err(e) = preflight_result {
        // Failure before any DDL — release the lock before propagating.
        let _ = release_lock(client).await;
        return Err(e);
    }

    let apply_id = audit::open_apply_log(client, &plan, &actor).await?;
    let exec_result =
        execute::execute_plan(client, &plan, apply_id, overrides.abort_after_step).await;
    match exec_result {
        Ok(()) => {
            audit::close_apply_log(client, apply_id, "succeeded", None).await?;
            release_lock(client).await?;
            Ok(ApplyOutcome::Succeeded { apply_id })
        }
        Err(ApplyError::AbortedAfterStep { step_no }) => {
            audit::close_apply_log(
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
            audit::close_apply_log(client, apply_id, "failed", Some(&msg)).await?;
            let _ = release_lock(client).await;
            Err(e)
        }
    }
}

/// Best-effort actor string for audit logging.
fn default_actor() -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    let host = hostname_string();
    format!("{user}@{host}")
}

fn hostname_string() -> String {
    // Read /etc/hostname or fall back to env. No `unsafe` per crate lints.
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".into())
}
