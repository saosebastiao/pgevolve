//! `pgevolve plan` — full pipeline: parse → introspect → diff → order →
//! rewrite → group → write plan directory.

use std::path::PathBuf;

use anyhow::Result;

use pgevolve_core::plan::{LintWaiver, Plan, PlannerPolicy, write_plan_dir};

use crate::cli::PlanArgs;
use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};

/// Run `pgevolve plan`.
pub async fn run(args: PlanArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let client = connect(&opts).await?;

    // Load pre-existing lint waivers from a previous run's intent.toml,
    // if the user supplied an --output dir. (When no --output is given
    // we don't yet know the default plan dir — it's derived from the
    // plan id — so we skip pre-loading; the user can always re-run with
    // an explicit --output once they've authored waivers.)
    let existing_lint_waivers = args
        .output
        .as_deref()
        .map_or_else(Vec::new, load_existing_waivers);

    let build_opts = crate::api::BuildPlanOptions {
        managed_schemas: opts.managed_schemas.clone(),
        ignore_objects: opts.ignore_objects.clone(),
        strategy: opts.strategy,
        planner_ruleset_version: PlannerPolicy::default().planner_ruleset_version,
        existing_lint_waivers,
        source_rev: detect_git_rev().ok(),
    };

    let plan = match crate::api::build_plan(&cfg.project.schema_dir, client, build_opts).await {
        Ok(p) => p,
        Err(crate::api::BuildPlanError::LintAtPlanRequiresWaiver(msg)) => {
            eprintln!("pgevolve plan: refusing to plan due to unwaived LintAtPlan findings:");
            eprintln!("  {msg}");
            eprintln!();
            eprintln!(
                "Resolve by correcting the source schema, or add `[[lint_waiver]]` rows to your intent.toml."
            );
            return Ok(2);
        }
        Err(e) => return Err(anyhow::anyhow!("{e}")),
    };

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

    if args.shadow_validate {
        let source = pgevolve_core::parse::parse_directory(&cfg.project.schema_dir, &[])
            .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;
        run_shadow_cross_check(&source, cfg, args.shadow_strict).await?;
    }

    Ok(0)
}

/// Run the `--shadow-validate` cross-check (arch spec Decision 12).
///
/// Resolves the backend from the `[shadow]` config, calls [`cross_check`],
/// and prints / propagates any warnings or errors.
///
/// [`cross_check`]: crate::shadow::validate::cross_check
async fn run_shadow_cross_check(
    source: &pgevolve_core::ir::catalog::Catalog,
    cfg: &PgevolveConfig,
    strict: bool,
) -> anyhow::Result<()> {
    let shadow_cfg = cfg.shadow.as_ref().ok_or_else(|| {
        anyhow::anyhow!("--shadow-validate requires a [shadow] section in pgevolve.toml")
    })?;
    let backend = crate::shadow::resolve(shadow_cfg)?;
    // v0.1: default to PG 17. v0.2 will thread the real major from the
    // live DB connection or from [shadow].postgres_version.
    let major = shadow_cfg
        .postgres_version
        .as_deref()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(17);
    let report =
        crate::shadow::validate::cross_check(backend.as_ref(), source, major, strict).await?;
    eprintln!(
        "shadow-validate: {} structural edge(s) checked",
        report.structural_edges_checked
    );
    if !report.warnings.is_empty() {
        eprintln!("shadow-validate: {} warning(s):", report.warnings.len());
        for w in &report.warnings {
            eprintln!("  - {w}");
        }
        if strict {
            anyhow::bail!("shadow-validate --strict: warnings treated as errors");
        }
    }
    if !report.errors.is_empty() {
        for e in &report.errors {
            eprintln!("  - {e}");
        }
        anyhow::bail!("shadow-validate: {} error(s)", report.errors.len());
    }
    Ok(())
}

/// Load `[[lint_waiver]]` rows from an existing `intent.toml` in `dir`, if
/// one exists. Returns an empty vec if the file is absent or unparseable.
fn load_existing_waivers(dir: &std::path::Path) -> Vec<LintWaiver> {
    let path = dir.join("intent.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match pgevolve_core::plan::read_intent_toml(&text) {
        Ok(parsed) => parsed.lint_waivers,
        Err(_) => Vec::new(),
    }
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
