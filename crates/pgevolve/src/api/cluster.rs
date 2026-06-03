//! Cluster-level library entry points.
//!
//! Provides [`build_cluster_plan`], which runs the full parse → catalog read
//! → diff → lint → emit pipeline for cluster-level role management.
//!
//! Stage 9 of the cluster-roles implementation plan.

use std::path::Path;
use std::sync::Arc;

use pgevolve_core::catalog::cluster::read_cluster_catalog;
use pgevolve_core::diff::cluster::{ClusterChangeSet, diff_cluster};
use pgevolve_core::ir::cluster::catalog::ClusterCatalog;
use pgevolve_core::lint::Finding;
use pgevolve_core::lint::universal::check_cluster_changeset;
use pgevolve_core::parse::cluster::parse_cluster_directory;
use pgevolve_core::plan::cluster_rewrite::emit_cluster_changes;
use pgevolve_core::plan::raw_step::RawStep;

use crate::cluster_config::ClusterConfig;
use crate::pg_querier::PgCatalogQuerier;

/// Output of building a cluster plan.
///
/// Carries the computed steps plus the intermediate pipeline stages so
/// callers (CLI, conformance tests) can inspect findings and the full diff.
///
/// `advisory_findings` is populated by [`check_cluster_changeset`] and must be
/// surfaced to the user — the CLI (Stage 10) iterates it and prints each
/// finding to stderr.
#[derive(Debug)]
pub struct ClusterPlan {
    /// DDL steps to execute, in emission order.
    pub steps: Vec<RawStep>,
    /// Desired cluster state (parsed from `roles/*.sql`).
    pub source: ClusterCatalog,
    /// Current live cluster state (read from `pg_authid`).
    pub target: ClusterCatalog,
    /// Full changeset produced by the differ.
    pub changes: ClusterChangeSet,
    /// Advisory lint findings. Non-blocking but must be shown to the user.
    ///
    /// CRITICAL: this field is always populated by
    /// [`check_cluster_changeset`] — do not skip the lint call.
    pub advisory_findings: Vec<Finding>,
}

/// Errors raised by [`build_cluster_plan`].
#[derive(Debug, thiserror::Error)]
pub enum ClusterPlanError {
    /// `parse_cluster_directory` rejected the `roles/` source files.
    #[error("parse error: {0}")]
    Parse(#[from] pgevolve_core::parse::error::ParseError),
    /// `read_cluster_catalog` failed against the live database.
    #[error("catalog read error: {0}")]
    Catalog(#[from] pgevolve_core::catalog::error::CatalogError),
    /// Could not open a connection to Postgres.
    #[error("connection error: {0}")]
    Connection(String),
}

impl ClusterPlan {
    /// Materialize a serializable `Plan` from this cluster plan.
    ///
    /// `plan_id` is computed externally (cluster plan ids hash
    /// `ClusterCatalog`, not per-DB `Catalog` — see the existing
    /// `compute_cluster_plan_id` in `commands/cluster/plan.rs`).
    ///
    /// `target_identity` should be the cluster identity returned by
    /// [`crate::target_identity::compute_cluster_target_identity`].
    ///
    /// The resulting `Plan` can be written via
    /// `pgevolve_core::plan::write_plan_dir` and read back via
    /// `pgevolve_core::plan::read_plan_dir`.
    ///
    /// # Errors
    ///
    /// Propagates any error from
    /// [`pgevolve_core::plan::Plan::from_grouped_with_id`].
    pub fn to_plan(
        self,
        plan_id: pgevolve_core::plan::PlanId,
        target_identity: String,
    ) -> Result<pgevolve_core::plan::Plan, pgevolve_core::plan::PlanError> {
        let groups = pgevolve_core::plan::group_steps(self.steps);
        pgevolve_core::plan::Plan::from_grouped_with_id(
            groups,
            plan_id,
            target_identity,
            None, // source_rev: cluster plans don't currently carry source_rev
            pgevolve_core::VERSION,
            pgevolve_core::plan::PlannerPolicy::default().planner_ruleset_version,
        )
    }
}

/// Build a cluster plan: parse `roles/`, read live cluster, diff, lint, emit.
///
/// Steps:
/// 1. Parse every `*.sql` file under `<project_root>/roles/` into a
///    [`ClusterCatalog`] (the desired state).
/// 2. Connect to Postgres using `cfg.connection.dsn` and read the live cluster
///    state via [`read_cluster_catalog`] (the current state).
/// 3. Diff desired ← current to produce a [`ClusterChangeSet`].
/// 4. **CRITICAL:** Run [`check_cluster_changeset`] to collect advisory lint
///    findings and surface them in [`ClusterPlan::advisory_findings`]. Without
///    this step, lint rules are dead code (v0.2.1 bug).
/// 5. Emit DDL [`RawStep`]s from the changeset.
///
/// The caller is responsible for showing advisory findings to the user. The
/// CLI (Stage 10) does this by iterating `ClusterPlan::advisory_findings` and
/// printing each finding to stderr before presenting the plan.
pub async fn build_cluster_plan(
    project_root: &Path,
    cfg: &ClusterConfig,
) -> Result<ClusterPlan, ClusterPlanError> {
    // --- Step 1: parse source ---
    let roles_dir = project_root.join("roles");
    let source = parse_cluster_directory(&roles_dir)?;

    // --- Step 2: connect + read live catalog ---
    let (client, connection) = tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
        .await
        .map_err(|e| ClusterPlanError::Connection(e.to_string()))?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster catalog connection task ended");
        }
    });

    let bootstrap_roles: Vec<String> = cfg.bootstrap.roles.clone();
    let querier = PgCatalogQuerier::from_arc(Arc::new(client))
        .map_err(|e| ClusterPlanError::Connection(e.to_string()))?;

    let target =
        tokio::task::spawn_blocking(move || read_cluster_catalog(&querier, &bootstrap_roles))
            .await
            .map_err(|e| ClusterPlanError::Connection(format!("join error: {e}")))?
            .map_err(ClusterPlanError::Catalog)?;

    // --- Step 3: diff ---
    let changes = diff_cluster(&target, &source);

    // --- Step 4: lint (CRITICAL — must not be skipped) ---
    // check_cluster_changeset fires role-loses-superuser and
    // role-membership-cycle rules. The findings are advisory (non-blocking),
    // but the user must see them. They flow to ClusterPlan::advisory_findings
    // here and the CLI prints them to stderr in Stage 10.
    let advisory_findings = check_cluster_changeset(&source, &changes);

    // --- Step 5: emit steps ---
    let steps = emit_cluster_changes(&changes);

    Ok(ClusterPlan {
        steps,
        source,
        target,
        changes,
        advisory_findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    fn synthetic_create_role(name: &str) -> RawStep {
        RawStep {
            step_no: 0,
            kind: StepKind::CreateRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: format!("CREATE ROLE {name};"),
            transactional: TransactionConstraint::InTransaction,
        }
    }

    fn synthetic_drop_role(name: &str) -> RawStep {
        RawStep {
            step_no: 0,
            kind: StepKind::DropRole,
            destructive: true,
            destructive_reason: Some(format!("drops role {name} (may orphan objects)")),
            intent_id: None,
            targets: vec![],
            sql: format!("DROP ROLE {name};"),
            transactional: TransactionConstraint::InTransaction,
        }
    }

    fn empty_changes() -> pgevolve_core::diff::cluster::ClusterChangeSet {
        pgevolve_core::diff::cluster::ClusterChangeSet::default()
    }

    fn empty_plan_id() -> pgevolve_core::plan::PlanId {
        // Build via PlanId::compute against empty catalogs as a deterministic
        // placeholder for unit tests.
        pgevolve_core::plan::PlanId::compute(
            &pgevolve_core::ir::catalog::Catalog::empty(),
            &pgevolve_core::ir::catalog::Catalog::empty(),
            "0.0.0-test",
            0,
        )
        .expect("compute placeholder id")
    }

    #[test]
    fn to_plan_assigns_step_numbers_and_intents() {
        let plan = ClusterPlan {
            steps: vec![synthetic_create_role("a"), synthetic_drop_role("b")],
            source: ClusterCatalog::empty(),
            target: ClusterCatalog::empty(),
            changes: empty_changes(),
            advisory_findings: vec![],
        };

        let core_plan = plan
            .to_plan(empty_plan_id(), "cluster:0000000000003039".into())
            .expect("to_plan ok");

        // One group with both steps (InTransaction).
        assert_eq!(core_plan.groups.len(), 1);
        assert_eq!(core_plan.groups[0].steps.len(), 2);
        // Step numbers assigned in emission order.
        assert_eq!(core_plan.groups[0].steps[0].step_no, 1);
        assert_eq!(core_plan.groups[0].steps[1].step_no, 2);
        // One intent for the drop.
        assert_eq!(core_plan.intents.len(), 1);
        assert_eq!(core_plan.intents[0].step, 2);
        assert!(!core_plan.intents[0].approved);
        // target_identity passed through.
        assert_eq!(
            core_plan.metadata.target_identity,
            "cluster:0000000000003039"
        );
    }

    #[test]
    fn to_plan_empty_steps_produces_empty_plan() {
        let plan = ClusterPlan {
            steps: vec![],
            source: ClusterCatalog::empty(),
            target: ClusterCatalog::empty(),
            changes: empty_changes(),
            advisory_findings: vec![],
        };

        let core_plan = plan
            .to_plan(empty_plan_id(), "cluster:empty".into())
            .expect("empty to_plan ok");

        assert!(core_plan.groups.is_empty());
        assert!(core_plan.intents.is_empty());
    }
}
