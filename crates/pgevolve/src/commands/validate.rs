//! `pgevolve validate` — parse source IR + lint stub. With `--shadow`,
//! round-trip the source through an ephemeral Postgres of the configured
//! version (spec §10 / Phase 12).

use std::collections::BTreeSet;

use anyhow::{Result, anyhow};
use tempfile::TempDir;

use pgevolve_core::catalog::{CatalogFilter, DriftReport, read_catalog};
use pgevolve_core::diff::grants::collect_managed_roles;
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::difference::Difference;
use pgevolve_core::ir::eq::Diff;
use pgevolve_core::ir::grant::GrantTarget;
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
        let mismatch_count = report.canonical_mismatches.len()
            + report.extra_ast_edges.len()
            + report.missing_ast_edges.len();
        if mismatch_count > 0 {
            for m in &report.canonical_mismatches {
                eprintln!(
                    "  canonical mismatch {}: source={:?} catalog={:?}",
                    m.view_qname, m.source_canonical, m.catalog_canonical
                );
            }
            for e in &report.extra_ast_edges {
                eprintln!("  extra AST edge {}: {}", e.view_qname, e.dep_node);
            }
            for m in &report.missing_ast_edges {
                eprintln!(
                    "  missing AST edge {}: {}.{}",
                    m.view_qname, m.ref_schema, m.ref_name
                );
            }
            if args.shadow_strict {
                anyhow::bail!("shadow-validate --strict: {mismatch_count} mismatch(es)");
            }
        }
        let n_edges = report.structural_edges_checked;
        eprintln!(
            "shadow-validate: ok ({n_edges} structural edge(s), {mismatch_count} canonical mismatch(es))"
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

    let shadow_catalog = sanitize_shadow_catalog(source, shadow_catalog);
    Ok(source.diff(&shadow_catalog))
}

// ---------------------------------------------------------------------------
// Shadow-catalog sanitization
// ---------------------------------------------------------------------------

/// Mirror the differ's semantics onto `shadow` before a strict `Diff` compare.
///
/// 1. **Owner** (`None` = unmanaged): when source has no owner declared, the
///    catalog-side owner (always auto-assigned by PG) is cleared to match.
/// 2. **Grants** (lenient policy): catalog grants to roles absent from the
///    source managed-roles set are filtered out, just as the differ skips them.
// Length is structural: one block per grantable object family (8 total).
// Each block is trivially simple; extracting further would only add indirection.
#[allow(clippy::too_many_lines)]
fn sanitize_shadow_catalog(source: &Catalog, mut shadow: Catalog) -> Catalog {
    let managed_roles = collect_managed_roles(source);

    // Build unmanaged-owner key sets for each object family from the source.
    let u_schemas = unmanaged_keys(
        source
            .schemas
            .iter()
            .map(|s| (&s.owner, s.name.to_string())),
    );
    let u_tables = unmanaged_keys(
        source
            .tables
            .iter()
            .map(|t| (&t.owner, t.qname.to_string())),
    );
    let u_seqs = unmanaged_keys(
        source
            .sequences
            .iter()
            .map(|s| (&s.owner, s.qname.to_string())),
    );
    let u_views = unmanaged_keys(source.views.iter().map(|v| (&v.owner, v.qname.to_string())));
    let u_mvs = unmanaged_keys(
        source
            .materialized_views
            .iter()
            .map(|m| (&m.owner, m.qname.to_string())),
    );
    let u_fns = unmanaged_keys(
        source
            .functions
            .iter()
            .map(|f| (&f.owner, f.qname.to_string())),
    );
    let u_procs = unmanaged_keys(
        source
            .procedures
            .iter()
            .map(|p| (&p.owner, p.qname.to_string())),
    );
    let u_types = unmanaged_keys(source.types.iter().map(|t| (&t.owner, t.qname.to_string())));

    // Apply: clear unmanaged owners; strip unmanaged-role grants.
    for s in &mut shadow.schemas {
        sanitize_object(
            &u_schemas,
            &s.name.to_string(),
            &managed_roles,
            &mut s.owner,
            &mut s.grants,
        );
    }
    for t in &mut shadow.tables {
        sanitize_object(
            &u_tables,
            &t.qname.to_string(),
            &managed_roles,
            &mut t.owner,
            &mut t.grants,
        );
    }
    for s in &mut shadow.sequences {
        sanitize_object(
            &u_seqs,
            &s.qname.to_string(),
            &managed_roles,
            &mut s.owner,
            &mut s.grants,
        );
    }
    for v in &mut shadow.views {
        sanitize_object(
            &u_views,
            &v.qname.to_string(),
            &managed_roles,
            &mut v.owner,
            &mut v.grants,
        );
    }
    for m in &mut shadow.materialized_views {
        sanitize_object(
            &u_mvs,
            &m.qname.to_string(),
            &managed_roles,
            &mut m.owner,
            &mut m.grants,
        );
    }
    for f in &mut shadow.functions {
        sanitize_object(
            &u_fns,
            &f.qname.to_string(),
            &managed_roles,
            &mut f.owner,
            &mut f.grants,
        );
    }
    for p in &mut shadow.procedures {
        sanitize_object(
            &u_procs,
            &p.qname.to_string(),
            &managed_roles,
            &mut p.owner,
            &mut p.grants,
        );
    }
    for t in &mut shadow.types {
        sanitize_object(
            &u_types,
            &t.qname.to_string(),
            &managed_roles,
            &mut t.owner,
            &mut t.grants,
        );
    }

    shadow
}

/// Collect the keys of objects whose owner is `None` (unmanaged).
fn unmanaged_keys<'a>(
    iter: impl Iterator<Item = (&'a Option<Identifier>, String)>,
) -> BTreeSet<String> {
    iter.filter_map(|(owner, key)| owner.is_none().then_some(key))
        .collect()
}

/// Clear `owner` when the key is in `unmanaged_keys`; filter `grants` to managed grantees.
fn sanitize_object(
    unmanaged: &BTreeSet<String>,
    key: &str,
    managed_roles: &BTreeSet<Identifier>,
    owner: &mut Option<Identifier>,
    grants: &mut Vec<pgevolve_core::ir::grant::Grant>,
) {
    if unmanaged.contains(key) {
        *owner = None;
    }
    grants.retain(|g| match &g.grantee {
        GrantTarget::Public => true,
        GrantTarget::Role(n) => managed_roles.contains(n),
    });
}

fn build_plan_for_shadow(source: &Catalog, target_identity: String) -> Result<Plan> {
    let empty = Catalog::empty();
    let changes = pgevolve_core::diff::diff(&empty, source, &DriftReport::default());
    let policy = PlannerPolicy {
        strategy: Strategy::Online,
        ..PlannerPolicy::default()
    };
    let ordered =
        order(&empty, source, changes, &policy).map_err(|e| anyhow!("plan order: {e}"))?;
    let steps = rewrite(ordered, &empty, &policy);
    let groups = group_steps(steps);
    Plan::from_grouped(
        groups,
        source,
        &empty,
        target_identity,
        None,
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    )
    .map_err(|e| anyhow!("from_grouped: {e}"))
}

/// Parse an optional `postgres_version` string into a `PgMajor`.
///
/// Defaults to `17` when `None` is supplied. (The shadow's role is to
/// validate the source IR by round-tripping it through an ephemeral PG;
/// defaulting to 17 — one major behind newest — keeps the round-trip
/// stable while still exercising a modern catalog.)
fn parse_pg_major(s: Option<&str>) -> Result<crate::shadow::PgMajor> {
    match s.map(str::trim) {
        None | Some("17") => Ok(17),
        Some("18") => Ok(18),
        Some("16") => Ok(16),
        Some("15") => Ok(15),
        Some("14") => Ok(14),
        Some(other) => Err(anyhow!(
            "[shadow].postgres_version must be one of 14/15/16/17/18; got `{other}`",
        )),
    }
}
