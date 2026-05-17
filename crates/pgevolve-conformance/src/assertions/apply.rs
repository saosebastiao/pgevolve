//! Layer 4: apply roundtrip against ephemeral Postgres.
//!
//! Seeds `before.sql` directly into an `EphemeralPostgres` (bypassing
//! pgevolve), constructs a temp project with `after.sql` as the source,
//! invokes the real pgevolve binary for plan+apply, then introspects
//! the post-apply DB and compares the resulting IR against `after.sql`
//! parsed independently.
//!
//! Docker-gated. Skipped (not failed) when `docker_available()` is
//! false, consistent with the rest of the workspace.

use std::path::{Path, PathBuf};
use std::process::Command;

use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::eq::Diff;
use pgevolve_core::parse::parse_directory;
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
    Ok(PostApplyState),
    /// Apply was expected to fail, and it did fail with matching substrings.
    /// No post-apply state is available.
    OkExpectedFailure,
    /// Apply succeeded but introspected IR diverged from after.sql.
    IrMismatch(String),
    /// `pgevolve plan` or `pgevolve apply` failed.
    ApplyFailed {
        /// stderr from the failing command.
        stderr: String,
        /// "plan" or "apply" or "bootstrap".
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

/// Run Layer 4.
pub async fn check(fixture: &Fixture, pg_major: u32) -> anyhow::Result<ApplyOutcome> {
    if !docker_available() {
        return Ok(ApplyOutcome::Skipped);
    }

    let version = pg_version_from_major(pg_major)?;
    let pg = EphemeralPostgres::start(version).await?;

    seed_before(&pg, &fixture.before_sql).await?;

    let project = tempfile::tempdir()?;
    let project_path = project.path();
    write_project(project_path, pg.dsn(), fixture)?;

    if let Err(stderr) = run_pgevolve(project_path, &["bootstrap", "--db", "dev"]) {
        return Ok(check_failure_expectation(fixture, &stderr, "bootstrap"));
    }

    let plan_dir = match plan_and_locate(project_path) {
        Ok(p) => p,
        Err(stderr) => {
            return Ok(check_failure_expectation(fixture, &stderr, "plan"));
        }
    };

    if let Err(stderr) = run_pgevolve(
        project_path,
        &["apply", &plan_dir.display().to_string(), "--db", "dev"],
    ) {
        return Ok(check_failure_expectation(fixture, &stderr, "apply"));
    }

    if !fixture.expect.apply.succeeds {
        return Ok(ApplyOutcome::UnexpectedSuccess);
    }

    let (post_apply_ir, post_apply_drift) = introspect_with_drift(&pg, fixture).await?;
    let expected_ir = parse_post_apply_target(fixture)?;

    if post_apply_ir.canonical_eq(&expected_ir) {
        Ok(ApplyOutcome::Ok(PostApplyState {
            catalog: post_apply_ir,
            drift: post_apply_drift,
            after_source: expected_ir,
        }))
    } else {
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

fn write_project(project_path: &Path, dsn: &str, fixture: &Fixture) -> anyhow::Result<()> {
    let schemas = collect_managed_schemas(&fixture.after_sql);
    let schema_list = schemas
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");

    let strategy = fixture
        .passthrough
        .planner
        .get("strategy")
        .and_then(|v| v.as_str())
        .unwrap_or("online");

    let cfg = format!(
        "[project]\nname = \"conformance\"\nschema_dir = \"schema\"\nplan_dir = \"plans\"\nlayout_profile = \"schema-mirror\"\n\n\
         [managed]\nschemas = [{schema_list}]\n\n\
         [planner]\nstrategy = \"{strategy}\"\n\n\
         [environments.dev]\nurl = \"{dsn}\"\n"
    );
    std::fs::write(project_path.join("pgevolve.toml"), cfg)?;

    std::fs::create_dir_all(project_path.join("schema"))?;
    std::fs::write(project_path.join("schema/0001-fixture.sql"), &fixture.after_sql)?;

    if !fixture.passthrough.intent.is_empty() {
        let body = toml::to_string(&fixture.passthrough.intent)?;
        std::fs::write(project_path.join("intent.toml"), body)?;
    }
    Ok(())
}

/// Crude regex scan: every line containing `CREATE SCHEMA <name>` adds <name>.
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

fn cargo_bin() -> PathBuf {
    // CARGO_BIN_EXE_pgevolve is injected by Cargo when the integration test
    // binary declares pgevolve as a [[bin]] dependency in the *same* package.
    // For cross-package dev-deps, the env var is not set; fall back to the
    // absolute workspace debug path derived from CARGO_MANIFEST_DIR.
    option_env!("CARGO_BIN_EXE_pgevolve")
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_else(|| {
            // CARGO_MANIFEST_DIR is the crate root of pgevolve-conformance.
            // The workspace root is two levels up (../../).
            let manifest_dir = env!("CARGO_MANIFEST_DIR");
            PathBuf::from(manifest_dir)
                .join("../../target/debug/pgevolve")
        })
}

fn run_pgevolve(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cargo_bin())
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(())
}

fn plan_and_locate(cwd: &Path) -> Result<PathBuf, String> {
    let out = Command::new(cargo_bin())
        .current_dir(cwd)
        .args(["plan", "--db", "dev"])
        .output()
        .map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("Wrote plan"))
        .ok_or_else(|| format!("no 'Wrote plan' in stdout:\n{stdout}"))?;
    let rel = line
        .split(" to ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .ok_or_else(|| format!("could not parse plan dir from: {line}"))?;
    Ok(cwd.join(rel))
}

fn check_failure_expectation(
    fixture: &Fixture,
    stderr: &str,
    stage: &'static str,
) -> ApplyOutcome {
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
