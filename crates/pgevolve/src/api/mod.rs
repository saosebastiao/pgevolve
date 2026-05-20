//! Library entry points for embedding pgevolve in other tools and tests.
//!
//! Today this module contains a single function, [`build_plan`], that runs
//! the full parse → introspect → diff → order → rewrite → group → assemble
//! pipeline and returns a [`Plan`] value. The CLI command
//! `pgevolve plan` is now a thin wrapper over this entry point plus
//! CLI-only UX (stdout, interactive waiver prompts, shadow validation).
//!
//! Conformance tests and test harnesses use this entry point directly to
//! avoid the cost of spawning the CLI binary per fixture.
//!
//! See `docs/superpowers/specs/2026-05-19-in-process-apply-runner-design.md`.

use std::path::Path;

use tokio_postgres::Client;

use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::diff::diff;
use pgevolve_core::identifier::Identifier;
use pgevolve_core::lint::Severity;
use pgevolve_core::lint::universal::run_drift_lints;
use pgevolve_core::plan::{
    LintWaiver, Plan, PlannerPolicy, RecordedFinding, Strategy, group_steps, order,
    rewrite_with_source,
};

use crate::pg_querier::PgCatalogQuerier;
use crate::target_identity::compute_target_identity;

/// Options for [`build_plan`].
///
/// These map to the equivalent fields in `pgevolve.toml` and the CLI
/// `pgevolve plan` invocation, but the caller supplies them directly
/// instead of reading from a config file.
#[derive(Debug, Clone)]
pub struct BuildPlanOptions {
    /// Schema names the catalog reader will include.
    pub managed_schemas: Vec<Identifier>,
    /// Glob patterns for objects to ignore inside managed schemas.
    pub ignore_objects: Vec<String>,
    /// Planner strategy (`Online` or `Atomic`).
    pub strategy: Strategy,
    /// Planner ruleset version stamped into the plan.
    ///
    /// Use `PlannerPolicy::default().planner_ruleset_version` unless
    /// you have a specific reason to override.
    pub planner_ruleset_version: u32,
    /// Pre-existing lint waivers (typically loaded from an existing
    /// `intent.toml` in the caller's plan directory). When empty, every
    /// `LintAtPlan` finding is treated as unwaived.
    pub existing_lint_waivers: Vec<LintWaiver>,
    /// Optional source-tree revision identifier stamped into the plan
    /// (e.g., `"git:abc1234"`).
    pub source_rev: Option<String>,
}

impl Default for BuildPlanOptions {
    fn default() -> Self {
        Self {
            managed_schemas: Vec::new(),
            ignore_objects: Vec::new(),
            strategy: Strategy::Online,
            planner_ruleset_version: PlannerPolicy::default().planner_ruleset_version,
            existing_lint_waivers: Vec::new(),
            source_rev: None,
        }
    }
}

/// Errors raised by [`build_plan`].
#[derive(Debug, thiserror::Error)]
pub enum BuildPlanError {
    /// `parse_directory` rejected the source schema.
    #[error("parse error: {0}")]
    Parse(String),
    /// `read_catalog` failed against the live database.
    #[error("catalog read error: {0}")]
    CatalogRead(String),
    /// The planner pipeline failed (typically `order`).
    #[error("planner error: {0}")]
    Planner(String),
    /// One or more `LintAtPlan` findings need explicit waivers in
    /// `existing_lint_waivers` before a plan can be built.
    #[error("unwaived LintAtPlan findings: {0}")]
    LintAtPlanRequiresWaiver(String),
    /// Lower-level connection / introspection failure.
    #[error("connection error: {0}")]
    Connection(String),
}

/// Build a [`Plan`] from a source schema directory and a live database.
///
/// Consumes `client`: the connection is moved into the catalog reader and
/// dropped when this function returns. Callers that need to apply the
/// plan should open a second `Client` for [`crate::executor::apply_plan`].
/// (This matches the CLI's behavior where `pgevolve plan` and
/// `pgevolve apply` are separate processes with separate connections.)
///
/// Mirrors the core pipeline of `pgevolve plan` but skips CLI-specific
/// behaviour: no `println!`, no interactive waiver prompts, no
/// `--shadow-validate`, no writing of the plan directory to disk.
pub async fn build_plan(
    schema_dir: &Path,
    client: Client,
    opts: BuildPlanOptions,
) -> Result<Plan, BuildPlanError> {
    let source = pgevolve_core::parse::parse_directory(schema_dir, &[])
        .map_err(|e| BuildPlanError::Parse(e.to_string()))?;

    let target_identity = compute_target_identity(&client)
        .await
        .map_err(|e| BuildPlanError::Connection(e.to_string()))?;

    let filter = CatalogFilter::new(opts.managed_schemas.clone(), opts.ignore_objects.clone())
        .map_err(|e| BuildPlanError::CatalogRead(e.to_string()))?;
    let querier =
        PgCatalogQuerier::new(client).map_err(|e| BuildPlanError::Connection(e.to_string()))?;
    let (target, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| BuildPlanError::Connection(format!("join error: {e}")))?
        .map_err(|e| BuildPlanError::CatalogRead(e.to_string()))?;

    let changes = diff(&target, &source, &drift);
    let policy = PlannerPolicy {
        strategy: opts.strategy,
        online: PlannerPolicy::default().online,
        ..PlannerPolicy::default()
    };
    let ordered = order(&target, &source, changes, &policy)
        .map_err(|e| BuildPlanError::Planner(e.to_string()))?;
    let steps = rewrite_with_source(ordered, &target, &source, &policy);
    let groups = group_steps(steps);
    let mut plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        target_identity,
        opts.source_rev.clone(),
        pgevolve_core::VERSION,
        opts.planner_ruleset_version,
    );

    // --- Drift-lint gate (mirrors commands::plan::run) ---
    let drift_findings = run_drift_lints(&source, &target);
    let lint_at_plan: Vec<_> = drift_findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::LintAtPlan))
        .collect();

    if !lint_at_plan.is_empty() {
        let unwaived: Vec<_> = lint_at_plan
            .iter()
            .filter(|f| !waiver_matches(f, &opts.existing_lint_waivers))
            .collect();
        if !unwaived.is_empty() {
            let msg = unwaived
                .iter()
                .map(|f| format!("[{}] ({}): {}", f.rule, f.severity, f.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(BuildPlanError::LintAtPlanRequiresWaiver(msg));
        }
        plan.metadata.lint_at_plan_findings = lint_at_plan
            .iter()
            .map(|f| {
                let target_str = f.message.split(':').next().unwrap_or("").trim().to_string();
                RecordedFinding {
                    rule: f.rule.to_string(),
                    target: target_str,
                    message: f.message.clone(),
                }
            })
            .collect();
    }

    Ok(plan)
}

fn waiver_matches(finding: &pgevolve_core::lint::Finding, waivers: &[LintWaiver]) -> bool {
    waivers
        .iter()
        .any(|w| w.rule == finding.rule && finding.message.contains(&w.target))
}
