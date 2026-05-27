//! Group/step execution loop.
//!
//! Each transactional group runs inside a single `BEGIN; ... COMMIT;` — a
//! single step failure rolls back the whole group, and every step in the
//! group is marked `rolled_back` (or `failed` for the actual failing step).
//! Non-transactional groups execute as a sequence of autocommit statements;
//! a failure stops the group and leaves earlier steps in `succeeded` state.

use tokio_postgres::{Client, Error as PgError};
use uuid::Uuid;

/// Render a `tokio_postgres::Error` with as much context as we can pull
/// from the underlying `DbError` (SQLSTATE + server message). Falls back
/// to the terse Display if no `DbError` is attached.
fn render_pg_error(e: &PgError) -> String {
    e.as_db_error().map_or_else(
        || e.to_string(),
        |db| {
            format!(
                "[{code}] {msg}: {detail}",
                code = db.code().code(),
                msg = db.message(),
                detail = db.detail().unwrap_or(""),
            )
        },
    )
}

use pgevolve_core::plan::plan::StepOverride;
use pgevolve_core::plan::plan::kind_name;
use pgevolve_core::plan::raw_step::RawStep;
use pgevolve_core::plan::{Plan, TransactionGroup};

use super::audit;
use super::error::ApplyError;

/// Return `true` if a matching `[[step_override]]` row suppresses this step.
fn is_step_suppressed(step: &RawStep, overrides: &[StepOverride]) -> bool {
    let wire = kind_name(step.kind);
    let primary_target = step
        .targets
        .first()
        .map(pgevolve_core::identifier::QualifiedName::render_sql);
    overrides.iter().any(|so| {
        so.suppress && so.kind == wire && primary_target.as_deref() == Some(so.target.as_str())
    })
}

/// Apply every group in a plan in order.
///
/// If `abort_after_step` is `Some(n)`, the executor stops cleanly after the
/// step whose `step_no == n` succeeds and returns
/// [`ApplyError::AbortedAfterStep`]. Used by the testkit chaos harness.
pub async fn execute_plan(
    client: &mut Client,
    plan: &Plan,
    apply_id: Uuid,
    abort_after_step: Option<u32>,
) -> Result<(), ApplyError> {
    for group in &plan.groups {
        if group.transactional {
            execute_transactional_group(
                client,
                apply_id,
                group,
                &plan.step_overrides,
                abort_after_step,
            )
            .await?;
        } else {
            execute_autocommit_group(
                client,
                apply_id,
                group,
                &plan.step_overrides,
                abort_after_step,
            )
            .await?;
        }
        // After-group abort check: if the abort step lived in this group
        // and the executor returned cleanly, that group's loop already
        // raised AbortedAfterStep — but the per-group functions return Ok
        // only after fully completing, so this is a no-op here.
    }
    Ok(())
}

async fn execute_transactional_group(
    client: &mut Client,
    apply_id: Uuid,
    group: &TransactionGroup,
    step_overrides: &[StepOverride],
    abort_after_step: Option<u32>,
) -> Result<(), ApplyError> {
    let tx = client.transaction().await?;
    let mut abort_step: Option<u32> = None;

    for step in &group.steps {
        // Check for a matching [[step_override]] row before running.
        if is_step_suppressed(step, step_overrides) {
            // Record in the audit log as skipped and skip execution.
            // Use tx.client() so the audit UPDATE is part of the same
            // transaction as the other steps; if the tx rolls back later,
            // mark_steps_rolled_back will flip these to 'rolled_back'.
            audit::mark_step_suppressed(tx.client(), apply_id, step.step_no).await?;
            if abort_after_step == Some(step.step_no) {
                abort_step = Some(step.step_no);
                break;
            }
            continue;
        }
        // mark_step_running operates on the same connection as the tx;
        // tokio_postgres's Transaction::client() returns the underlying
        // `&Client` so audit UPDATEs are part of the same transaction and
        // get rolled back together if a later step fails. That's fine: we
        // re-mark them outside the tx after rollback.
        audit::mark_step_running(tx.client(), apply_id, step.step_no).await?;
        let resolved_sql =
            crate::executor::env_interp::resolve(&step.sql, |k| std::env::var(k).ok())
                .map_err(|e| ApplyError::MissingEnvVar(e.0, step.step_no))?;
        if let Err(e) = tx.batch_execute(&resolved_sql).await {
            let err_msg = render_pg_error(&e);
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
        if abort_after_step == Some(step.step_no) {
            abort_step = Some(step.step_no);
            break;
        }
    }

    tx.commit().await?;
    if let Some(step_no) = abort_step {
        return Err(ApplyError::AbortedAfterStep { step_no });
    }
    Ok(())
}

async fn execute_autocommit_group(
    client: &Client,
    apply_id: Uuid,
    group: &TransactionGroup,
    step_overrides: &[StepOverride],
    abort_after_step: Option<u32>,
) -> Result<(), ApplyError> {
    for step in &group.steps {
        // Check for a matching [[step_override]] row before running.
        if is_step_suppressed(step, step_overrides) {
            // Record in the audit log as skipped and skip execution.
            audit::mark_step_suppressed(client, apply_id, step.step_no).await?;
            if abort_after_step == Some(step.step_no) {
                return Err(ApplyError::AbortedAfterStep {
                    step_no: step.step_no,
                });
            }
            continue;
        }
        audit::mark_step_running(client, apply_id, step.step_no).await?;
        let resolved_sql =
            crate::executor::env_interp::resolve(&step.sql, |k| std::env::var(k).ok())
                .map_err(|e| ApplyError::MissingEnvVar(e.0, step.step_no))?;
        if let Err(e) = client.batch_execute(&resolved_sql).await {
            let err_msg = render_pg_error(&e);
            audit::mark_step_failed(client, apply_id, step.step_no, &err_msg).await?;
            return Err(ApplyError::StepFailed {
                step_no: step.step_no,
                group_no: group.id,
                error: err_msg,
            });
        }
        audit::mark_step_succeeded(client, apply_id, step.step_no).await?;
        if abort_after_step == Some(step.step_no) {
            return Err(ApplyError::AbortedAfterStep {
                step_no: step.step_no,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::identifier::{Identifier, QualifiedName};
    use pgevolve_core::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn refresh_step(target_schema: &str, target_name: &str) -> RawStep {
        RawStep {
            step_no: 1,
            kind: StepKind::RefreshMaterializedView,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![qn(target_schema, target_name)],
            sql: format!("REFRESH MATERIALIZED VIEW {target_schema}.{target_name};"),
            transactional: TransactionConstraint::InTransaction,
        }
    }

    #[test]
    fn suppressed_step_matches_kind_and_target() {
        let step = refresh_step("app", "daily_revenue");
        let overrides = vec![StepOverride {
            kind: "refresh_materialized_view".to_string(),
            target: "app.daily_revenue".to_string(),
            suppress: true,
        }];
        assert!(is_step_suppressed(&step, &overrides));
    }

    #[test]
    fn non_suppressed_override_does_not_suppress() {
        let step = refresh_step("app", "daily_revenue");
        let overrides = vec![StepOverride {
            kind: "refresh_materialized_view".to_string(),
            target: "app.daily_revenue".to_string(),
            suppress: false,
        }];
        assert!(!is_step_suppressed(&step, &overrides));
    }

    #[test]
    fn kind_mismatch_does_not_suppress() {
        let step = refresh_step("app", "daily_revenue");
        let overrides = vec![StepOverride {
            kind: "create_materialized_view".to_string(),
            target: "app.daily_revenue".to_string(),
            suppress: true,
        }];
        assert!(!is_step_suppressed(&step, &overrides));
    }

    #[test]
    fn target_mismatch_does_not_suppress() {
        let step = refresh_step("app", "daily_revenue");
        let overrides = vec![StepOverride {
            kind: "refresh_materialized_view".to_string(),
            target: "app.other_mv".to_string(),
            suppress: true,
        }];
        assert!(!is_step_suppressed(&step, &overrides));
    }

    #[test]
    fn empty_overrides_does_not_suppress() {
        let step = refresh_step("app", "daily_revenue");
        assert!(!is_step_suppressed(&step, &[]));
    }

    #[test]
    fn step_with_no_targets_is_not_suppressed() {
        let step = RawStep {
            step_no: 1,
            kind: StepKind::RefreshMaterializedView,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: "REFRESH MATERIALIZED VIEW app.daily_revenue;".to_string(),
            transactional: TransactionConstraint::InTransaction,
        };
        let overrides = vec![StepOverride {
            kind: "refresh_materialized_view".to_string(),
            target: "app.daily_revenue".to_string(),
            suppress: true,
        }];
        assert!(!is_step_suppressed(&step, &overrides));
    }
}
