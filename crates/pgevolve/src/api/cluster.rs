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
