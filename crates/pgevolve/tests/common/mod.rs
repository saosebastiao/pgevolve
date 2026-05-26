//! Test helpers shared by chaos / property tests.
//!
//! Each helper takes a live `EphemeralPostgres`, builds a `Plan` from
//! `(target, source)` against that DB's identity, writes the plan to a
//! tempdir, and runs `pgevolve::apply` with the caller's overrides.

#![allow(dead_code)] // helpers consumed conditionally per test file

use std::path::Path;

use anyhow::{Result, anyhow};
use tempfile::TempDir;
use tokio_postgres::Client;

use pgevolve::executor::{ApplyError, ApplyOverrides};
use pgevolve::pg_querier::PgCatalogQuerier;
use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::plan::{
    Plan, PlannerPolicy, Strategy, group_steps, order, rewrite, write_plan_dir,
};

/// Open a fresh client to `pg`, bootstrap the pgevolve metadata schema,
/// and pre-create the role universe the IR generator may reference.
///
/// The IR generator (`pgevolve_testkit::ir_generator`) draws role names
/// for owners and grantees from a small fixed pool — see
/// `GRANTEE_ROLE_NAMES` in `ir_generator.rs`. We pre-create those roles
/// in the ephemeral DB so generated catalogs can apply without
/// `[42704] role "X" does not exist` failures. Property tests live in
/// the per-DB layer and don't otherwise have a way to bring cluster-level
/// roles into existence.
pub async fn connect_and_bootstrap(pg: &pgevolve_testkit::EphemeralPostgres) -> Result<Client> {
    let mut client = pg.connect().await?;
    pgevolve::executor::bootstrap_metadata(&mut client).await?;
    create_generator_role_pool(&client).await?;
    Ok(client)
}

/// Idempotently CREATE ROLE for every name in the IR-generator pool.
/// Uses a DO block per role so a duplicate is silently swallowed (PG has
/// no `CREATE ROLE IF NOT EXISTS`).
async fn create_generator_role_pool(client: &Client) -> Result<()> {
    const ROLES: &[&str] = &["app_owner", "readers", "writers", "app", "ops", "auditor"];
    for r in ROLES {
        let sql = format!(
            "DO $do$ BEGIN \
                CREATE ROLE {r}; \
             EXCEPTION WHEN duplicate_object THEN NULL; END $do$;"
        );
        client
            .batch_execute(&sql)
            .await
            .map_err(|e| anyhow!("pre-create role {r}: {e}"))?;
    }
    Ok(())
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
    let changes = pgevolve_core::diff::diff(
        target,
        source,
        &pgevolve_core::catalog::DriftReport::default(),
    );
    let policy = PlannerPolicy {
        strategy: Strategy::Online,
        ..PlannerPolicy::default()
    };
    let ordered =
        order(target, source, changes, &policy).map_err(|e| anyhow!("plan order: {e}"))?;
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
    )
    .map_err(|e| anyhow!("from_grouped: {e}"))?;
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
        allow_drift: true,              // preflight drift recheck bypassed in tests
        allow_unwaived_lint: true,      // test plans have no lint waivers
        allow_unapproved_intents: true, // test plans are built programmatically
        actor: Some("chaos-harness".into()),
        abort_after_step,
    };
    Ok(pgevolve::apply(dir.path(), client, &filter, overrides).await)
}

/// Lenient convergence check: source `owner = None` / `grants = []` /
/// `rls_enabled = false` etc. are *unmanaged*, per the v0.3.x lenient
/// drift policy. Strict catalog equality spuriously fires on every
/// auto-assigned owner and unmanaged-grant the catalog reports. We're
/// convergent iff a re-plan from `live` → `source` emits no changes.
///
/// Falls back to the strict comparator's error formatter when not
/// convergent so the failure message stays human-readable.
pub fn assert_convergent(live: &Catalog, source: &Catalog) -> Result<()> {
    let changeset = pgevolve_core::diff::diff(
        live,
        source,
        &pgevolve_core::catalog::DriftReport::default(),
    );
    if changeset.is_empty() {
        return Ok(());
    }
    pgevolve_testkit::assert_canonical_eq(source, live)
}

/// Introspect `client`'s database for `managed_schemas` and return the IR.
pub async fn introspect(
    pg: &pgevolve_testkit::EphemeralPostgres,
    managed_schemas: &[Identifier],
) -> Result<Catalog> {
    let client = pg.connect().await?;
    let querier = PgCatalogQuerier::new(client)?;
    let filter = catalog_filter(managed_schemas)?;
    let (catalog, _drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| anyhow!("join: {e}"))?
        .map_err(|e| anyhow!("read_catalog: {e}"))?;
    Ok(catalog)
}

/// Convenience: the schemas declared by a `Catalog`, as `Identifier`s.
pub fn schemas_of(catalog: &Catalog) -> Vec<Identifier> {
    catalog.schemas.iter().map(|s| s.name.clone()).collect()
}
