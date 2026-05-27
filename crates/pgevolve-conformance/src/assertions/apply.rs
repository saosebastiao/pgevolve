//! Layer 4: apply roundtrip against ephemeral Postgres.
//!
//! Seeds `before.sql` directly into an `EphemeralPostgres` (bypassing
//! pgevolve), writes `after.sql` into a tempdir as the source schema,
//! drives the full plan + apply pipeline via the pgevolve library
//! entry points (`pgevolve::api::build_plan` and
//! `pgevolve::executor::apply_plan`), then introspects the post-apply
//! DB and compares the resulting IR against `after.sql` parsed
//! independently.
//!
//! Docker-gated. Skipped (not failed) when `docker_available()` is
//! false, consistent with the rest of the workspace.

use pgevolve::api::BuildPlanOptions;
use pgevolve::executor::ApplyOverrides;
use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::eq::Diff;
use pgevolve_core::parse::parse_directory;
use pgevolve_core::plan::Strategy;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, docker_available};
use pgevolve_testkit::pg_querier::PgCatalogQuerier;

use crate::fixture::Fixture;

/// Post-apply state available for downstream assertions (e.g. L5 minimality).
#[derive(Debug)]
pub struct PostApplyState {
    /// Introspected catalog immediately after apply succeeded.
    pub catalog: pgevolve_core::ir::catalog::Catalog,
    /// Drift report from the same introspection.
    pub drift: pgevolve_core::catalog::DriftReport,
    /// Parsed `after.sql` (or `post_apply_equals_to`) catalog — the source IR.
    pub after_source: pgevolve_core::ir::catalog::Catalog,
}

/// Outcome of an apply roundtrip.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Docker unavailable; layer skipped.
    Skipped,
    /// Apply succeeded; IRs were equal. Carries post-apply state for L5.
    Ok(Box<PostApplyState>),
    /// Apply was expected to fail, and it did fail with matching substrings.
    OkExpectedFailure,
    /// Apply succeeded but introspected IR diverged from after.sql.
    IrMismatch(String),
    /// `build_plan` or `apply_plan` failed.
    ApplyFailed {
        /// Error message from the failing call.
        stderr: String,
        /// "plan" or "apply".
        stage: &'static str,
    },
    /// The fixture expected `apply.succeeds = false` but apply succeeded.
    UnexpectedSuccess,
}

impl ApplyOutcome {
    /// True for any non-failure variant the runner should treat as pass.
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_) | Self::OkExpectedFailure | Self::Skipped)
    }
}

/// Options for Layer 4.
#[derive(Debug, Default, Clone, Copy)]
pub struct ApplyOptions {
    /// When `true`, flip every destructive intent's `approved` field to
    /// `true` after `build_plan` returns. Mirrors the user editing
    /// `intent.toml` and re-running `apply`.
    pub auto_approve_intents: bool,
}

/// Run Layer 4.
pub async fn check(fixture: &Fixture, pg_major: u32) -> anyhow::Result<ApplyOutcome> {
    check_with_options(fixture, pg_major, ApplyOptions::default()).await
}

/// Run Layer 4 with explicit options.
pub async fn check_with_options(
    fixture: &Fixture,
    pg_major: u32,
    opts: ApplyOptions,
) -> anyhow::Result<ApplyOutcome> {
    if !docker_available() {
        return Ok(ApplyOutcome::Skipped);
    }

    let version = pg_version_from_major(pg_major)?;
    let pg = EphemeralPostgres::start(version).await?;
    // Seed setup.sql first (infrastructure: roles, extensions, etc.) then
    // before.sql (the pgevolve-managed state to start from).
    if let Some(setup) = fixture.setup_sql.as_deref() {
        seed_before(&pg, setup).await?;
    }
    seed_before(&pg, &fixture.before_sql).await?;

    // Write `after.sql` into a tempdir; the parser walks a directory.
    let schema_tmp = tempfile::tempdir()?;
    std::fs::write(
        schema_tmp.path().join("0001-fixture.sql"),
        &fixture.after_sql,
    )?;

    // build_plan never touches the pgevolve metadata tables, so no
    // pre-bootstrap is required. apply_plan calls bootstrap_metadata as
    // its first step, mirroring the CLI's flow.
    //
    // build_plan consumes its client; apply_plan opens a fresh one.
    let build_client = pg.connect().await?;
    let build_opts = build_options_from_fixture(fixture)?;
    let mut plan =
        match pgevolve::api::build_plan(schema_tmp.path(), build_client, build_opts).await {
            Ok(p) => p,
            Err(e) => return Ok(check_failure_expectation(fixture, &e.to_string(), "plan")),
        };

    if opts.auto_approve_intents {
        plan.approve_all_intents();
    }

    let filter = filter_from_fixture(fixture)?;
    let overrides = ApplyOverrides::default();
    let mut apply_client = pg.connect().await?;
    match pgevolve::executor::apply_plan(&plan, &mut apply_client, &filter, overrides).await {
        Ok(_) => {}
        Err(e) => return Ok(check_failure_expectation(fixture, &e.to_string(), "apply")),
    }

    if !fixture.expect.apply.succeeds {
        return Ok(ApplyOutcome::UnexpectedSuccess);
    }

    let (post_apply_ir, post_apply_drift) = introspect_with_drift(&pg, fixture).await?;
    let expected_ir = parse_post_apply_target(fixture)?;

    // Use pgevolve-differ semantics to check equivalence: the source IR
    // (`expected_ir`, parsed from after.sql) may leave optional fields such
    // as `extension.version` or `extension.schema` as `None` to mean
    // "don't care". The strict `canonical_eq` would flag those as mismatches,
    // but the pgevolve differ correctly treats source-`None` as a wildcard.
    //
    // We compare `post_apply_ir` (target) vs `expected_ir` (source) using
    // the pgevolve diff, and declare them equivalent when the changeset is
    // empty (no further changes needed).
    let convergence_diff = pgevolve_core::diff::diff(
        &post_apply_ir,
        &expected_ir,
        &pgevolve_core::catalog::DriftReport::default(),
    );
    if convergence_diff.is_empty() {
        Ok(ApplyOutcome::Ok(Box::new(PostApplyState {
            catalog: post_apply_ir,
            drift: post_apply_drift,
            after_source: expected_ir,
        })))
    } else {
        // Fall back to the Diff-trait-based rendering for a human-readable
        // mismatch description.
        let diffs = expected_ir.diff(&post_apply_ir);
        let rendered = diffs
            .iter()
            .map(|d| format!("{}: {} -> {}", d.path, d.from, d.to))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ApplyOutcome::IrMismatch(rendered))
    }
}

fn pg_version_from_major(major: u32) -> anyhow::Result<pgevolve_core::catalog::PgVersion> {
    use pgevolve_core::catalog::PgVersion;
    match major {
        14 => Ok(PgVersion::Pg14),
        15 => Ok(PgVersion::Pg15),
        16 => Ok(PgVersion::Pg16),
        17 => Ok(PgVersion::Pg17),
        18 => Ok(PgVersion::Pg18),
        other => Err(anyhow::anyhow!("unsupported PG major: {other}")),
    }
}

async fn seed_before(pg: &EphemeralPostgres, before_sql: &str) -> anyhow::Result<()> {
    if before_sql.trim().is_empty() {
        return Ok(());
    }
    let client = pg.connect().await?;
    client.batch_execute(before_sql).await?;
    Ok(())
}

fn build_options_from_fixture(fixture: &Fixture) -> anyhow::Result<BuildPlanOptions> {
    let schemas: Vec<Identifier> = collect_managed_schemas(&fixture.after_sql)
        .into_iter()
        .map(|s| Identifier::from_unquoted(&s).map_err(|e| anyhow::anyhow!(e.to_string())))
        .collect::<Result<_, _>>()?;
    let strategy = match fixture
        .passthrough
        .planner
        .get("strategy")
        .and_then(|v| v.as_str())
        .unwrap_or("online")
    {
        "atomic" => Strategy::Atomic,
        _ => Strategy::Online,
    };
    Ok(BuildPlanOptions {
        managed_schemas: schemas,
        ignore_objects: vec![],
        strategy,
        planner_ruleset_version: pgevolve_core::plan::PlannerPolicy::default()
            .planner_ruleset_version,
        existing_lint_waivers: vec![],
        source_rev: None,
    })
}

fn filter_from_fixture(fixture: &Fixture) -> anyhow::Result<CatalogFilter> {
    let schemas: Vec<Identifier> = collect_managed_schemas(&fixture.after_sql)
        .into_iter()
        .map(|s| Identifier::from_unquoted(&s).map_err(|e| anyhow::anyhow!(e.to_string())))
        .collect::<Result<_, _>>()?;
    CatalogFilter::new(schemas, vec![]).map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn check_failure_expectation(fixture: &Fixture, stderr: &str, stage: &'static str) -> ApplyOutcome {
    if fixture.expect.apply.succeeds {
        return ApplyOutcome::ApplyFailed {
            stderr: stderr.to_string(),
            stage,
        };
    }
    let all_match = fixture
        .expect
        .apply
        .error_contains
        .iter()
        .all(|s| stderr.contains(s.as_str()));
    if all_match {
        ApplyOutcome::OkExpectedFailure
    } else {
        ApplyOutcome::ApplyFailed {
            stderr: format!(
                "fixture expected failure with substrings {:?}; got stderr:\n{stderr}",
                fixture.expect.apply.error_contains,
            ),
            stage,
        }
    }
}

async fn introspect_with_drift(
    pg: &EphemeralPostgres,
    fixture: &Fixture,
) -> anyhow::Result<(Catalog, pgevolve_core::catalog::DriftReport)> {
    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let schemas = collect_managed_schemas(&fixture.after_sql);
    let managed: Vec<Identifier> = schemas
        .into_iter()
        .map(|s| Identifier::from_unquoted(&s))
        .collect::<Result<_, _>>()?;
    let filter = CatalogFilter::new(managed, vec![])?;
    tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await?
        .map_err(Into::into)
}

fn parse_post_apply_target(fixture: &Fixture) -> anyhow::Result<Catalog> {
    let rel = &fixture.expect.apply.post_apply_equals_to;
    let body = std::fs::read_to_string(fixture.dir.join(rel))
        .map_err(|e| anyhow::anyhow!("read {rel}: {e}"))?;
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("after.sql"), body)?;
    parse_directory(tmp.path(), &[]).map_err(|e| anyhow::anyhow!("parse {rel}: {e}"))
}

/// Crude regex scan: every line containing `CREATE SCHEMA <name>` adds the schema name.
fn collect_managed_schemas(after_sql: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?i)CREATE\s+SCHEMA\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)")
        .expect("static regex");
    let mut out: Vec<String> = re
        .captures_iter(after_sql)
        .map(|c| c[1].to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}
