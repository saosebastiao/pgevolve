# In-process apply runner — design

**Status:** Approved 2026-05-19. Implementation plan to follow.

## Goal

Replace the three subprocess invocations in
`crates/pgevolve-conformance/src/assertions/apply.rs`
(`bootstrap`, `plan`, `apply`) with direct library calls into
`pgevolve` and `pgevolve-core`. Conformance fixture runs become:

- **Faster** — no rebuild + spawn per fixture.
- **Simpler** — no `cargo_bin` PATH dance, no stderr parsing, no
  `"Wrote plan ... to <dir>"` line-scraping, no temp `pgevolve.toml` or
  `plans/` directory.
- **More debuggable** — assertions can inspect the `Plan` value
  directly, not just its on-disk rendering.

This was the second-largest source of debugging friction in recent
work (the index-on-mv investigation required adding `eprintln!` to
library code to see the plan that the subprocess generated).

## Non-goals

- Touching the CLI binary or how end users invoke `pgevolve`.
- Refactoring the conformance runner's outer shape (it stays
  Docker-gated, tempdir-based for the schema directory, etc.).
- Sharing a single `Client` across bootstrap/plan/apply for speed —
  that's a separate optimization out of scope here.
- Migrating `pgevolve-testkit` helpers to the new entry points.
  Independent concern; can land later piecemeal.
- Changing what fixtures are run, what's asserted, or any
  pass/fail outcome. This is a pure infrastructure refactor.

## Architecture

Two small additions to `pgevolve`'s library surface, plus one helper
on `Plan`:

### 1. `pgevolve::api::build_plan(...)` — new module, ~60 lines

A library entry point that mirrors `commands::plan::run`'s core
pipeline (parse → introspect → diff → order → rewrite → group →
assemble `Plan`) without the CLI ceremony (`println!`,
`--shadow-validate`, interactive lint-waiver UX). Signature:

```rust
pub async fn build_plan(
    schema_dir: &Path,
    client: &mut tokio_postgres::Client,
    opts: BuildPlanOptions,
) -> Result<Plan, BuildPlanError>;
```

`BuildPlanOptions` carries the things `commands::plan::run` reads
from `PgevolveConfig`:

```rust
#[derive(Debug, Clone, Default)]
pub struct BuildPlanOptions {
    pub managed_schemas: Vec<Identifier>,
    pub ignore_objects: Vec<String>,
    pub strategy: pgevolve_core::plan::PlannerStrategy,
    pub planner_ruleset_version: u32,
    pub existing_lint_waivers: Vec<LintWaiver>,
    pub git_rev: Option<String>,
}
```

`BuildPlanError` is a typed enum:

```rust
#[derive(Debug, thiserror::Error)]
pub enum BuildPlanError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("catalog read error: {0}")]
    CatalogRead(String),
    #[error("planner error: {0}")]
    Planner(String),
    #[error("unwaived lint-at-plan findings: {0}")]
    LintAtPlanRequiresWaiver(String),
    #[error("connection error: {0}")]
    Connection(String),
}
```

`commands::plan::run` is refactored to be a thin shim over this
function plus the CLI-only bits (printing "Wrote plan…", writing the
plan dir, interactive waiver-prompt UX, shadow validation).

### 2. `pgevolve::executor::apply_plan(plan: &Plan, ...)` — sibling to `apply`

Takes an in-memory `Plan` instead of a `plan_dir: &Path`. The
existing `apply` is refactored to a thin shim:

```rust
pub async fn apply(
    plan_dir: &Path,
    client: &mut Client,
    filter: &CatalogFilter,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    let plan = pgevolve_core::plan::read_plan_dir(plan_dir)?;
    apply_plan(&plan, client, filter, overrides).await
}

pub async fn apply_plan(
    plan: &Plan,
    client: &mut Client,
    filter: &CatalogFilter,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    // The current body of `apply`, minus the read_plan_dir line.
}
```

Everything downstream (bootstrap_metadata, preflight, audit_open,
execute_plan, audit_close) already operates on a `&Plan`, so the
refactor is mechanical.

### 3. `Plan::approve_all_intents(&mut self)` — helper on `pgevolve_core::plan::Plan`

```rust
impl Plan {
    /// Mark every destructive intent as approved. Replaces the
    /// text-substitution hack the conformance runner used against
    /// on-disk `intent.toml`. Intended for test harnesses that build
    /// plans programmatically; production apply requires intents to
    /// have been approved through `intent.toml`.
    pub fn approve_all_intents(&mut self) {
        for intent in &mut self.intents {
            intent.approved = true;
        }
    }
}
```

## Conformance runner after the change

```rust
pub async fn check_with_options(
    fixture: &Fixture,
    pg_major: u32,
    opts: ApplyOptions,
) -> anyhow::Result<ApplyOutcome> {
    if !docker_available() {
        return Ok(ApplyOutcome::Skipped);
    }
    let pg = EphemeralPostgres::start(pg_version_from_major(pg_major)?).await?;
    seed_before(&pg, &fixture.before_sql).await?;

    let schema_dir = write_schema_tempdir(fixture)?;  // unchanged helper
    let mut client = connect_to_dsn(pg.dsn()).await?;
    pgevolve::executor::bootstrap_metadata(&mut client).await?;

    let build_opts = build_options_from_fixture(fixture);
    let mut plan = match pgevolve::api::build_plan(schema_dir.path(), &mut client, build_opts).await {
        Ok(p) => p,
        Err(e) => return Ok(check_failure_expectation(fixture, &e.to_string(), "plan")),
    };

    if opts.auto_approve_intents {
        plan.approve_all_intents();
    }

    let filter = filter_from_fixture(fixture)?;
    let overrides = ApplyOverrides::default();
    if let Err(e) = pgevolve::executor::apply_plan(&plan, &mut client, &filter, overrides).await {
        return Ok(check_failure_expectation(fixture, &e.to_string(), "apply"));
    }

    let (post_apply_ir, post_apply_drift) = introspect_with_drift(&pg, fixture).await?;
    // ... existing IR comparison (unchanged) ...
}
```

Functions deleted from the runner:
`cargo_bin`, `run_pgevolve`, `plan_and_locate`,
`patch_intent_toml_approve_all`, `write_project`. The whole file
shrinks by roughly 150 lines.

The schema-dir tempdir stays (`parse_directory` walks a directory).

## Files to add

- `crates/pgevolve/src/api/mod.rs` — `build_plan`,
  `BuildPlanOptions`, `BuildPlanError`. Single file, ~120 lines
  including doc comments.

## Files to modify

- `crates/pgevolve/src/lib.rs` — `pub mod api;` and re-export the
  new types alongside the existing re-exports.
- `crates/pgevolve/src/executor/mod.rs` — add `apply_plan`; refactor
  `apply` to a one-line shim that calls it.
- `crates/pgevolve/src/commands/plan.rs` — call `api::build_plan`
  for the core pipeline; keep only the CLI-only bits
  (`println!`, waiver-prompt UX, `--shadow-validate`).
- `crates/pgevolve-core/src/plan/plan.rs` (or wherever `Plan` lives)
  — add `Plan::approve_all_intents`.
- `crates/pgevolve-conformance/src/assertions/apply.rs` — rewrite
  to use the library entry points; delete the subprocess helpers.

## Files to delete

None. The CLI binary and every existing public function stay.

## Error handling

- `BuildPlanError` wraps the `pgevolve-core` parse/catalog/planner
  errors as `String`s rather than re-exporting their types. Reason:
  keeping the `pgevolve::api` surface narrow; callers that need
  structured info already work at the `pgevolve-core` layer.
- `apply_plan` returns the existing `Result<ApplyOutcome, ApplyError>`
  — no change.
- The conformance runner stringifies both errors for
  `check_failure_expectation`, exactly as today.

## Testing

- **Existing conformance fixtures pass unchanged.** Same SQL is
  generated, same DB end-state. This is the contract.
- **One new unit test for `Plan::approve_all_intents`** in
  `crates/pgevolve-core/src/plan/plan.rs::tests`: build a `Plan` with
  two unapproved `DestructiveIntent`s, call the method, assert both
  are flipped to `approved = true`.
- **One new test for `api::build_plan`** in
  `crates/pgevolve/tests/api_build_plan.rs`: Docker-gated, exercises
  the happy path against an `EphemeralPostgres` with a small fixture.
  Mirrors how other Docker-gated integration tests in the workspace
  are structured.
- **No new tests for `apply_plan`** — the existing `apply` test
  coverage (conformance suite + `apply_e2e.rs` + chaos tests) covers
  the same code path since `apply` now delegates.
- Conformance suite test count and pass/fail behavior must be
  identical to before. If a fixture starts behaving differently,
  that's a regression to fix, not a test to update.

## Migration order

Each step is one commit; conformance suite stays green throughout.

1. **Add `Plan::approve_all_intents`** to `pgevolve-core`. Includes a
   single-purpose unit test. No callers yet.
2. **Add `pgevolve::executor::apply_plan`**, refactor `apply` to
   delegate. No CLI changes; no test changes.
3. **Add `pgevolve::api` module** with `build_plan`,
   `BuildPlanOptions`, `BuildPlanError`. Refactor
   `commands::plan::run` to call into it. Add the new Docker-gated
   integration test for `api::build_plan`.
4. **Rewrite `conformance/src/assertions/apply.rs`** to use the
   library entry points. Delete the five helper functions
   (`cargo_bin`, `run_pgevolve`, `plan_and_locate`,
   `patch_intent_toml_approve_all`, `write_project`). Run the full
   conformance suite to verify nothing regressed.

## Out-of-scope items (for clarity)

- The `bootstrap` command — `executor::bootstrap_metadata` is already
  a library entry point; the runner just calls it.
- The `pgevolve_testkit` crate — separate concern; future migration
  can land independently.
- Tier-3 plan goldens — already in-process via `assertions/plan.rs`.
- Sharing a `Client` across bootstrap/plan/apply (speed
  optimization).
- Changing what the CLI binary prints or how its exit codes work.

## Open questions

None. Migration mechanics are covered by the implementation plan.
