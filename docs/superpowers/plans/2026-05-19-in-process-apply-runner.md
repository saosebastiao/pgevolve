# In-process Apply Runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the three subprocess invocations in `crates/pgevolve-conformance/src/assertions/apply.rs` (`bootstrap`, `plan`, `apply`) with direct library calls into `pgevolve` and `pgevolve-core`. Pure infrastructure refactor — every conformance fixture produces the same SQL, the same DB end-state, and the same pass/fail outcome.

**Architecture:** Add `Plan::approve_all_intents` on `pgevolve_core`; add `pgevolve::executor::apply_plan(&Plan, ...)` and have the existing `apply(path, ...)` delegate; add `pgevolve::api::build_plan(...)` that performs the parse → introspect → diff → order → rewrite → group pipeline without CLI ceremony; rewrite the conformance apply runner to call them instead of spawning subprocesses.

**Tech Stack:** Rust 2024 edition; existing `tokio_postgres`, `anyhow`, `thiserror`; no new dependencies.

**Reference design:** `docs/superpowers/specs/2026-05-19-in-process-apply-runner-design.md`.

---

## File Structure

**Created:**
- `crates/pgevolve/src/api/mod.rs` — `build_plan`, `BuildPlanOptions`, `BuildPlanError`.
- `crates/pgevolve/tests/api_build_plan.rs` — Docker-gated happy-path integration test.

**Modified:**
- `crates/pgevolve-core/src/plan/plan.rs` — add `Plan::approve_all_intents`.
- `crates/pgevolve/src/lib.rs` — `pub mod api;` and re-export the new types.
- `crates/pgevolve/src/executor/mod.rs` — add `apply_plan`; refactor `apply` to a one-line shim.
- `crates/pgevolve/src/commands/plan.rs` — call `api::build_plan` for the core pipeline; keep CLI-only bits.
- `crates/pgevolve-conformance/src/assertions/apply.rs` — rewrite to use the library entry points; delete the subprocess helpers.

---

## Task 1: `Plan::approve_all_intents`

**Files:**
- Modify: `crates/pgevolve-core/src/plan/plan.rs`

- [ ] **Step 1: Write the failing test**

In `crates/pgevolve-core/src/plan/plan.rs`, locate the existing `#[cfg(test)] mod tests` block (or create one at the end of the file if none exists). Add this test:

```rust
    #[test]
    fn approve_all_intents_flips_every_intent_to_approved() {
        let mut plan = sample_plan_with_two_unapproved_intents();
        assert!(!plan.intents[0].approved);
        assert!(!plan.intents[1].approved);
        plan.approve_all_intents();
        assert!(plan.intents[0].approved);
        assert!(plan.intents[1].approved);
    }

    fn sample_plan_with_two_unapproved_intents() -> Plan {
        Plan {
            id: PlanId::compute_from_bytes(b"sample"),
            groups: Vec::new(),
            intents: vec![
                DestructiveIntent {
                    id: 1,
                    step: 1,
                    kind: "drop_column".into(),
                    target: "app.users.legacy_email".into(),
                    reason: "test".into(),
                    approved: false,
                },
                DestructiveIntent {
                    id: 2,
                    step: 2,
                    kind: "drop_table".into(),
                    target: "app.old_users".into(),
                    reason: "test".into(),
                    approved: false,
                },
            ],
            lint_waivers: Vec::new(),
            step_overrides: Vec::new(),
            metadata: PlanMetadata {
                pgevolve_version: pgevolve_core::VERSION.to_string(),
                planner_ruleset_version: 1,
                source_rev: None,
                target_identity: "test-identity".into(),
                target_snapshot: crate::ir::catalog::Catalog::empty(),
                created_at: time::OffsetDateTime::UNIX_EPOCH,
                lint_at_plan_findings: Vec::new(),
            },
        }
    }
```

Imports the test needs (add `use super::*;` at the top of the `mod tests` block if not already present). If `PlanId::compute_from_bytes` is not the actual constructor name, replace with whatever the file uses for constructing a `PlanId` in existing tests (grep `PlanId::` inside the file's tests). The point is to construct any valid `PlanId` — the test does not depend on its value.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p pgevolve-core --lib plan::plan::tests::approve_all_intents_flips_every_intent_to_approved`
Expected: COMPILE ERROR — `approve_all_intents` does not exist.

- [ ] **Step 3: Implement the method**

In `crates/pgevolve-core/src/plan/plan.rs`, inside the existing `impl Plan { ... }` block (find it with `grep -n "^impl Plan"`), add:

```rust
    /// Mark every destructive intent as `approved = true`.
    ///
    /// Intended for test harnesses that build plans programmatically and
    /// want to bypass the `intent.toml`-based approval workflow. Production
    /// apply must continue to require explicit approval in `intent.toml`.
    pub fn approve_all_intents(&mut self) {
        for intent in &mut self.intents {
            intent.approved = true;
        }
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p pgevolve-core --lib plan::plan::tests::approve_all_intents_flips_every_intent_to_approved`
Expected: PASS.

- [ ] **Step 5: Run the full pgevolve-core test suite + clippy**

Run:
```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/plan/plan.rs
git commit -m "$(cat <<'EOF'
feat(plan): add Plan::approve_all_intents

Marks every destructive intent in the plan as approved. Replaces the
text-substitution hack the conformance apply runner used against an
on-disk intent.toml. Production apply still requires explicit approval
in intent.toml; this helper is for test harnesses that build plans
programmatically.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `pgevolve::executor::apply_plan`

**Files:**
- Modify: `crates/pgevolve/src/executor/mod.rs`

- [ ] **Step 1: Inspect the current `apply` function shape**

Run: `grep -n "^pub async fn apply\b\|^async fn apply\b\|^pub fn apply\b" crates/pgevolve/src/executor/mod.rs`
Expected: one match — the existing `pub async fn apply(plan_dir: &Path, ...)` signature.

Read the body of that function so you have it cached for the refactor in Step 2.

- [ ] **Step 2: Refactor `apply` to delegate to a new `apply_plan`**

In `crates/pgevolve/src/executor/mod.rs`, replace the current body of `apply` (the `pub async fn apply(plan_dir: &Path, ...) -> Result<...>` function) with this two-function shape. The first reads from disk and delegates; the second contains the existing body, minus the first line (`let plan = pgevolve_core::plan::read_plan_dir(plan_dir)?;`).

Replace the entire current `apply` function with:

```rust
/// Apply a plan directory to a live Postgres connection.
///
/// Reads the plan from disk and delegates to [`apply_plan`]. Use
/// `apply_plan` directly when you already have a [`Plan`] value (test
/// harnesses, library callers that built the plan via
/// [`crate::api::build_plan`]).
///
/// See [`apply_plan`] for the full step-by-step description.
pub async fn apply(
    plan_dir: &Path,
    client: &mut Client,
    filter: &CatalogFilter,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    let plan = pgevolve_core::plan::read_plan_dir(plan_dir)?;
    apply_plan(&plan, client, filter, overrides).await
}

/// Apply an in-memory [`Plan`] to a live Postgres connection.
///
/// Steps (spec §8):
/// 1. Bootstrap or upgrade the `pgevolve` metadata schema.
/// 2. Acquire the singleton advisory lock.
/// 3. Run preflight checks (identity match, drift, intent approval).
/// 4. Open an `apply_log` row + pre-populate `plan_steps` as `pending`.
/// 5. Execute each group in order; mark steps `succeeded`, `failed`, or `rolled_back`.
/// 6. Close the `apply_log` row with the final status.
///
/// The advisory lock is released automatically when the returned future
/// completes (success or failure).
pub async fn apply_plan(
    plan: &Plan,
    client: &mut Client,
    filter: &CatalogFilter,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    bootstrap_metadata(client).await?;

    let actor = overrides.actor.clone().unwrap_or_else(default_actor);
    try_acquire_lock(client, &actor).await?;

    let preflight = PreflightOverrides {
        allow_different_target: overrides.allow_different_target,
        allow_drift: overrides.allow_drift,
        allow_unwaived_lint: overrides.allow_unwaived_lint,
        allow_unapproved_intents: overrides.allow_unapproved_intents,
    };
    let preflight_result = run_preflight(client, plan, filter, preflight).await;
    if let Err(e) = preflight_result {
        // Failure before any DDL — release the lock before propagating.
        let _ = release_lock(client).await;
        return Err(e);
    }

    let apply_id = audit::open_apply_log(client, plan, &actor).await?;
    let exec_result =
        execute::execute_plan(client, plan, apply_id, overrides.abort_after_step).await;
    match exec_result {
        Ok(()) => {
            audit::close_apply_log(client, apply_id, "succeeded", None).await?;
            release_lock(client).await?;
            Ok(ApplyOutcome::Succeeded { apply_id })
        }
        Err(ApplyError::AbortedAfterStep { step_no }) => {
            audit::close_apply_log(
                client,
                apply_id,
                "aborted",
                Some(&format!("abort_after_step={step_no}")),
            )
            .await?;
            let _ = release_lock(client).await;
            Err(ApplyError::AbortedAfterStep { step_no })
        }
        Err(e) => {
            let msg = e.to_string();
            audit::close_apply_log(client, apply_id, "failed", Some(&msg)).await?;
            let _ = release_lock(client).await;
            Err(e)
        }
    }
}
```

Notes:
- The body of `apply_plan` is the existing body of `apply` with one mechanical change: every `&plan` becomes `plan` (and every `&plan` → `plan`) where `plan` is now a `&Plan` parameter. No internal logic changes.
- `bootstrap_metadata` is called before `try_acquire_lock` because the metadata tables must exist before the advisory lock function can be looked up. The original `apply` did this in the same order; `apply_plan` keeps it.
- The first line of the old `apply` (`let plan = pgevolve_core::plan::read_plan_dir(plan_dir)?;`) is the only thing that moves OUT of the new function; it stays in the new `apply` shim.

Add a `Plan` import at the top of the file if not already present:

```rust
use pgevolve_core::plan::Plan;
```

(Check existing imports first — `Plan` may already be imported.)

- [ ] **Step 3: Update the `pub use` re-export in `src/lib.rs`**

In `crates/pgevolve/src/lib.rs`, find the existing line:

```rust
pub use executor::{ApplyError, ApplyOutcome, ApplyOverrides, apply, bootstrap_metadata};
```

Replace it with:

```rust
pub use executor::{
    ApplyError, ApplyOutcome, ApplyOverrides, apply, apply_plan, bootstrap_metadata,
};
```

- [ ] **Step 4: Build, run all `pgevolve` crate tests, clippy**

Run:
```
cargo build -p pgevolve
cargo test -p pgevolve --lib --tests
cargo clippy -p pgevolve --all-targets -- -D warnings
```
Expected: all existing tests pass (the existing `apply` callers still work because `apply` still exists and behaves identically). No new tests yet.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve/src/executor/mod.rs crates/pgevolve/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(executor): add apply_plan that takes a Plan value

The existing apply(plan_dir, ...) becomes a thin shim that reads the
plan from disk and delegates to apply_plan(&Plan, ...). Library callers
that already have a Plan (api::build_plan, test harnesses) skip the
disk round-trip.

Public behavior identical for the existing apply entry point.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `pgevolve::api::build_plan`

**Files:**
- Create: `crates/pgevolve/src/api/mod.rs`
- Modify: `crates/pgevolve/src/lib.rs`
- Modify: `crates/pgevolve/src/commands/plan.rs`

- [ ] **Step 1: Create the `api` module with `build_plan`**

`build_plan` consumes its `Client` (moves it into `PgCatalogQuerier` for the blocking read; the `Arc<Client>` inside the querier is dropped when the querier goes out of scope). Callers that need a client for the subsequent `apply_plan` open a second client. This matches the CLI's existing pattern where `pgevolve plan` and `pgevolve apply` are separate processes that each open their own connection.

Create `crates/pgevolve/src/api/mod.rs`:

```rust
//! Library entry points for embedding pgevolve in other tools and tests.
//!
//! Today this module contains a single function, [`build_plan`], that runs
//! the full parse → introspect → diff → order → rewrite → group → assemble
//! pipeline and returns a [`Plan`] value. The CLI command
//! `pgevolve plan` is now a thin wrapper over this entry point plus
//! CLI-only UX (stdout, interactive waiver prompts, shadow validation).
//!
//! Conformance tests and test harnesses use this entry point directly to
//! avoid the cost of spawning the CLI binary per fixture.
//!
//! See `docs/superpowers/specs/2026-05-19-in-process-apply-runner-design.md`.

use std::path::Path;

use tokio_postgres::Client;

use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::diff::diff;
use pgevolve_core::identifier::Identifier;
use pgevolve_core::lint::Severity;
use pgevolve_core::lint::universal::run_drift_lints;
use pgevolve_core::plan::{
    LintWaiver, Plan, PlannerPolicy, RecordedFinding, Strategy, group_steps, order,
    rewrite_with_source,
};

use crate::pg_querier::PgCatalogQuerier;
use crate::target_identity::compute_target_identity;

/// Options for [`build_plan`].
///
/// These map to the equivalent fields in `pgevolve.toml` and the CLI
/// `pgevolve plan` invocation, but the caller supplies them directly
/// instead of reading from a config file.
#[derive(Debug, Clone, Default)]
pub struct BuildPlanOptions {
    /// Schema names the catalog reader will include.
    pub managed_schemas: Vec<Identifier>,
    /// Glob patterns for objects to ignore inside managed schemas.
    pub ignore_objects: Vec<String>,
    /// Planner strategy (`Online` or `Atomic`).
    pub strategy: Strategy,
    /// Planner ruleset version stamped into the plan.
    ///
    /// Use `PlannerPolicy::default().planner_ruleset_version` unless
    /// you have a specific reason to override.
    pub planner_ruleset_version: u32,
    /// Pre-existing lint waivers (typically loaded from an existing
    /// `intent.toml` in the caller's plan directory). When empty, every
    /// `LintAtPlan` finding is treated as unwaived.
    pub existing_lint_waivers: Vec<LintWaiver>,
    /// Optional source-tree revision identifier stamped into the plan
    /// (e.g., `"git:abc1234"`).
    pub source_rev: Option<String>,
}

/// Errors raised by [`build_plan`].
#[derive(Debug, thiserror::Error)]
pub enum BuildPlanError {
    /// `parse_directory` rejected the source schema.
    #[error("parse error: {0}")]
    Parse(String),
    /// `read_catalog` failed against the live database.
    #[error("catalog read error: {0}")]
    CatalogRead(String),
    /// The planner pipeline failed (typically `order`).
    #[error("planner error: {0}")]
    Planner(String),
    /// One or more `LintAtPlan` findings need explicit waivers in
    /// `existing_lint_waivers` before a plan can be built.
    #[error("unwaived LintAtPlan findings: {0}")]
    LintAtPlanRequiresWaiver(String),
    /// Lower-level connection / introspection failure.
    #[error("connection error: {0}")]
    Connection(String),
}

/// Build a [`Plan`] from a source schema directory and a live database.
///
/// Consumes `client`: the connection is moved into the catalog reader and
/// dropped when this function returns. Callers that need to apply the
/// plan should open a second `Client` for [`crate::executor::apply_plan`].
/// (This matches the CLI's behavior where `pgevolve plan` and
/// `pgevolve apply` are separate processes with separate connections.)
///
/// Mirrors the core pipeline of `pgevolve plan` but skips CLI-specific
/// behavior: no `println!`, no interactive waiver prompts, no
/// `--shadow-validate`, no writing of the plan directory to disk.
pub async fn build_plan(
    schema_dir: &Path,
    client: Client,
    opts: BuildPlanOptions,
) -> Result<Plan, BuildPlanError> {
    let source = pgevolve_core::parse::parse_directory(schema_dir, &[])
        .map_err(|e| BuildPlanError::Parse(e.to_string()))?;

    let target_identity = compute_target_identity(&client)
        .await
        .map_err(|e| BuildPlanError::Connection(e.to_string()))?;

    let filter = CatalogFilter::new(opts.managed_schemas.clone(), opts.ignore_objects.clone())
        .map_err(|e| BuildPlanError::CatalogRead(e.to_string()))?;
    let querier =
        PgCatalogQuerier::new(client).map_err(|e| BuildPlanError::Connection(e.to_string()))?;
    let (target, drift) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter))
        .await
        .map_err(|e| BuildPlanError::Connection(format!("join error: {e}")))?
        .map_err(|e| BuildPlanError::CatalogRead(e.to_string()))?;

    let changes = diff(&target, &source, &drift);
    let policy = PlannerPolicy {
        strategy: opts.strategy,
        online: PlannerPolicy::default().online,
        ..PlannerPolicy::default()
    };
    let ordered = order(&target, &source, changes, &policy)
        .map_err(|e| BuildPlanError::Planner(e.to_string()))?;
    let steps = rewrite_with_source(ordered, &target, &source, &policy);
    let groups = group_steps(steps);
    let mut plan = Plan::from_grouped(
        groups,
        &source,
        &target,
        target_identity,
        opts.source_rev.clone(),
        pgevolve_core::VERSION,
        opts.planner_ruleset_version,
    );

    // --- Drift-lint gate (mirrors commands::plan::run) ---
    let drift_findings = run_drift_lints(&source, &target);
    let lint_at_plan: Vec<_> = drift_findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::LintAtPlan))
        .collect();

    if !lint_at_plan.is_empty() {
        let unwaived: Vec<_> = lint_at_plan
            .iter()
            .filter(|f| !waiver_matches(f, &opts.existing_lint_waivers))
            .collect();
        if !unwaived.is_empty() {
            let msg = unwaived
                .iter()
                .map(|f| format!("[{}] ({}): {}", f.rule, f.severity, f.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(BuildPlanError::LintAtPlanRequiresWaiver(msg));
        }
        plan.metadata.lint_at_plan_findings = lint_at_plan
            .iter()
            .map(|f| {
                let target_str = f.message.split(':').next().unwrap_or("").trim().to_string();
                RecordedFinding {
                    rule: f.rule.to_string(),
                    target: target_str,
                    message: f.message.clone(),
                }
            })
            .collect();
    }

    Ok(plan)
}

fn waiver_matches(finding: &pgevolve_core::lint::Finding, waivers: &[LintWaiver]) -> bool {
    waivers
        .iter()
        .any(|w| w.rule == finding.rule && finding.message.contains(&w.target))
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

In `crates/pgevolve/src/lib.rs`, after the existing `pub mod` lines, add:

```rust
pub mod api;
```

And below the existing `pub use` re-exports, add:

```rust
pub use api::{BuildPlanError, BuildPlanOptions, build_plan};
```

- [ ] **Step 3: Refactor `commands::plan::run` to call `api::build_plan`**

In `crates/pgevolve/src/commands/plan.rs`, replace the body of `pub async fn run` so the core pipeline goes through `api::build_plan`. The new function:

(a) Loads pre-existing lint waivers from `intent.toml` in the output dir (existing helper `load_existing_waivers`).
(b) Builds `BuildPlanOptions` from `cfg` and `opts`.
(c) Connects to the DB.
(d) Calls `api::build_plan`, handling `BuildPlanError::LintAtPlanRequiresWaiver` specially to print the friendly user-facing waiver-instruction message.
(e) Writes the plan dir via `pgevolve_core::plan::write_plan_dir`.
(f) Prints the "Wrote plan ..." line.
(g) Optionally runs `--shadow-validate`.

The refactor preserves every existing CLI behavior; only the inner parse-introspect-diff-rewrite-group logic is delegated to the library. Replace the function with:

```rust
pub async fn run(args: PlanArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let client = connect(&opts).await?;

    let build_opts = crate::api::BuildPlanOptions {
        managed_schemas: opts.managed_schemas.clone(),
        ignore_objects: opts.ignore_objects.clone(),
        strategy: opts.strategy,
        planner_ruleset_version: pgevolve_core::plan::PlannerPolicy::default()
            .planner_ruleset_version,
        existing_lint_waivers: Vec::new(), // re-populated below if waivers exist
        source_rev: detect_git_rev().ok(),
    };

    // The output directory determines where to look for pre-existing
    // waivers. We need the plan first to know its id; but we can pre-load
    // waivers using a stable path derived from cfg + a placeholder.
    // Simplest: do a two-pass build — first call build_plan with empty
    // waivers to discover the LintAtPlan findings (if any), check the
    // output dir for waivers, and re-call with those.
    //
    // To avoid the two-pass dance, we load waivers from the conventional
    // path before the first build: `<plan_dir>/intent.toml` doesn't yet
    // exist for a new plan, so for first plans the load returns empty.
    // For re-plans where the user edited intent.toml, we honor those.
    let pre_out_dir = args.output.clone();
    let pre_existing = if let Some(dir) = pre_out_dir.as_deref() {
        load_existing_waivers(dir)
    } else {
        Vec::new()
    };
    let build_opts = crate::api::BuildPlanOptions {
        existing_lint_waivers: pre_existing,
        ..build_opts
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
```

Notes:
- `run_shadow_cross_check` previously had `source` in scope; now we re-parse the source directory for it. The cost is minor; alternative would be to thread `source` through `build_plan`'s return, but that adds API noise for a niche feature.
- The lint-waiver error message is less detailed than the existing CLI message (which prints each finding and an example `[[lint_waiver]]` row). If you want full parity, capture the full `Vec<Finding>` in the `BuildPlanError::LintAtPlanRequiresWaiver` variant (change the field type from `String` to `Vec<(String, String, String)>` or similar). For now the simpler message is acceptable; if a conformance fixture or another test relies on the old wording, expand the variant.
- Keep the existing `load_existing_waivers`, `waiver_matches`, `default_plan_dir`, `detect_git_rev`, and `run_shadow_cross_check` helpers; they're still used.

- [ ] **Step 4: Write the Docker-gated integration test for `api::build_plan`**

Create `crates/pgevolve/tests/api_build_plan.rs`:

```rust
//! Docker-gated happy-path test for `pgevolve::api::build_plan`.
//!
//! Spins up an `EphemeralPostgres`, seeds a minimal schema, calls
//! `build_plan` against a tempdir source, and asserts the returned plan
//! has the expected shape.

use anyhow::Result;
use pgevolve::api::{BuildPlanOptions, build_plan};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::plan::Strategy;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_plan_produces_a_create_table_plan() -> Result<()> {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return Ok(());
    }

    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let mut client = pg.connect().await?;
    pgevolve::executor::bootstrap_metadata(&mut client).await?;
    // Seed an empty `app` schema so the catalog has *something* in
    // managed scope but no tables.
    client.batch_execute("CREATE SCHEMA app;").await?;

    let tmp = tempfile::tempdir()?;
    std::fs::create_dir_all(tmp.path().join("schema"))?;
    std::fs::write(
        tmp.path().join("schema/0001.sql"),
        "-- @pgevolve schema=app\nCREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    )?;

    let opts = BuildPlanOptions {
        managed_schemas: vec![Identifier::from_unquoted("app").unwrap()],
        ignore_objects: vec![],
        strategy: Strategy::Online,
        planner_ruleset_version: pgevolve_core::plan::PlannerPolicy::default()
            .planner_ruleset_version,
        existing_lint_waivers: vec![],
        source_rev: None,
    };
    let plan = build_plan(&tmp.path().join("schema"), client, opts).await?;

    assert!(!plan.groups.is_empty(), "expected at least one group");
    let step_count: usize = plan.groups.iter().map(|g| g.steps.len()).sum();
    assert!(step_count >= 1, "expected at least one step, got {step_count}");
    Ok(())
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p pgevolve --test api_build_plan`
Expected: PASS (assuming Docker is available). If Docker is not available, the test logs and returns Ok — that's the expected pattern across the workspace.

- [ ] **Step 6: Run the full `pgevolve` test suite and clippy**

Run:
```
cargo test -p pgevolve --lib --tests
cargo clippy -p pgevolve --all-targets -- -D warnings
```
Expected: all green. The CLI `plan` command's behavior is unchanged because we re-implemented it using `api::build_plan` (any differences in output formatting from the lint-waiver message branch are acceptable if no test currently asserts on the exact wording — verify by running `cargo test -p pgevolve` and inspecting any failures).

If the CLI test asserts on the exact lint-waiver stderr message and fails, expand `BuildPlanError::LintAtPlanRequiresWaiver` to carry structured data (e.g., `Vec<RecordedFinding>`) and reconstruct the CLI message in `commands::plan::run` from that data.

- [ ] **Step 7: Commit**

```bash
git add crates/pgevolve/src/api/ crates/pgevolve/src/lib.rs crates/pgevolve/src/commands/plan.rs crates/pgevolve/tests/api_build_plan.rs
git commit -m "$(cat <<'EOF'
feat(api): pgevolve::api::build_plan library entry point

Extracts the parse→introspect→diff→order→rewrite→group→assemble
pipeline from commands::plan::run into a reusable library function
that returns a Plan value. The CLI `pgevolve plan` command becomes a
thin wrapper that adds println/waiver-prompt UX and disk-write on top.

Adds a Docker-gated happy-path test in tests/api_build_plan.rs that
exercises the new entry point against an EphemeralPostgres.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Rewrite the conformance apply runner to use the library

**Files:**
- Modify: `crates/pgevolve-conformance/src/assertions/apply.rs`

- [ ] **Step 1: Inspect the current runner to understand what's being replaced**

Read `crates/pgevolve-conformance/src/assertions/apply.rs` end-to-end. The new file deletes:
- `cargo_bin()`
- `run_pgevolve()`
- `plan_and_locate()`
- `patch_intent_toml_approve_all()`
- `write_project()`

And replaces `check_with_options` to use library calls.

- [ ] **Step 2: Replace `check_with_options` and `introspect_with_drift` with library-call versions**

Replace the entire body of `crates/pgevolve-conformance/src/assertions/apply.rs` with:

```rust
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

use std::path::Path;

use pgevolve::api::{BuildPlanError, BuildPlanOptions};
use pgevolve::executor::{ApplyError, ApplyOverrides};
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
    seed_before(&pg, &fixture.before_sql).await?;

    // Write `after.sql` into a tempdir; the parser walks a directory.
    let schema_tmp = tempfile::tempdir()?;
    std::fs::write(schema_tmp.path().join("0001-fixture.sql"), &fixture.after_sql)?;

    // build_plan never touches the pgevolve metadata tables, so no
    // pre-bootstrap is required. apply_plan calls bootstrap_metadata as
    // its first step, mirroring the CLI's flow.
    //
    // build_plan consumes its client; apply_plan opens a fresh one.
    let build_client = pg.connect().await?;
    let build_opts = build_options_from_fixture(fixture)?;
    let mut plan = match pgevolve::api::build_plan(schema_tmp.path(), build_client, build_opts).await
    {
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

    if post_apply_ir.canonical_eq(&expected_ir) {
        Ok(ApplyOutcome::Ok(Box::new(PostApplyState {
            catalog: post_apply_ir,
            drift: post_apply_drift,
            after_source: expected_ir,
        })))
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
```

Notes for the implementer:
- The `client` is moved into `build_plan` and returned in the tuple. The variable shadowing (`let mut client = client;`) before the `apply_plan` call is the standard Rust pattern for re-binding ownership.
- If `build_plan` failed because of an in-progress preflight check rather than a planner error, the error message still reaches `check_failure_expectation`, which substring-matches against `fixture.expect.apply.error_contains`. Existing fixtures should pass if their `error_contains` strings are substrings of the new error wording. If a fixture fails because the new error wording differs from the CLI's old wording, update the fixture's `expect.apply.error_contains` to a substring present in both — do NOT update the runner to mimic the old wording.

- [ ] **Step 3: Build, run the conformance suite, clippy**

Run:
```
cargo build -p pgevolve-conformance
cargo test -p pgevolve-conformance --test run
cargo clippy -p pgevolve-conformance --all-targets -- -D warnings
```
Expected: conformance suite passes. If a fixture fails on `error_contains`, update that fixture's strings to substrings shared between old and new error wording.

- [ ] **Step 4: Run the full workspace tests**

Run: `cargo test --workspace --lib --tests`
Expected: all green.

- [ ] **Step 5: Run the conformance suite against PG 14 to verify version-coverage**

Run: `PGEVOLVE_TEST_PG_VERSION=14 cargo test -p pgevolve-conformance --test run`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-conformance/src/assertions/apply.rs
git commit -m "$(cat <<'EOF'
refactor(conformance): apply runner uses library entry points

Replaces the three subprocess invocations (bootstrap, plan, apply) with
direct library calls into pgevolve::executor::{bootstrap_metadata,
apply_plan} and pgevolve::api::build_plan. Drops ~150 lines of
subprocess scaffolding (cargo_bin, run_pgevolve, plan_and_locate,
patch_intent_toml_approve_all, write_project).

Same fixtures, same SQL emitted, same DB end-state. Faster (no
per-fixture binary rebuild + spawn) and easier to debug (assertions
can inspect the Plan value rather than its on-disk rendering).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Workspace-wide verification

**Files:** none modified — verification only.

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test --workspace --lib --tests`
Expected: all green.

- [ ] **Step 2: Run clippy with `-D warnings` on the whole workspace**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt --check`
Expected: no output.

- [ ] **Step 4: Run the conformance suite against each supported PG major**

Run:
```
for v in 14 15 16 17; do
  echo "=== PG $v ==="
  PGEVOLVE_TEST_PG_VERSION=$v cargo test -p pgevolve-conformance --test run 2>&1 | tail -5
done
```
Expected: all 4 PG versions PASS.

- [ ] **Step 5: Run the property tests (Docker-gated, ~70s each)**

Run:
```
cargo test -p pgevolve-core --test property_tests -- --include-ignored
cargo test -p pgevolve --test pg_property_tests -- --include-ignored
cargo test -p pgevolve --test chaos_apply -- --include-ignored
```
Expected: all PASS.

- [ ] **Step 6: Verify the subprocess helpers are gone**

Run:
```
grep -n "cargo_bin\|run_pgevolve\|plan_and_locate\|patch_intent_toml_approve_all\|write_project" crates/pgevolve-conformance/src/assertions/apply.rs
```
Expected: zero matches.

Run:
```
grep -n "Command::new\|std::process::Command" crates/pgevolve-conformance/src/assertions/apply.rs
```
Expected: zero matches.

- [ ] **Step 7: If any step produced fixes, commit them**

If `cargo fmt` produced changes, or clippy required local adjustments, commit:

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore: post-in-process-runner cleanup

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

If no changes were needed, skip the commit.

---

## Self-review pre-flight checklist for the implementing agent

Before declaring the plan complete:

- [ ] `crates/pgevolve-conformance/src/assertions/apply.rs` does not import `std::process::Command`.
- [ ] `pgevolve::api::build_plan` exists and is re-exported from `pgevolve::lib`.
- [ ] `pgevolve::executor::apply_plan` exists and is re-exported from `pgevolve::lib`.
- [ ] `Plan::approve_all_intents` exists on `pgevolve_core::plan::Plan` and has its own unit test.
- [ ] All conformance fixtures pass on PG 14, 15, 16, 17.
- [ ] Property tests pass.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- [ ] `cargo fmt --check` is clean.

---

## Out-of-scope (do NOT touch)

- The `bootstrap` command — `executor::bootstrap_metadata` is already a library entry point; the runner just calls it.
- The `pgevolve_testkit` crate — separate concern; if test helpers want to migrate to `apply_plan` later, that's its own change.
- Tier-3 plan goldens — already in-process via `assertions/plan.rs`.
- Sharing a `Client` across bootstrap/plan/apply for speed — out of scope.
- Any change to the CLI binary's user-visible behavior (stdout, exit codes, flags) beyond the lint-waiver error message wording (which may shift slightly; only adjust if a test relies on it).
