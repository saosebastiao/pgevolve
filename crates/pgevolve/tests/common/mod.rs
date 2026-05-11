//! Test helpers shared by chaos / property tests.
//!
//! Each helper takes a live `EphemeralPostgres`, builds a `Plan` from
//! `(target, source)` against that DB's identity, writes the plan to a
//! tempdir, and runs `pgevolve::apply` with the caller's overrides.

#![allow(dead_code)] // helpers consumed conditionally per test file

use std::path::Path;

use anyhow::{anyhow, Result};
use tempfile::TempDir;
use tokio_postgres::Client;

use pgevolve::executor::{ApplyError, ApplyOverrides};
use pgevolve::pg_querier::PgCatalogQuerier;
use pgevolve_core::catalog::{read_catalog, CatalogFilter};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::plan::{
    group_steps, order, rewrite, write_plan_dir, Plan, PlannerPolicy, Strategy,
};

/// Open a fresh client to `pg` and bootstrap the pgevolve metadata schema.
pub async fn connect_and_bootstrap(pg: &pgevolve_testkit::EphemeralPostgres) -> Result<Client> {
    let mut client = pg.connect().await?;
    pgevolve::executor::bootstrap_metadata(&mut client).await?;
    Ok(client)
}

/// Build a `Plan` going from `target` (pre-image) to `source` (desired)
/// against `client`'s live identity and write it to `dir`.
pub async fn build_plan(
    client: &Client,
    target: &Catalog,
    source: &Catalog,
    dir: &Path,
) -> Result<Plan> {
    let identity = pgevolve::compute_target_identity(client).await?;
    let changes = pgevolve_core::diff::diff(target, source);
    let ordered = order(target, source, changes).map_err(|e| anyhow!("plan order: {e}"))?;
    let policy = PlannerPolicy {
        strategy: Strategy::Online,
        ..PlannerPolicy::default()
    };
    let steps = rewrite(ordered, target, &policy);
    let groups = group_steps(steps);
    let plan = Plan::from_grouped(
        groups,
        source,
        target,
        identity,
        None,
        pgevolve_core::VERSION,
        policy.planner_ruleset_version,
    );
    write_plan_dir(&plan, dir)?;
    Ok(plan)
}

/// Filter that introspection uses against a freshly-created PG.
///
/// `managed_schemas` is the set of schema names the caller's catalog
/// declares.
pub fn catalog_filter(managed_schemas: &[Identifier]) -> Result<CatalogFilter> {
    CatalogFilter::new(managed_schemas.to_vec(), vec![]).map_err(|e| anyhow!(e))
}

/// Apply `source` to `client`'s database, treating `target` as the pre-image.
///
/// `target` is the catalog the planner should diff *from*; for a fresh DB
/// pass `Catalog::empty()`. Returns the apply outcome.
pub async fn apply_diff(
    client: &mut Client,
    target: &Catalog,
    source: &Catalog,
    managed_schemas: &[Identifier],
    abort_after_step: Option<u32>,
) -> Result<Result<pgevolve::executor::ApplyOutcome, ApplyError>> {
    let dir = TempDir::new()?;
    let _plan = build_plan(client, target, source, dir.path()).await?;
    let filter = catalog_filter(managed_schemas)?;
    let overrides = ApplyOverrides {
        allow_different_target: false,
        allow_drift: true, // preflight drift recheck stub bypasses
        actor: Some("chaos-harness".into()),
        abort_after_step,
    };
    Ok(pgevolve::apply(dir.path(), client, &filter, overrides).await)
}

/// Introspect `client`'s database for `managed_schemas` and return the IR.
pub async fn introspect(
    pg: &pgevolve_testkit::EphemeralPostgres,
    managed_schemas: &[Identifier],
) -> Result<Catalog> {
    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let filter = catalog_filter(managed_schemas)?;
    let catalog = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| anyhow!("join: {e}"))?
        .map_err(|e| anyhow!("read_catalog: {e}"))?;
    Ok(catalog)
}

/// Convenience: the schemas declared by a `Catalog`, as `Identifier`s.
pub fn schemas_of(catalog: &Catalog) -> Vec<Identifier> {
    catalog.schemas.iter().map(|s| s.name.clone()).collect()
}
