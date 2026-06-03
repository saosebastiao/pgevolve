//! Cluster-flavored apply preflight.
//!
//! Mirrors the per-DB `executor::preflight` but checks cluster identity (via
//! `pg_control_system().system_identifier`) instead of per-DB identity, and
//! does not perform a drift recheck (see design doc §3.1).
//!
//! Manifest cross-check is already enforced by `read_plan_dir`, which
//! validates that `plan.sql`, `intent.toml`, and `manifest.toml` carry the
//! same `plan_id`. No extra check is needed here.

use tokio_postgres::Client;

use pgevolve_core::plan::Plan;

use crate::executor::ApplyError;
use crate::target_identity::compute_cluster_target_identity;

/// Overrides for [`run_cluster_preflight`]. Mirrors `PreflightOverrides` but
/// only carries the flags that apply to cluster ops.
#[derive(Debug, Clone, Default)]
pub struct ClusterPreflightOverrides {
    /// Skip identity match. Use only when intentionally applying a plan to a
    /// different cluster.
    pub allow_different_target: bool,
    /// Skip intent approval. Set internally by test harnesses; not exposed
    /// via the CLI.
    pub allow_unapproved_intents: bool,
}

/// Run cluster-apply preflight against `client`.
///
/// Checks:
/// 1. Live cluster identity matches `plan.metadata.target_identity` (unless
///    `overrides.allow_different_target`).
/// 2. Every destructive intent in `plan.intents` has `approved = true` in
///    `intent.toml` (unless `overrides.allow_unapproved_intents`).
///
/// # Errors
///
/// Returns [`ApplyError::TargetIdentityMismatch`] if the live cluster identity
/// does not match the plan, or [`ApplyError::UnapprovedIntents`] if any
/// destructive intents have not been approved in `intent.toml`.
pub async fn run_cluster_preflight(
    client: &Client,
    plan: &Plan,
    overrides: ClusterPreflightOverrides,
) -> Result<(), ApplyError> {
    if !overrides.allow_different_target {
        let live = compute_cluster_target_identity(client).await?;
        if live != plan.metadata.target_identity {
            return Err(ApplyError::TargetIdentityMismatch {
                plan: plan.metadata.target_identity.clone(),
                live,
            });
        }
    }

    if !overrides.allow_unapproved_intents {
        check_intent_approval(plan)?;
    }

    Ok(())
}

/// Enforce that every `DestructiveIntent` in the plan has been approved.
///
/// Mirrors [`crate::executor::preflight::check_intent_approval`] but lives
/// here so cluster preflight does not depend on the full per-DB preflight
/// module (which pulls in drift-recheck logic irrelevant to cluster apply).
fn check_intent_approval(plan: &Plan) -> Result<(), ApplyError> {
    let unapproved: Vec<_> = plan
        .intents
        .iter()
        .filter(|i| !i.approved)
        .map(|i| (i.id, i.target.clone(), i.reason.clone()))
        .collect();

    if !unapproved.is_empty() {
        return Err(ApplyError::UnapprovedIntents {
            count: unapproved.len(),
            details: unapproved,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::ir::catalog::Catalog;
    use pgevolve_core::plan::{DestructiveIntent, Plan, PlanId, PlanMetadata};
    use time::OffsetDateTime;

    fn plan_with_identity_and_intents(identity: &str, intents: Vec<DestructiveIntent>) -> Plan {
        Plan {
            id: PlanId([0u8; 32]),
            groups: vec![],
            intents,
            lint_waivers: vec![],
            step_overrides: vec![],
            metadata: PlanMetadata {
                pgevolve_version: "0.0.0-test".into(),
                planner_ruleset_version: 0,
                source_rev: None,
                target_identity: identity.into(),
                target_snapshot: Catalog::empty(),
                created_at: OffsetDateTime::now_utc(),
                lint_at_plan_findings: vec![],
            },
            advisory_findings: vec![],
        }
    }

    #[test]
    fn unapproved_intent_rejected() {
        let intent = DestructiveIntent {
            id: 1,
            step: 1,
            kind: "drop_role".into(),
            target: "alice".into(),
            reason: "drops role alice".into(),
            approved: false,
        };
        let plan = plan_with_identity_and_intents("cluster:abc", vec![intent]);
        let result = check_intent_approval(&plan);
        assert!(
            matches!(result, Err(ApplyError::UnapprovedIntents { count: 1, .. })),
            "expected UnapprovedIntents(1), got: {result:?}"
        );
    }

    #[test]
    fn approved_intent_passes() {
        let intent = DestructiveIntent {
            id: 1,
            step: 1,
            kind: "drop_role".into(),
            target: "alice".into(),
            reason: "drops role alice".into(),
            approved: true,
        };
        let plan = plan_with_identity_and_intents("cluster:abc", vec![intent]);
        assert!(
            check_intent_approval(&plan).is_ok(),
            "expected Ok when all intents are approved"
        );
    }

    #[test]
    fn multiple_unapproved_intents_all_reported() {
        let intents = vec![
            DestructiveIntent {
                id: 1,
                step: 1,
                kind: "drop_role".into(),
                target: "alice".into(),
                reason: "drops role alice".into(),
                approved: false,
            },
            DestructiveIntent {
                id: 2,
                step: 2,
                kind: "drop_role".into(),
                target: "bob".into(),
                reason: "drops role bob".into(),
                approved: false,
            },
        ];
        let plan = plan_with_identity_and_intents("cluster:abc", intents);
        let result = check_intent_approval(&plan);
        assert!(
            matches!(result, Err(ApplyError::UnapprovedIntents { count: 2, .. })),
            "expected UnapprovedIntents(2), got: {result:?}"
        );
    }

    #[test]
    fn no_intents_passes() {
        let plan = plan_with_identity_and_intents("cluster:abc", vec![]);
        assert!(
            check_intent_approval(&plan).is_ok(),
            "expected Ok when there are no intents"
        );
    }
}
