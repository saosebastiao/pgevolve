//! `pgevolve plan` — full pipeline: parse → introspect → diff → order →
//! rewrite → group → write plan directory.

use std::path::PathBuf;

use anyhow::Result;

use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::diff::diff;
use pgevolve_core::lint::Severity;
use pgevolve_core::lint::universal::run_drift_lints;
use pgevolve_core::plan::{
    LintWaiver, Plan, PlannerPolicy, group_steps, order, rewrite_with_source, write_plan_dir,
};

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
    let steps = rewrite_with_source(ordered, &target, &source, &policy);
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

    // Determine the output directory before writing so we can check for
    // pre-existing waivers in case this is a re-run after the user edited
    // intent.toml.
    let out_dir = args.output.unwrap_or_else(|| default_plan_dir(cfg, &plan));

    // --- Drift-lint gate (spec §12 / arch Decision 14) ---
    //
    // Run drift lints that compare source against target. Any `LintAtPlan`
    // finding must have a matching `[[lint_waiver]]` row in intent.toml (which
    // the user adds in a second pass after reading the error message).
    //
    // We load pre-existing waivers from an intent.toml that may already exist
    // in `out_dir` from a previous run — this is how the user provides waivers.
    let existing_waivers = load_existing_waivers(&out_dir);
    let drift_findings = run_drift_lints(&source, &target);
    let lint_at_plan: Vec<_> = drift_findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::LintAtPlan))
        .collect();

    if !lint_at_plan.is_empty() {
        let unwaived: Vec<_> = lint_at_plan
            .iter()
            .filter(|f| !waiver_matches(f, &existing_waivers))
            .collect();
        if !unwaived.is_empty() {
            eprintln!("pgevolve plan: refusing to plan due to unwaived LintAtPlan findings:");
            for f in &unwaived {
                eprintln!("  - [{}] ({}): {}", f.rule, f.severity, f.message);
            }
            eprintln!();
            eprintln!("Resolve by correcting the source schema, or add a `[[lint_waiver]]` row to");
            eprintln!("  {}", out_dir.join("intent.toml").display());
            eprintln!("and re-run `pgevolve plan`. Example waiver:");
            eprintln!();
            eprintln!("  [[lint_waiver]]");
            eprintln!("  rule   = \"{}\"", unwaived[0].rule);
            // Extract a plausible target from the beginning of the message
            // (findings always lead with "schema.table: …").
            let target_hint = unwaived[0]
                .message
                .split(':')
                .next()
                .unwrap_or("schema.table")
                .trim();
            eprintln!("  target = \"{target_hint}\"");
            eprintln!("  reason = \"<explain why this drift is acceptable>\"");
            return Ok(2);
        }
    }

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

/// Return `true` when `finding` is covered by at least one waiver in `waivers`.
///
/// A waiver matches when its `rule` equals the finding's rule AND its `target`
/// appears as a substring of the finding's message (findings always lead with
/// the qualified name of the affected object, e.g. `"app.users: …"`).
fn waiver_matches(finding: &pgevolve_core::lint::Finding, waivers: &[LintWaiver]) -> bool {
    waivers
        .iter()
        .any(|w| w.rule == finding.rule && finding.message.contains(&w.target))
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
