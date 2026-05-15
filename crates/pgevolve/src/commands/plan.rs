//! `pgevolve plan` — full pipeline: parse → introspect → diff → order →
//! rewrite → group → write plan directory.

use std::path::PathBuf;

use anyhow::Result;

use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::diff::diff;
use pgevolve_core::plan::{Plan, PlannerPolicy, group_steps, order, rewrite, write_plan_dir};

use crate::cli::PlanArgs;
use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};
use crate::pg_querier::PgCatalogQuerier;
use crate::target_identity::compute_target_identity;

/// Run `pgevolve plan`.
pub async fn run(args: PlanArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let source = pgevolve_core::parse::parse_directory(&cfg.project.schema_dir, &[])
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    let client = connect(&opts).await?;
    let target_identity = compute_target_identity(&client).await?;

    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(opts.managed_schemas.clone(), opts.ignore_objects.clone())?;
    let (target, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| anyhow::anyhow!("join error: {e}"))??;

    let changes = diff(&target, &source, &drift);
    let ordered = order(&target, &source, changes)?;

    let policy = PlannerPolicy {
        strategy: opts.strategy,
        online: PlannerPolicy::default().online,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(ordered, &target, &policy);
    let groups = group_steps(steps);
    let plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        target_identity,
        detect_git_rev().ok(),
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    );

    let out_dir = args.output.unwrap_or_else(|| default_plan_dir(cfg, &plan));
    write_plan_dir(&plan, &out_dir)?;
    println!(
        "Wrote plan {} to {} ({} group(s), {} step(s), {} intent(s))",
        plan.id.short(),
        out_dir.display(),
        plan.groups.len(),
        plan.groups.iter().map(|g| g.steps.len()).sum::<usize>(),
        plan.intents.len(),
    );
    Ok(0)
}

fn default_plan_dir(cfg: &PgevolveConfig, plan: &Plan) -> PathBuf {
    let date = plan.metadata.created_at.date().to_string(); // YYYY-MM-DD per time crate Display
    cfg.project
        .plan_dir
        .join(format!("{date}-{}", plan.id.short()))
}

fn detect_git_rev() -> std::io::Result<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(format!("git:{s}"))
    } else {
        Err(std::io::Error::other("git rev-parse failed"))
    }
}
