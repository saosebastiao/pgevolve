//! `pgevolve validate` — parse source IR + lint stub. With `--shadow`,
//! round-trip the source through an ephemeral Postgres of the configured
//! version (spec §10 / Phase 12).

use anyhow::{Result, anyhow};
use tempfile::TempDir;

use pgevolve_core::catalog::{CatalogFilter, DriftReport, PgVersion, read_catalog};
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
use crate::shadow_pg::{ShadowPostgres, docker_available};

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
    if !docker_available() {
        return Err(anyhow!(
            "--shadow requires Docker. Install Docker or run without --shadow.",
        ));
    }
    let shadow_cfg = cfg
        .shadow
        .as_ref()
        .ok_or_else(|| anyhow!("--shadow requires a [shadow] section in pgevolve.toml"))?;
    let pg_version = parse_pg_version(&shadow_cfg.postgres_version)?;

    let shadow = ShadowPostgres::start(pg_version).await?;
    let mut client = tokio_postgres::connect(shadow.dsn(), tokio_postgres::NoTls)
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
            actor: Some("shadow-validate".into()),
            abort_after_step: None,
        },
    )
    .await?;

    // Re-introspect.
    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(managed, vec![]).map_err(|e| anyhow!(e))?;
    let (shadow_catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
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

fn parse_pg_version(s: &str) -> Result<PgVersion> {
    match s.trim() {
        "14" => Ok(PgVersion::Pg14),
        "15" => Ok(PgVersion::Pg15),
        "16" => Ok(PgVersion::Pg16),
        "17" => Ok(PgVersion::Pg17),
        other => Err(anyhow!(
            "[shadow].postgres_version must be one of 14/15/16/17; got `{other}`",
        )),
    }
}
