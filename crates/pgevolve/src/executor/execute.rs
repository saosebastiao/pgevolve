//! Group/step execution loop.
//!
//! Each transactional group runs inside a single `BEGIN; ... COMMIT;` — a
//! single step failure rolls back the whole group, and every step in the
//! group is marked `rolled_back` (or `failed` for the actual failing step).
//! Non-transactional groups execute as a sequence of autocommit statements;
//! a failure stops the group and leaves earlier steps in `succeeded` state.

use tokio_postgres::Client;
use uuid::Uuid;

use pgevolve_core::plan::{Plan, TransactionGroup};

use super::audit;
use super::error::ApplyError;

/// Apply every group in a plan in order.
pub async fn execute_plan(
    client: &mut Client,
    plan: &Plan,
    apply_id: Uuid,
) -> Result<(), ApplyError> {
    for group in &plan.groups {
        if group.transactional {
            execute_transactional_group(client, apply_id, group).await?;
        } else {
            execute_autocommit_group(client, apply_id, group).await?;
        }
    }
    Ok(())
}

async fn execute_transactional_group(
    client: &mut Client,
    apply_id: Uuid,
    group: &TransactionGroup,
) -> Result<(), ApplyError> {
    let tx = client.transaction().await?;

    for step in &group.steps {
        // mark_step_running operates on the same connection as the tx;
        // tokio_postgres's Transaction::client() returns the underlying
        // `&Client` so audit UPDATEs are part of the same transaction and
        // get rolled back together if a later step fails. That's fine: we
        // re-mark them outside the tx after rollback.
        audit::mark_step_running(tx.client(), apply_id, step.step_no).await?;
        if let Err(e) = tx.batch_execute(&step.sql).await {
            let err_msg = e.to_string();
            tx.rollback().await?;
            // After rollback, write the final audit rows on the bare client.
            audit::mark_step_failed(client, apply_id, step.step_no, &err_msg).await?;
            audit::mark_steps_rolled_back(client, apply_id, group.id).await?;
            return Err(ApplyError::StepFailed {
                step_no: step.step_no,
                group_no: group.id,
                error: err_msg,
            });
        }
        audit::mark_step_succeeded(tx.client(), apply_id, step.step_no).await?;
    }

    tx.commit().await?;
    Ok(())
}

async fn execute_autocommit_group(
    client: &Client,
    apply_id: Uuid,
    group: &TransactionGroup,
) -> Result<(), ApplyError> {
    for step in &group.steps {
        audit::mark_step_running(client, apply_id, step.step_no).await?;
        if let Err(e) = client.batch_execute(&step.sql).await {
            let err_msg = e.to_string();
            audit::mark_step_failed(client, apply_id, step.step_no, &err_msg).await?;
            return Err(ApplyError::StepFailed {
                step_no: step.step_no,
                group_no: group.id,
                error: err_msg,
            });
        }
        audit::mark_step_succeeded(client, apply_id, step.step_no).await?;
    }
    Ok(())
}
