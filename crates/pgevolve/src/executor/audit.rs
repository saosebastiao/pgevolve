//! Audit writers for the `pgevolve.apply_log` and `pgevolve.plan_steps` tables.
//!
//! Every apply run inserts one `apply_log` row (in `running` state) and one
//! `plan_steps` row per step (in `pending` state). Per-step status transitions
//! happen during [`execute_plan`](super::execute::execute_plan); the final
//! `apply_log` status is set by [`close_apply_log`].

use tokio_postgres::Client;
use uuid::Uuid;

use pgevolve_core::plan::plan::kind_name;
use pgevolve_core::plan::Plan;

use super::error::ApplyError;

/// Insert the `apply_log` row and pre-populate `plan_steps` with one
/// `pending` row per step. Returns the new `apply_id`.
pub async fn open_apply_log(
    client: &Client,
    plan: &Plan,
    actor: &str,
) -> Result<Uuid, ApplyError> {
    let apply_id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO pgevolve.apply_log
             (apply_id, plan_id, plan_hash, pgevolve_version, source_rev,
              target_identity, actor, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'running')",
            &[
                &apply_id,
                &plan.id.short(),
                &plan.id.to_hex(),
                &plan.metadata.pgevolve_version,
                &plan.metadata.source_rev,
                &plan.metadata.target_identity,
                &actor,
            ],
        )
        .await?;

    for group in &plan.groups {
        for step in &group.steps {
            let targets: Vec<String> = step.targets.iter().map(ToString::to_string).collect();
            client
                .execute(
                    "INSERT INTO pgevolve.plan_steps
                     (apply_id, step_no, group_no, kind, destructive, targets, sql_text, status)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending')",
                    &[
                        &apply_id,
                        &i32::try_from(step.step_no).unwrap_or(i32::MAX),
                        &i32::try_from(group.id).unwrap_or(i32::MAX),
                        &kind_name(step.kind),
                        &step.destructive,
                        &targets,
                        &step.sql,
                    ],
                )
                .await?;
        }
    }
    Ok(apply_id)
}

/// Mark a step as `running` and stamp `started_at`.
pub async fn mark_step_running(
    client: &Client,
    apply_id: Uuid,
    step_no: u32,
) -> Result<(), ApplyError> {
    client
        .execute(
            "UPDATE pgevolve.plan_steps
             SET status='running', started_at=now()
             WHERE apply_id=$1 AND step_no=$2",
            &[&apply_id, &i32::try_from(step_no).unwrap_or(i32::MAX)],
        )
        .await?;
    Ok(())
}

/// Mark a step as `succeeded` and stamp `finished_at`.
pub async fn mark_step_succeeded(
    client: &Client,
    apply_id: Uuid,
    step_no: u32,
) -> Result<(), ApplyError> {
    client
        .execute(
            "UPDATE pgevolve.plan_steps
             SET status='succeeded', finished_at=now()
             WHERE apply_id=$1 AND step_no=$2",
            &[&apply_id, &i32::try_from(step_no).unwrap_or(i32::MAX)],
        )
        .await?;
    Ok(())
}

/// Mark a step as `failed`, stamp `finished_at`, and store the error message.
pub async fn mark_step_failed(
    client: &Client,
    apply_id: Uuid,
    step_no: u32,
    err: &str,
) -> Result<(), ApplyError> {
    client
        .execute(
            "UPDATE pgevolve.plan_steps
             SET status='failed', finished_at=now(), error_message=$3
             WHERE apply_id=$1 AND step_no=$2",
            &[&apply_id, &i32::try_from(step_no).unwrap_or(i32::MAX), &err],
        )
        .await?;
    Ok(())
}

/// Mark steps in a group as `rolled_back`.
///
/// Updates every step whose current status is `succeeded` or `running` to
/// `rolled_back`. Used after a transactional group fails: every step in the
/// same group that already completed is now logically reverted.
pub async fn mark_steps_rolled_back(
    client: &Client,
    apply_id: Uuid,
    group_no: u32,
) -> Result<(), ApplyError> {
    client
        .execute(
            "UPDATE pgevolve.plan_steps
             SET status='rolled_back', finished_at=now()
             WHERE apply_id=$1 AND group_no=$2 AND status IN ('succeeded','running')",
            &[&apply_id, &i32::try_from(group_no).unwrap_or(i32::MAX)],
        )
        .await?;
    Ok(())
}

/// Close out the `apply_log` row with the final status. `status` is one of
/// `succeeded`, `failed`, `aborted`.
pub async fn close_apply_log(
    client: &Client,
    apply_id: Uuid,
    status: &str,
    err: Option<&str>,
) -> Result<(), ApplyError> {
    client
        .execute(
            "UPDATE pgevolve.apply_log
             SET status=$2, finished_at=now(), error_message=$3
             WHERE apply_id=$1",
            &[&apply_id, &status, &err],
        )
        .await?;
    Ok(())
}
