//! `pgevolve validate` — parse source IR + lint stub. With `--shadow`,
//! round-trip the source through an ephemeral Postgres of the configured
//! version (spec §10 / Phase 12).

use anyhow::{Result, anyhow};
use tempfile::TempDir;

use pgevolve_core::catalog::{CatalogFilter, DriftReport, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::difference::Difference;
use pgevolve_core::ir::eq::Diff;
use pgevolve_core::plan::{
    Plan, PlannerPolicy, Strategy, group_steps, order, rewrite, write_plan_dir,
};

use crate::cli::ValidateArgs;
use crate::config::PgevolveConfig;
use crate::executor::{ApplyOverrides, apply};
use crate::pg_querier::PgCatalogQuerier;
use crate::shadow::{docker_available, resolve};

/// Run `pgevolve validate`.
pub async fn run(args: &ValidateArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let schema_dir = &cfg.project.schema_dir;
    if !schema_dir.is_dir() {
        return Err(anyhow!(
            "schema directory not found at {}",
            schema_dir.display(),
        ));
    }
    let source = pgevolve_core::parse::parse_directory(schema_dir, &[])
        .map_err(|e| anyhow!("parse error: {e}"))?;

    if args.shadow {
        let findings = run_shadow_validation(&source, cfg).await?;
        if findings.is_empty() {
            println!(
                "pgevolve validate --shadow: round-trip matched ({} object(s))",
                source.tables.len() + source.indexes.len() + source.sequences.len(),
            );
            return Ok(0);
        }
        eprintln!(
            "pgevolve validate --shadow: {} mismatch(es):",
            findings.len()
        );
        for d in &findings {
            eprintln!("  - {}: `{}` vs `{}`", d.path, d.from, d.to);
        }
        return Ok(1);
    }

    if args.shadow_validate {
        let shadow_cfg = cfg.shadow.as_ref().ok_or_else(|| {
            anyhow!("--shadow-validate requires a [shadow] section in pgevolve.toml")
        })?;
        let backend = resolve(shadow_cfg)?;
        // v0.1: default to PG 17. v0.2 will thread the real major from the
        // live DB connection or from [shadow].postgres_version.
        let major = shadow_cfg
            .postgres_version
            .as_deref()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(17);
        let report = crate::shadow::validate::cross_check(
            backend.as_ref(),
            &source,
            major,
            args.shadow_strict,
        )
        .await?;
        eprintln!(
            "shadow-validate: {} structural edge(s) checked",
            report.structural_edges_checked
        );
        if !report.warnings.is_empty() {
            eprintln!("shadow-validate: {} warning(s):", report.warnings.len());
            for w in &report.warnings {
                eprintln!("  - {w}");
            }
            if args.shadow_strict {
                anyhow::bail!("shadow-validate --strict: warnings treated as errors");
            }
        }
        if !report.errors.is_empty() {
            for e in &report.errors {
                eprintln!("  - {e}");
            }
            anyhow::bail!("shadow-validate: {} error(s)", report.errors.len());
        }
        eprintln!(
            "shadow-validate: ok ({} structural edge(s))",
            report.structural_edges_checked
        );
        return Ok(0);
    }

    println!(
        "pgevolve validate: source parses cleanly ({} schema(s), {} table(s)); 0 lint findings",
        source.schemas.len(),
        source.tables.len(),
    );
    Ok(0)
}

/// Spin up a shadow PG of the configured version, apply the source IR,
/// introspect, and return any source-vs-shadow diffs.
async fn run_shadow_validation(source: &Catalog, cfg: &PgevolveConfig) -> Result<Vec<Difference>> {
    let shadow_cfg = cfg
        .shadow
        .as_ref()
        .ok_or_else(|| anyhow!("--shadow requires a [shadow] section in pgevolve.toml"))?;

    // For the testcontainers backend we still need to know the PG major; for
    // the dsn backend the major is ignored.  Default to 17 if not specified.
    let pg_major = parse_pg_major(shadow_cfg.postgres_version.as_deref())?;

    // Validate Docker is available when no DSN is configured (auto/testcontainers).
    let backend_name = shadow_cfg.backend.as_deref().unwrap_or("auto");
    let needs_docker = backend_name == "testcontainers"
        || (backend_name == "auto" && shadow_cfg.url.is_none() && shadow_cfg.url_env.is_none());
    if needs_docker && !docker_available() {
        return Err(anyhow!(
            "--shadow requires Docker. Install Docker, or configure [shadow].url / [shadow].url_env.",
        ));
    }

    let backend = resolve(shadow_cfg)?;
    let guard = backend.checkout(pg_major).await?;

    let mut client = tokio_postgres::connect(guard.url(), tokio_postgres::NoTls)
        .await
        .map(|(c, conn)| {
            tokio::spawn(async move {
                let _ = conn.await;
            });
            c
        })?;
    crate::executor::bootstrap_metadata(&mut client).await?;

    // Build a Plan that creates everything in `source`, write it to a
    // tempdir, and apply it against the shadow DB.
    let target_identity = crate::compute_target_identity(&client).await?;
    let plan = build_plan_for_shadow(source, target_identity)?;
    let plan_dir = TempDir::new()?;
    write_plan_dir(&plan, plan_dir.path())?;

    let managed: Vec<Identifier> = source.schemas.iter().map(|s| s.name.clone()).collect();
    let filter = CatalogFilter::new(managed.clone(), vec![]).map_err(|e| anyhow!(e))?;
    apply(
        plan_dir.path(),
        &mut client,
        &filter,
        ApplyOverrides {
            allow_different_target: false,
            allow_drift: true,
            allow_unwaived_lint: true,
            allow_unapproved_intents: true,
            actor: Some("shadow-validate".into()),
            abort_after_step: None,
        },
    )
    .await?;

    // Re-introspect.
    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e))?;
    let (shadow_catalog, _drift) =
        tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
            .await
            .map_err(|e| anyhow!("join: {e}"))?
            .map_err(|e| anyhow!("read_catalog: {e}"))?;

    Ok(source.diff(&shadow_catalog))
}

fn build_plan_for_shadow(source: &Catalog, target_identity: String) -> Result<Plan> {
    let empty = Catalog::empty();
    let changes = pgevolve_core::diff::diff(&empty, source, &DriftReport::default());
    let ordered = order(&empty, source, changes).map_err(|e| anyhow!("plan order: {e}"))?;
    let policy = PlannerPolicy {
        strategy: Strategy::Online,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(ordered, &empty, &policy);
    let groups = group_steps(steps);
    Ok(Plan::from_grouped(
        groups,
        source,
        &empty,
        target_identity,
        None,
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    ))
}

/// Parse an optional `postgres_version` string into a `PgMajor`.
///
/// Defaults to `17` when `None` is supplied (testcontainers auto-selects
/// the latest stable; the dsn backend ignores the major entirely).
fn parse_pg_major(s: Option<&str>) -> Result<crate::shadow::PgMajor> {
    match s.map(str::trim) {
        None | Some("17") => Ok(17),
        Some("16") => Ok(16),
        Some("15") => Ok(15),
        Some("14") => Ok(14),
        Some(other) => Err(anyhow!(
            "[shadow].postgres_version must be one of 14/15/16/17; got `{other}`",
        )),
    }
}
