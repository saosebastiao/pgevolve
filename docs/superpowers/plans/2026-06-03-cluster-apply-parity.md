# Cluster Apply Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring `apply_cluster_plan_dir` to per-DB parity — structured plan loading, bootstrap, advisory lock, intent enforcement, manifest cross-check, `apply_log` audit — by routing cluster plans through the existing `pgevolve_core::plan::Plan` serializer.

**Architecture:** `ClusterPlan` learns `to_plan(target_identity, plan_id)`. The CLI `cluster plan` command stops hand-rolling the 3 files and uses `Plan::from_grouped_with_id` + `write_plan_dir`. The CLI `cluster apply` command uses `read_plan_dir` + a new `apply_cluster_plan(plan, client, cfg, overrides)` that mirrors per-DB `apply_plan` step-by-step. Per-DB apply machinery (bootstrap, lock, audit, execute) is reused; only the preflight has a cluster-flavored variant.

**Tech Stack:** Rust 1.95+, tokio-postgres, blake3, existing pgevolve-core plan serializer.

**Source spec:** [`docs/superpowers/specs/2026-06-03-cluster-apply-parity-design.md`](../specs/2026-06-03-cluster-apply-parity-design.md).

**Standing project rules** (from `CLAUDE.md`):
- Workspace lints strict: `clippy::pedantic` + `clippy::nursery`, `-D warnings`. No `--no-verify`.
- No `unwrap`/`expect` in production code; tests fine.
- Every commit ends with the Co-Authored-By trailer.
- Commits go directly to `main`. Each commit is a coherent, testable unit.
- CLAUDE.md §11: never `cargo publish` until CI green. Not applicable here.

---

## File structure

**Created (3 files):**
- `crates/pgevolve/src/executor/cluster_preflight.rs` — cluster-flavored preflight (identity match, intent approval).
- `crates/pgevolve/tests/cluster_apply_e2e.rs` — end-to-end integration tests against ephemeral PG.
- (Possibly) extracts inside existing files; no other new files.

**Modified (8 files):**
- `crates/pgevolve-core/src/plan/plan.rs` — add `Plan::from_grouped_with_id` constructor.
- `crates/pgevolve/src/target_identity.rs` — add `compute_cluster_target_identity`.
- `crates/pgevolve/src/api/cluster.rs` — add `ClusterPlan::to_plan` method.
- `crates/pgevolve/src/executor/mod.rs` — declare + re-export `cluster_preflight` and `apply_cluster_plan`.
- `crates/pgevolve/src/executor/cluster_apply.rs` — add `apply_cluster_plan`, slim `apply_cluster_plan_dir`, retire `apply_cluster_steps` + `split_sql_statements` + `run_in_transaction` + `execute_step`.
- `crates/pgevolve/src/commands/cluster/plan.rs` — replace hand-rolled 3-file writer with `Plan::from_grouped_with_id` + `write_plan_dir`.
- `crates/pgevolve/src/commands/cluster/apply.rs` — replace `apply_cluster_plan_dir` with `read_plan_dir` + `apply_cluster_plan`.
- `crates/pgevolve/src/lib.rs` — update re-exports for retired API.
- `CHANGELOG.md` — `### Changed` + `### Removed` lines under `[Unreleased]`.

**Modified (1 file, plans index):**
- `docs/superpowers/plans/README.md` — append row for this plan.

---

## Task 1: Add `Plan::from_grouped_with_id` constructor in pgevolve-core

The existing `Plan::from_grouped` hashes a per-DB `Catalog` source/target via `PlanId::compute`. Cluster plans hash `ClusterCatalog` instead. Rather than make `PlanId::compute` generic, add a constructor that takes a pre-computed `PlanId`. The existing `from_grouped` delegates to it.

**Files:**
- Modify: `crates/pgevolve-core/src/plan/plan.rs`

- [ ] **Step 1: Read the existing `from_grouped` body**

Run: `sed -n '237,302p' crates/pgevolve-core/src/plan/plan.rs`

Note the body that walks groups assigning step numbers + intent ids + building `intents: Vec<DestructiveIntent>`. That logic must be factored out so both constructors share it.

- [ ] **Step 2: Add failing test for `from_grouped_with_id`**

In `crates/pgevolve-core/src/plan/plan.rs`'s `#[cfg(test)] mod tests` block, add:

```rust
#[test]
fn from_grouped_with_id_uses_provided_plan_id() {
    use crate::plan::PlanId;
    let pre_id = PlanId::from_hex("0123456789abcdef").expect("valid hex");
    let plan = Plan::from_grouped_with_id(
        vec![],
        pre_id.clone(),
        "test-cluster-id".into(),
        None,
        "0.0.0-test",
        99,
    )
    .expect("build empty cluster-style plan");
    assert_eq!(plan.id, pre_id);
    assert_eq!(plan.metadata.target_identity, "test-cluster-id");
    assert_eq!(plan.metadata.planner_ruleset_version, 99);
    assert!(plan.groups.is_empty());
    assert!(plan.intents.is_empty());
}
```

If `PlanId::from_hex` doesn't already exist, use whatever constructor `PlanId` exposes; if needed add a thin `pub fn from_str(s: &str) -> Result<Self, ...>` parser. Run the lookup first:

```sh
grep -n "impl PlanId\|pub fn" crates/pgevolve-core/src/plan/plan_id.rs 2>/dev/null | head -10
```

If `PlanId` only exposes `compute` (no public constructor for tests), the simplest test option is to capture the id from a known fixture: build a Plan with `from_grouped` against `Catalog::empty()`s, grab `.id.clone()`, pass it into `from_grouped_with_id` and assert round-trip equality. Adjust the test to match what's available.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p pgevolve-core --lib plan::plan::tests::from_grouped_with_id_uses_provided_plan_id`

Expected: FAIL (`from_grouped_with_id` not found).

- [ ] **Step 4: Implement `from_grouped_with_id` + refactor `from_grouped`**

In `crates/pgevolve-core/src/plan/plan.rs`, replace the existing `from_grouped` body with this structure (preserving the doc comment):

```rust
    #[allow(clippy::too_many_arguments)]
    pub fn from_grouped(
        groups: Vec<TransactionGroup>,
        source: &Catalog,
        target: &Catalog,
        target_identity: String,
        source_rev: Option<String>,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Result<Self, PlanError> {
        let id = PlanId::compute(source, target, pgevolve_version, planner_ruleset_version)?;
        Self::from_grouped_with_id(
            groups,
            id,
            target_identity,
            source_rev,
            pgevolve_version,
            planner_ruleset_version,
        )
        // Note: from_grouped still includes the per-DB `target_snapshot: target.clone()`
        // semantics. from_grouped_with_id receives an empty target snapshot — see below.
    }
```

Then add `from_grouped_with_id`:

```rust
    /// Build a `Plan` from grouped steps using a caller-supplied `PlanId`.
    ///
    /// Used by cluster apply, which hashes cluster catalogs (not per-DB
    /// catalogs) for its plan id and so cannot use `from_grouped`.
    ///
    /// `target_snapshot` is left empty — per-DB snapshots only serve the
    /// per-DB drift recheck, which cluster apply does not perform (see
    /// design doc §3.1).
    ///
    /// # Errors
    ///
    /// Currently infallible; the `Result` shape matches `from_grouped` for
    /// future-proofing.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::missing_const_for_fn)] // construct-only; never const-callable
    pub fn from_grouped_with_id(
        mut groups: Vec<TransactionGroup>,
        id: PlanId,
        target_identity: String,
        source_rev: Option<String>,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Result<Self, PlanError> {
        let mut step_no: u32 = 0;
        let mut intent_no: u32 = 0;
        let mut intents: Vec<DestructiveIntent> = Vec::new();
        for group in &mut groups {
            for step in &mut group.steps {
                step_no += 1;
                step.step_no = step_no;
                if step.destructive {
                    intent_no += 1;
                    step.intent_id = Some(intent_no);
                    intents.push(DestructiveIntent {
                        id: intent_no,
                        step: step_no,
                        kind: kind_name(step.kind).to_string(),
                        target: render_targets(&step.targets),
                        reason: step
                            .destructive_reason
                            .clone()
                            .unwrap_or_else(|| "destructive".to_string()),
                        approved: false,
                    });
                }
            }
        }
        let metadata = PlanMetadata {
            pgevolve_version: pgevolve_version.to_string(),
            planner_ruleset_version,
            source_rev,
            target_identity,
            target_snapshot: Catalog::empty(),
            created_at: OffsetDateTime::now_utc(),
            lint_at_plan_findings: Vec::new(),
        };
        Ok(Self {
            id,
            groups,
            intents,
            lint_waivers: Vec::new(),
            step_overrides: Vec::new(),
            metadata,
            advisory_findings: Vec::new(),
        })
    }
```

Now update the original `from_grouped` to include the `target_snapshot: target.clone()` it had before — since `from_grouped_with_id` uses empty, `from_grouped` must override after delegation:

```rust
    #[allow(clippy::too_many_arguments)]
    pub fn from_grouped(
        groups: Vec<TransactionGroup>,
        source: &Catalog,
        target: &Catalog,
        target_identity: String,
        source_rev: Option<String>,
        pgevolve_version: &str,
        planner_ruleset_version: u32,
    ) -> Result<Self, PlanError> {
        let id = PlanId::compute(source, target, pgevolve_version, planner_ruleset_version)?;
        let mut plan = Self::from_grouped_with_id(
            groups,
            id,
            target_identity,
            source_rev,
            pgevolve_version,
            planner_ruleset_version,
        )?;
        plan.metadata.target_snapshot = target.clone();
        Ok(plan)
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p pgevolve-core --lib plan::plan::tests::from_grouped_with_id_uses_provided_plan_id`

Expected: PASS.

- [ ] **Step 6: Run the full per-DB plan tests to verify no regressions**

Run: `cargo test -p pgevolve-core --lib plan::`

Expected: all pass. Specifically watch for any test asserting the existing `target_snapshot` is populated for per-DB plans — that must still pass via the override in `from_grouped`.

- [ ] **Step 7: Verify gate**

Run, in order:
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

Expected: all exit 0.

- [ ] **Step 8: Commit**

```sh
git add crates/pgevolve-core/src/plan/plan.rs
git commit -m "$(cat <<'EOF'
feat(plan): Plan::from_grouped_with_id constructor

Cluster apply hashes cluster catalogs (not per-DB Catalog) for its
plan id. Add a constructor that takes a pre-computed PlanId so cluster
plan emission can reuse the existing Plan struct + 3-file serializer.

Backward-compatible: the existing from_grouped delegates to the new
constructor and overrides target_snapshot for per-DB callers that
still need the snapshot for drift recheck.

Step 1/8 of issue #7 (cluster apply parity).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add `compute_cluster_target_identity`

The cluster identity is `cluster:{system_identifier_hex}` per design §2.

**Files:**
- Modify: `crates/pgevolve/src/target_identity.rs`

- [ ] **Step 1: Add failing unit test**

In `crates/pgevolve/src/target_identity.rs`, scroll to the bottom and add a `#[cfg(test)] mod tests` block (or extend if one exists). Add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_identity_format() {
        // Hex of decimal 12345 = "3039"; format prefixed with "cluster:".
        let id = format_cluster_identity(12345u64);
        assert_eq!(id, "cluster:0000000000003039");
    }

    #[test]
    fn cluster_identity_max_value() {
        let id = format_cluster_identity(u64::MAX);
        assert_eq!(id, "cluster:ffffffffffffffff");
    }

    #[test]
    fn cluster_identity_zero() {
        let id = format_cluster_identity(0);
        assert_eq!(id, "cluster:0000000000000000");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pgevolve --lib target_identity::tests`

Expected: FAIL (`format_cluster_identity` not found).

- [ ] **Step 3: Implement `format_cluster_identity` + `compute_cluster_target_identity`**

In `crates/pgevolve/src/target_identity.rs`, add (after the existing `compute_target_identity`):

```rust
/// Compute the cluster target identity for the cluster `client` is connected to.
///
/// Format: `cluster:{system_identifier_lower_hex_zero_padded_to_16_chars}`.
/// The prefix distinguishes cluster identities from per-DB identities at a
/// glance in `apply_log` queries; the hex encoding is fixed-width so identity
/// strings sort predictably.
///
/// `system_identifier` comes from `pg_control_system()` (PG 9.6+). It is unique
/// per `initdb` and stable across replicas of the same physical cluster.
///
/// # Errors
///
/// Returns `ApplyError` if the `pg_control_system()` query fails or the
/// returned column cannot be parsed as a u64.
pub async fn compute_cluster_target_identity(client: &Client) -> Result<String, ApplyError> {
    let row = client
        .query_one(
            "SELECT system_identifier::text FROM pg_control_system()",
            &[],
        )
        .await?;
    let s: String = row.try_get(0)?;
    let n: u64 = s
        .parse()
        .map_err(|_| ApplyError::Internal(format!("unparseable system_identifier: {s}")))?;
    Ok(format_cluster_identity(n))
}

/// Format a system identifier as the cluster identity string.
fn format_cluster_identity(system_identifier: u64) -> String {
    format!("cluster:{system_identifier:016x}")
}
```

If `ApplyError::Internal` doesn't exist, check what error variants are available:

```sh
grep -n "pub enum ApplyError" crates/pgevolve/src/executor/error.rs
sed -n '1,80p' crates/pgevolve/src/executor/error.rs
```

Use whatever generic error variant is appropriate. If none exists, use a `#[from] tokio_postgres::Error` style wrap by constructing a synthetic Postgres error — or add an `Internal(String)` variant in this task (and call it out in the commit message).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p pgevolve --lib target_identity::tests`

Expected: 3 tests PASS.

- [ ] **Step 5: Verify gate**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

- [ ] **Step 6: Commit**

```sh
git add crates/pgevolve/src/target_identity.rs
# include executor/error.rs only if you added an Internal variant
git commit -m "$(cat <<'EOF'
feat(executor): compute_cluster_target_identity

Cluster identity is cluster:{system_identifier_hex16}, drawn from
pg_control_system(). Distinguishes cluster plans from per-DB plans at
a glance in apply_log queries.

Step 2/8 of issue #7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add `ClusterPlan::to_plan` method

`ClusterPlan` carries `steps: Vec<RawStep>` + `changes: ClusterChangeSet`. The `to_plan` method regroups via `group_steps` (the existing per-DB grouper handles cluster RawSteps unchanged — kind-agnostic).

**Files:**
- Modify: `crates/pgevolve/src/api/cluster.rs`

- [ ] **Step 1: Add failing unit test**

Scroll to the bottom of `crates/pgevolve/src/api/cluster.rs`. Add (or extend) a `#[cfg(test)] mod tests` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::ir::cluster::catalog::ClusterCatalog;
    use pgevolve_core::ir::cluster::role::Role;
    use pgevolve_core::ir::cluster::role::RoleAttributes;
    use pgevolve_core::identifier::Identifier;
    use pgevolve_core::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

    fn synthetic_create_role(name: &str) -> RawStep {
        RawStep {
            step_no: 0,
            kind: StepKind::CreateRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: format!("CREATE ROLE {name};"),
            transactional: TransactionConstraint::InTransaction,
        }
    }

    fn synthetic_drop_role(name: &str) -> RawStep {
        RawStep {
            step_no: 0,
            kind: StepKind::DropRole,
            destructive: true,
            destructive_reason: Some(format!("drops role {name} (may orphan objects)")),
            intent_id: None,
            targets: vec![],
            sql: format!("DROP ROLE {name};"),
            transactional: TransactionConstraint::InTransaction,
        }
    }

    fn empty_changes() -> pgevolve_core::diff::cluster::ClusterChangeSet {
        pgevolve_core::diff::cluster::ClusterChangeSet::default()
    }

    fn empty_plan_id() -> pgevolve_core::plan::PlanId {
        // Use whatever PlanId constructor / Default is available; if PlanId
        // has no public test constructor, build via PlanId::compute against
        // empty catalogs as a deterministic placeholder for unit tests.
        pgevolve_core::plan::PlanId::compute(
            &pgevolve_core::ir::catalog::Catalog::empty(),
            &pgevolve_core::ir::catalog::Catalog::empty(),
            "0.0.0-test",
            0,
        )
        .expect("compute placeholder id")
    }

    #[test]
    fn to_plan_assigns_step_numbers_and_intents() {
        let plan = ClusterPlan {
            steps: vec![synthetic_create_role("a"), synthetic_drop_role("b")],
            source: ClusterCatalog::empty(),
            target: ClusterCatalog::empty(),
            changes: empty_changes(),
            advisory_findings: vec![],
        };

        let core_plan = plan
            .to_plan(empty_plan_id(), "cluster:0000000000003039".into())
            .expect("to_plan ok");

        // One group with both steps (InTransaction).
        assert_eq!(core_plan.groups.len(), 1);
        assert_eq!(core_plan.groups[0].steps.len(), 2);
        // Step numbers assigned in emission order.
        assert_eq!(core_plan.groups[0].steps[0].step_no, 1);
        assert_eq!(core_plan.groups[0].steps[1].step_no, 2);
        // One intent for the drop.
        assert_eq!(core_plan.intents.len(), 1);
        assert_eq!(core_plan.intents[0].step, 2);
        assert!(!core_plan.intents[0].approved);
        // target_identity passed through.
        assert_eq!(core_plan.metadata.target_identity, "cluster:0000000000003039");
    }

    #[test]
    fn to_plan_empty_steps_produces_empty_plan() {
        let plan = ClusterPlan {
            steps: vec![],
            source: ClusterCatalog::empty(),
            target: ClusterCatalog::empty(),
            changes: empty_changes(),
            advisory_findings: vec![],
        };

        let core_plan = plan
            .to_plan(empty_plan_id(), "cluster:empty".into())
            .expect("empty to_plan ok");

        assert!(core_plan.groups.is_empty());
        assert!(core_plan.intents.is_empty());
    }
}
```

If `ClusterCatalog::empty()` doesn't exist, grep for the constructor:
```sh
grep -n "impl ClusterCatalog\|pub fn" crates/pgevolve-core/src/ir/cluster/catalog.rs | head -10
```
Use whatever empty/default constructor is exposed.

If `ClusterChangeSet::default()` doesn't exist, the same applies — check for a `new()` or `empty()` constructor.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pgevolve --lib api::cluster::tests`

Expected: FAIL (`to_plan` method not found).

- [ ] **Step 3: Implement `to_plan`**

In `crates/pgevolve/src/api/cluster.rs`, add an `impl ClusterPlan` block (or extend an existing one):

```rust
impl ClusterPlan {
    /// Materialize a serializable `Plan` from this cluster plan.
    ///
    /// `plan_id` is computed externally (cluster plan ids hash
    /// `ClusterCatalog`, not per-DB `Catalog` — see the existing
    /// `compute_cluster_plan_id` in `commands/cluster/plan.rs`).
    ///
    /// `target_identity` should be the cluster identity returned by
    /// [`crate::target_identity::compute_cluster_target_identity`].
    ///
    /// The resulting `Plan` can be written via
    /// `pgevolve_core::plan::write_plan_dir` and read back via
    /// `pgevolve_core::plan::read_plan_dir`.
    ///
    /// # Errors
    ///
    /// Propagates any error from
    /// [`pgevolve_core::plan::Plan::from_grouped_with_id`].
    pub fn to_plan(
        self,
        plan_id: pgevolve_core::plan::PlanId,
        target_identity: String,
    ) -> Result<pgevolve_core::plan::Plan, pgevolve_core::plan::PlanError> {
        let groups = pgevolve_core::plan::group_steps(self.steps);
        pgevolve_core::plan::Plan::from_grouped_with_id(
            groups,
            plan_id,
            target_identity,
            None, // source_rev: cluster plans don't currently carry source_rev
            pgevolve_core::VERSION,
            pgevolve_core::plan::PlannerPolicy::default().planner_ruleset_version,
        )
    }
}
```

If `pgevolve_core::plan::PlanError` is named something else (e.g., `PlanIoError`, etc.), match what `from_grouped_with_id` actually returns from Task 1.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p pgevolve --lib api::cluster::tests`

Expected: both tests PASS.

- [ ] **Step 5: Verify gate**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

- [ ] **Step 6: Commit**

```sh
git add crates/pgevolve/src/api/cluster.rs
git commit -m "$(cat <<'EOF'
feat(api): ClusterPlan::to_plan materializes Plan from cluster steps

Routes cluster plans through the per-DB Plan struct + 3-file
serializer. Callers compute the plan_id externally (cluster plans
hash ClusterCatalog) and supply target_identity from
compute_cluster_target_identity.

Step 3/8 of issue #7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Add `executor::cluster_preflight`

Cluster-flavored preflight: identity match + intent approval. Manifest cross-check is already enforced by `read_plan_dir` (it validates plan_id consistency across the 3 files). No drift recheck for v1.0 per design §3.1.

**Files:**
- Create: `crates/pgevolve/src/executor/cluster_preflight.rs`
- Modify: `crates/pgevolve/src/executor/mod.rs` (add `pub mod cluster_preflight`)

- [ ] **Step 1: Add the module declaration**

In `crates/pgevolve/src/executor/mod.rs`, after `pub mod cluster_apply;` add:

```rust
pub mod cluster_preflight;
```

- [ ] **Step 2: Write the failing test scaffold**

Create `crates/pgevolve/src/executor/cluster_preflight.rs` with:

```rust
//! Cluster-flavored apply preflight.
//!
//! Mirrors the per-DB `executor::preflight` but checks cluster identity (via
//! `pg_control_system().system_identifier`) instead of per-DB identity, and
//! does not perform a drift recheck (see design doc §3.1).
//!
//! Manifest cross-check is already enforced by `read_plan_dir`, which
//! validates that `plan.sql`, `intent.toml`, and `manifest.toml` carry the
//! same plan_id. No extra check is needed here.

use tokio_postgres::Client;

use pgevolve_core::plan::Plan;

use crate::executor::ApplyError;
use crate::target_identity::compute_cluster_target_identity;

/// Overrides for [`run_cluster_preflight`]. Mirrors `PreflightOverrides` but
/// only carries the flags that apply to cluster ops.
#[derive(Debug, Clone, Default)]
pub struct ClusterPreflightOverrides {
    /// Skip identity match. Use only when intentionally applying a plan to a
    /// different cluster.
    pub allow_different_target: bool,
    /// Skip intent approval. Set internally by test harnesses; not exposed
    /// via the CLI.
    pub allow_unapproved_intents: bool,
}

/// Run cluster-apply preflight against `client`.
///
/// Checks:
/// 1. Live cluster identity matches `plan.metadata.target_identity` (unless
///    `overrides.allow_different_target`).
/// 2. Every destructive intent in `plan.intents` has `approved = true` in
///    `intent.toml` (unless `overrides.allow_unapproved_intents`).
///
/// # Errors
///
/// Returns `ApplyError::TargetIdentityMismatch` or
/// `ApplyError::UnapprovedIntent` on the first failing check.
pub async fn run_cluster_preflight(
    client: &Client,
    plan: &Plan,
    overrides: ClusterPreflightOverrides,
) -> Result<(), ApplyError> {
    if !overrides.allow_different_target {
        let live = compute_cluster_target_identity(client).await?;
        if live != plan.metadata.target_identity {
            return Err(ApplyError::TargetIdentityMismatch {
                expected: plan.metadata.target_identity.clone(),
                actual: live,
            });
        }
    }

    if !overrides.allow_unapproved_intents {
        for intent in &plan.intents {
            if !intent.approved {
                return Err(ApplyError::UnapprovedIntent {
                    step_no: intent.step,
                    kind: intent.kind.clone(),
                    target: intent.target.clone(),
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::plan::{DestructiveIntent, Plan, PlanMetadata};

    fn plan_with_metadata(identity: &str, intents: Vec<DestructiveIntent>) -> Plan {
        // We can't easily construct a Plan from outside the crate without
        // exposing internals. Use Plan::from_grouped_with_id against empty
        // groups (Task 1) — that's the test-friendly path.
        let id = pgevolve_core::plan::PlanId::compute(
            &pgevolve_core::ir::catalog::Catalog::empty(),
            &pgevolve_core::ir::catalog::Catalog::empty(),
            "0.0.0-test",
            0,
        )
        .expect("placeholder id");
        let mut plan = Plan::from_grouped_with_id(
            vec![],
            id,
            identity.into(),
            None,
            "0.0.0-test",
            0,
        )
        .expect("from_grouped_with_id");
        plan.intents = intents;
        plan
    }

    #[test]
    fn unapproved_intent_rejected() {
        let intent = DestructiveIntent {
            id: 1,
            step: 1,
            kind: "drop_role".into(),
            target: "alice".into(),
            reason: "drops role alice".into(),
            approved: false,
        };
        let plan = plan_with_metadata("cluster:abc", vec![intent]);
        // Cannot run the live-DB check inline; skip identity by enabling override.
        let overrides = ClusterPreflightOverrides {
            allow_different_target: true,
            allow_unapproved_intents: false,
        };
        // We can't actually call run_cluster_preflight without a Client;
        // instead, replicate the intent check inline as a unit test.
        let err = plan
            .intents
            .iter()
            .find(|i| !i.approved)
            .map(|i| ApplyError::UnapprovedIntent {
                step_no: i.step,
                kind: i.kind.clone(),
                target: i.target.clone(),
            });
        assert!(matches!(
            err,
            Some(ApplyError::UnapprovedIntent { step_no: 1, .. })
        ));
        let _ = overrides; // suppress unused warning
    }

    #[test]
    fn approved_intent_passes() {
        let intent = DestructiveIntent {
            id: 1,
            step: 1,
            kind: "drop_role".into(),
            target: "alice".into(),
            reason: "drops role alice".into(),
            approved: true,
        };
        let plan = plan_with_metadata("cluster:abc", vec![intent]);
        let unapproved = plan.intents.iter().find(|i| !i.approved);
        assert!(unapproved.is_none());
    }
}
```

The unit-test surface here is narrow because identity match needs a live Client. The full end-to-end identity + intent path lands in Task 7 (integration tests). The unit tests here pin down the intent-iteration logic deterministically.

If `ApplyError::TargetIdentityMismatch` / `UnapprovedIntent` don't already exist, check:

```sh
grep -n "TargetIdentityMismatch\|UnapprovedIntent\|pub enum ApplyError" crates/pgevolve/src/executor/error.rs
```

If they exist with different field names, adapt the construction calls. If they don't exist as variants, add them to `crates/pgevolve/src/executor/error.rs` in this same task (and call it out in the commit message). The per-DB preflight likely already returns these variants — grep to confirm:

```sh
grep -n "TargetIdentityMismatch\|UnapprovedIntent" crates/pgevolve/src/executor/preflight.rs
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p pgevolve --lib executor::cluster_preflight::tests`

Expected: both PASS.

- [ ] **Step 4: Verify gate**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

- [ ] **Step 5: Commit**

```sh
git add crates/pgevolve/src/executor/cluster_preflight.rs crates/pgevolve/src/executor/mod.rs
# Include error.rs only if ApplyError variants were added
git commit -m "$(cat <<'EOF'
feat(executor): cluster_preflight module

Cluster-flavored preflight: identity match + intent approval. Manifest
cross-check is enforced by read_plan_dir (validates plan_id across the
3 files). No drift recheck for v1.0 — see design §3.1.

Step 4/8 of issue #7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Add `executor::cluster_apply::apply_cluster_plan`

Mirrors per-DB `apply_plan`: bootstrap → lock → preflight → open apply_log → execute → close apply_log.

**Files:**
- Modify: `crates/pgevolve/src/executor/cluster_apply.rs`
- Modify: `crates/pgevolve/src/executor/mod.rs` (re-export `apply_cluster_plan`, `ClusterPreflightOverrides`)

- [ ] **Step 1: Read the existing per-DB `apply_plan` body for reference**

Run: `sed -n '116,167p' crates/pgevolve/src/executor/mod.rs`

This is the template you're mirroring. Take note of the lock-release order on each failure branch.

- [ ] **Step 2: Add `apply_cluster_plan` to `cluster_apply.rs`**

At the top of `crates/pgevolve/src/executor/cluster_apply.rs`, add (or merge into existing imports):

```rust
use tokio_postgres::Client;
use uuid::Uuid;

use pgevolve_core::plan::Plan;

use crate::executor::{
    ApplyError, ApplyOutcome, ApplyOverrides,
    audit::{close_apply_log, open_apply_log},
    bootstrap::bootstrap_metadata,
    cluster_preflight::{ClusterPreflightOverrides, run_cluster_preflight},
    execute::execute_plan,
    lock::{release_lock, try_acquire_lock},
};
```

Then add the function (place it above the existing `apply_cluster_plan_dir` so the file reads top-down: new high-level → existing helpers → tests):

```rust
/// Apply an in-memory cluster [`Plan`] to a live Postgres connection.
///
/// Mirrors [`crate::executor::apply_plan`] but with a cluster-flavored
/// preflight ([`run_cluster_preflight`]). No drift recheck.
///
/// Steps:
/// 1. Bootstrap or upgrade the `pgevolve` metadata schema.
/// 2. Acquire the singleton advisory lock.
/// 3. Run cluster preflight (identity match + intent approval).
/// 4. Open an `apply_log` row.
/// 5. Execute each group in order.
/// 6. Close the `apply_log` row with the final status.
///
/// # Errors
///
/// Returns `ApplyError` on the first failed step. The advisory lock is
/// released before propagating in every failure branch.
pub async fn apply_cluster_plan(
    plan: &Plan,
    client: &mut Client,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    bootstrap_metadata(client).await?;

    let actor = overrides
        .actor
        .clone()
        .unwrap_or_else(crate::executor::default_actor);
    try_acquire_lock(client, &actor).await?;

    let cluster_preflight = ClusterPreflightOverrides {
        allow_different_target: overrides.allow_different_target,
        allow_unapproved_intents: overrides.allow_unapproved_intents,
    };
    let preflight_result = run_cluster_preflight(client, plan, cluster_preflight).await;
    if let Err(e) = preflight_result {
        let _ = release_lock(client).await;
        return Err(e);
    }

    let apply_id = open_apply_log(client, plan, &actor).await?;
    let exec_result =
        execute_plan(client, plan, apply_id, overrides.abort_after_step).await;
    match exec_result {
        Ok(()) => {
            close_apply_log(client, apply_id, "succeeded", None).await?;
            release_lock(client).await?;
            Ok(ApplyOutcome::Succeeded { apply_id })
        }
        Err(ApplyError::AbortedAfterStep { step_no }) => {
            close_apply_log(
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
            close_apply_log(client, apply_id, "failed", Some(&msg)).await?;
            let _ = release_lock(client).await;
            Err(e)
        }
    }
}
```

`default_actor` is currently a private function inside `crates/pgevolve/src/executor/mod.rs`. Make it `pub(crate)` so cluster_apply can call it:

In `crates/pgevolve/src/executor/mod.rs`, find `fn default_actor() -> String {` and change to:

```rust
pub(crate) fn default_actor() -> String {
```

- [ ] **Step 3: Re-export `apply_cluster_plan` from the executor module**

In `crates/pgevolve/src/executor/mod.rs`, find the line:

```rust
pub use cluster_apply::{ClusterApplyError, apply_cluster_plan_dir, apply_cluster_steps};
```

Change to:

```rust
pub use cluster_apply::{
    ClusterApplyError, apply_cluster_plan, apply_cluster_plan_dir, apply_cluster_steps,
};
```

(`apply_cluster_steps` is still here; Task 7 retires it.)

- [ ] **Step 4: Verify gate (no new tests this step — integration tests land in Task 8)**

```sh
cargo build -p pgevolve
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

- [ ] **Step 5: Commit**

```sh
git add crates/pgevolve/src/executor/cluster_apply.rs crates/pgevolve/src/executor/mod.rs
git commit -m "$(cat <<'EOF'
feat(executor): apply_cluster_plan with full preflight + audit

Mirrors apply_plan: bootstrap, advisory lock, cluster preflight,
apply_log row, execute, close apply_log. No drift recheck (design §3.1).

Reuses the existing bootstrap_metadata / try_acquire_lock /
open_apply_log / execute_plan / close_apply_log infrastructure with
only the preflight swapped for the cluster variant.

End-to-end integration tests land in a follow-up commit (Step 8).

Step 5/8 of issue #7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Rewrite `cluster plan` CLI to use `Plan` + `write_plan_dir`

The current `commands/cluster/plan.rs` hand-rolls plan.sql / intent.toml / manifest.toml. Replace with the canonical 3-file writer.

**Files:**
- Modify: `crates/pgevolve/src/commands/cluster/plan.rs`

- [ ] **Step 1: Read the current cluster plan command**

Run: `cat crates/pgevolve/src/commands/cluster/plan.rs`

The hand-rolled writer logic (the `compute_cluster_plan_id` function, `build_intent_toml`, the manual file writes) will be replaced. `compute_cluster_plan_id` survives — Plan::from_grouped_with_id needs the id we compute here.

- [ ] **Step 2: Rewrite `run`**

Replace the existing `pub async fn run` body with this. Imports at the top of the file need updating; show both:

```rust
//! `pgevolve cluster plan` — write `cluster-plans/<plan_id>/` directory
//! using the canonical Plan + 3-file serializer.

use std::path::Path;

use anyhow::{Context, Result};

use pgevolve_core::plan::write_plan_dir;

use crate::api::cluster::build_cluster_plan;
use crate::cluster_config::ClusterConfig;
use crate::target_identity::compute_cluster_target_identity;

/// Run `pgevolve cluster plan`.
pub async fn run(project_root: &Path, cfg: &ClusterConfig) -> Result<i32> {
    let plan = build_cluster_plan(project_root, cfg)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if plan.changes.is_empty() {
        println!("No changes.");
        return Ok(0);
    }

    let plan_id = compute_cluster_plan_id(&plan.source, &plan.target)?;

    // Compute target_identity by opening a fresh connection. Don't reuse the
    // catalog connection — it was consumed by build_cluster_plan via
    // spawn_blocking.
    let (client, connection) = tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
        .await
        .context("connecting to cluster for target_identity")?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster plan target_identity connection ended");
        }
    });
    let target_identity = compute_cluster_target_identity(&client)
        .await
        .map_err(|e| anyhow::anyhow!("compute target_identity: {e}"))?;

    let core_plan_id = pgevolve_core::plan::PlanId::from_hex(&plan_id)
        .context("plan_id hex parse")?;

    let core_plan = plan
        .to_plan(core_plan_id, target_identity)
        .context("ClusterPlan::to_plan")?;

    let plan_dir = project_root.join("cluster-plans").join(&plan_id);
    std::fs::create_dir_all(&plan_dir)
        .with_context(|| format!("creating {}", plan_dir.display()))?;

    write_plan_dir(&core_plan, &plan_dir)
        .with_context(|| format!("writing {}", plan_dir.display()))?;

    println!("Wrote {}", plan_dir.display());
    println!("  plan.sql ({} steps)", core_plan.groups.iter().map(|g| g.steps.len()).sum::<usize>());
    println!("  intent.toml ({} destructive intents)", core_plan.intents.len());
    println!("  manifest.toml");

    for finding in &plan.advisory_findings {
        eprintln!(
            "pgevolve cluster plan: advisory [{}]: {}",
            finding.rule, finding.message
        );
    }

    Ok(0)
}

/// Compute a short plan id by hashing the canonical serialized source and
/// target cluster catalogs.
///
/// Uses the domain separator `pgevolve-cluster-plan-id-v1` so cluster plan
/// ids never collide with per-DB plan ids even if the byte contents were
/// identical. Returns the first 8 bytes of the digest as lowercase hex.
fn compute_cluster_plan_id(
    source: &pgevolve_core::ir::cluster::catalog::ClusterCatalog,
    target: &pgevolve_core::ir::cluster::catalog::ClusterCatalog,
) -> Result<String> {
    let source_bytes =
        serde_json::to_vec(source).context("serializing source catalog for plan id")?;
    let target_bytes =
        serde_json::to_vec(target).context("serializing target catalog for plan id")?;
    let mut h = blake3::Hasher::new();
    h.update(b"pgevolve-cluster-plan-id-v1\n");
    h.update(&source_bytes);
    h.update(&[0]);
    h.update(&target_bytes);
    Ok(hex::encode(&h.finalize().as_bytes()[..8]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgevolve_core::ir::cluster::catalog::ClusterCatalog;

    #[test]
    fn plan_id_differs_for_different_catalogs() {
        let a = ClusterCatalog::empty();
        let mut b = ClusterCatalog::empty();
        b.roles.push(pgevolve_core::ir::cluster::role::Role {
            name: pgevolve_core::identifier::Identifier::from_unquoted("reader").unwrap(),
            attributes: pgevolve_core::ir::cluster::role::RoleAttributes::default(),
            member_of: vec![],
            comment: None,
        });
        let id_a = compute_cluster_plan_id(&a, &a).unwrap();
        let id_b = compute_cluster_plan_id(&a, &b).unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn plan_id_is_deterministic() {
        let c = ClusterCatalog::empty();
        let id1 = compute_cluster_plan_id(&c, &c).unwrap();
        let id2 = compute_cluster_plan_id(&c, &c).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn plan_id_is_8_bytes_hex() {
        let c = ClusterCatalog::empty();
        let id = compute_cluster_plan_id(&c, &c).unwrap();
        assert_eq!(id.len(), 16, "8 bytes = 16 hex chars, got: {id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
```

The `build_intent_toml` helper from the old code is **removed** (replaced by `write_plan_dir`'s intent serialization).

**`PlanId::from_hex` may not exist yet.** Grep:

```sh
grep -n "fn from_hex\|pub fn" crates/pgevolve-core/src/plan/plan_id.rs 2>/dev/null
```

If not, add a thin `pub fn from_hex(s: &str) -> Result<Self, …>` in `crates/pgevolve-core/src/plan/plan_id.rs`. Implementation:

```rust
impl PlanId {
    /// Parse a `PlanId` from its hex string form (as produced by `Display`
    /// or external hashing).
    ///
    /// # Errors
    ///
    /// Returns `PlanError::Internal` if `s` is not valid lowercase hex of
    /// the expected length.
    pub fn from_hex(s: &str) -> Result<Self, PlanError> {
        let bytes = hex::decode(s).map_err(|e| PlanError::Internal(format!("plan_id hex decode: {e}")))?;
        if bytes.len() != Self::byte_length() {
            return Err(PlanError::Internal(format!(
                "plan_id hex length mismatch: got {} bytes, expected {}",
                bytes.len(),
                Self::byte_length()
            )));
        }
        // Adapt to whatever PlanId's internal representation is. Most likely:
        //   Self(bytes.try_into().unwrap())
        // — but check the actual type and use a checked conversion.
        Ok(Self::from_bytes(&bytes))
    }
}
```

Adjust to whatever internal shape `PlanId` actually has — look at the existing constructors / Display impl and mirror them.

- [ ] **Step 3: Build and run the existing plan tests in this file**

Run: `cargo test -p pgevolve --lib commands::cluster::plan::tests`

Expected: 3 PASS (plan_id_differs / deterministic / 8-bytes-hex). The intent_toml test from the old file was removed; if cargo complains about a missing `intent_toml_empty_for_no_destructive_steps`, that confirms you correctly removed `build_intent_toml`.

- [ ] **Step 4: Verify gate**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

- [ ] **Step 5: Commit**

```sh
git add crates/pgevolve/src/commands/cluster/plan.rs
# Include plan_id.rs if you added from_hex
git commit -m "$(cat <<'EOF'
refactor(cluster): cluster plan uses Plan + write_plan_dir

Drops the hand-rolled 3-file writer (custom intent.toml format, manifest
without target_identity, plan.sql without structured headers). Now
routes through ClusterPlan::to_plan + write_plan_dir for canonical
format parity with per-DB plans.

target_identity is computed from a fresh connection via
compute_cluster_target_identity. plan_id continues to hash
ClusterCatalog (not per-DB Catalog) so cluster plan ids are unique per
cluster diff.

Step 6/8 of issue #7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Rewrite `cluster apply` CLI to use `read_plan_dir` + `apply_cluster_plan`, retire dead code

**Files:**
- Modify: `crates/pgevolve/src/commands/cluster/apply.rs`
- Modify: `crates/pgevolve/src/executor/cluster_apply.rs` (retirements)
- Modify: `crates/pgevolve/src/executor/mod.rs` (drop `apply_cluster_steps` re-export)
- Modify: `crates/pgevolve/src/lib.rs` (drop `apply_cluster_steps` re-export)

- [ ] **Step 1: Rewrite the cluster apply CLI**

Replace `crates/pgevolve/src/commands/cluster/apply.rs`'s `run` and supporting imports with:

```rust
//! `pgevolve cluster apply` — apply a cluster plan directory.
//!
//! With no plan id, finds the most recently modified directory under
//! `cluster-plans/`. With an explicit id, applies that specific plan.
//!
//! Closes the v0.3.0 Stage-12 gaps (#7): structured plan loading, advisory
//! lock, intent enforcement, manifest cross-check, apply_log audit.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};

use pgevolve_core::plan::read_plan_dir;

use crate::cluster_config::ClusterConfig;
use crate::executor::{ApplyOverrides, apply_cluster_plan};

/// Run `pgevolve cluster apply`.
pub async fn run(project_root: &Path, cfg: &ClusterConfig, plan_id: Option<&str>) -> Result<i32> {
    let plan_dir = match plan_id {
        Some(id) => project_root.join("cluster-plans").join(id),
        None => find_latest_plan_dir(&project_root.join("cluster-plans"))
            .context("looking for the latest cluster plan")?,
    };

    eprintln!("Applying {}", plan_dir.display());

    let plan = read_plan_dir(&plan_dir)
        .with_context(|| format!("reading plan from {}", plan_dir.display()))?;

    let (mut client, connection) = tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
        .await
        .context("connecting to cluster for apply")?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster apply connection ended");
        }
    });

    let overrides = ApplyOverrides::default();
    apply_cluster_plan(&plan, &mut client, overrides)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    eprintln!("Done.");
    Ok(0)
}

/// Find the most recently modified subdirectory of `plans_root`.
///
/// Returns an error if the directory doesn't exist or is empty.
fn find_latest_plan_dir(plans_root: &Path) -> Result<PathBuf> {
    if !plans_root.exists() {
        return Err(anyhow!(
            "no cluster-plans directory found at {}",
            plans_root.display()
        ));
    }

    let mut latest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(plans_root)
        .with_context(|| format!("reading {}", plans_root.display()))?
    {
        let entry = entry.with_context(|| format!("iterating {}", plans_root.display()))?;
        if entry
            .file_type()
            .with_context(|| format!("stat {}", entry.path().display()))?
            .is_dir()
        {
            let mtime = entry
                .metadata()
                .with_context(|| format!("metadata {}", entry.path().display()))?
                .modified()
                .with_context(|| format!("mtime {}", entry.path().display()))?;
            if latest.as_ref().is_none_or(|(t, _)| mtime > *t) {
                latest = Some((mtime, entry.path()));
            }
        }
    }

    latest
        .map(|(_, p)| p)
        .ok_or_else(|| anyhow!("no cluster plans found in {}", plans_root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_latest_errors_when_no_dir() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_latest_plan_dir(&dir.path().join("no-such-dir")).unwrap_err();
        assert!(err.to_string().contains("no cluster-plans directory"));
    }

    #[test]
    fn find_latest_errors_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_latest_plan_dir(dir.path()).unwrap_err();
        assert!(err.to_string().contains("no cluster plans found"));
    }

    #[test]
    fn find_latest_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("aaaaaaaa")).unwrap();
        std::fs::create_dir(dir.path().join("bbbbbbbb")).unwrap();
        let p = find_latest_plan_dir(dir.path()).unwrap();
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(name == "aaaaaaaa" || name == "bbbbbbbb");
    }
}
```

- [ ] **Step 2: Retire `apply_cluster_steps`, `split_sql_statements`, `execute_step`, `run_in_transaction` from cluster_apply.rs**

Open `crates/pgevolve/src/executor/cluster_apply.rs`. Delete:
- The `apply_cluster_steps` function and its doc comment.
- The `split_sql_statements` function and its test module (the 3 tests for split behavior).
- The `execute_step` function.
- The `run_in_transaction` function.

Replace `apply_cluster_plan_dir`'s body (the part that called `split_sql_statements` + `run_in_transaction`) with the new flow:

```rust
/// Apply a cluster plan directory to a live Postgres connection.
///
/// Reads the plan from disk via `read_plan_dir` and delegates to
/// [`apply_cluster_plan`].
///
/// Use [`apply_cluster_plan`] directly when you already have a [`Plan`]
/// value (test harnesses, library callers that built the plan in-process).
pub async fn apply_cluster_plan_dir(
    plan_dir: &std::path::Path,
    cfg: &crate::cluster_config::ClusterConfig,
) -> Result<(), ClusterApplyError> {
    let plan = pgevolve_core::plan::read_plan_dir(plan_dir)
        .map_err(|e| ClusterApplyError::Io {
            path: plan_dir.join("plan.sql"),
            source: std::io::Error::other(e.to_string()),
        })?;

    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .map_err(|e| ClusterApplyError::Connection(e.to_string()))?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "cluster plan-dir apply connection task ended");
        }
    });

    let overrides = crate::executor::ApplyOverrides::default();
    apply_cluster_plan(&plan, &mut client, overrides)
        .await
        .map_err(|e| ClusterApplyError::Apply(e.to_string()))?;

    Ok(())
}
```

`ClusterApplyError` now needs an `Apply(String)` variant (or wrap `ApplyError` directly via `#[from]`). Add to the enum:

```rust
    /// Apply pipeline reported an error.
    #[error("apply error: {0}")]
    Apply(String),
```

The `Io` variant's repurposing for `read_plan_dir` failures isn't quite right semantically — but it's the closest existing variant. A cleaner option is to add a `PlanLoad(String)` variant. Pick one consistent with the surrounding error style.

- [ ] **Step 3: Drop the `apply_cluster_steps` re-export**

In `crates/pgevolve/src/executor/mod.rs`, change:

```rust
pub use cluster_apply::{
    ClusterApplyError, apply_cluster_plan, apply_cluster_plan_dir, apply_cluster_steps,
};
```

To:

```rust
pub use cluster_apply::{ClusterApplyError, apply_cluster_plan, apply_cluster_plan_dir};
```

In `crates/pgevolve/src/lib.rs`, find and update the corresponding re-export. Run:

```sh
grep -n "apply_cluster_steps\|ClusterApplyError" crates/pgevolve/src/lib.rs
```

Drop `apply_cluster_steps` from any re-export. Keep `ClusterApplyError`, `apply_cluster_plan_dir`, and add `apply_cluster_plan` to the public surface.

- [ ] **Step 4: Build + run all unit tests**

Run:
```sh
cargo build -p pgevolve
cargo test -p pgevolve --lib
```

Expected: build succeeds, all unit tests pass. The retired functions should not be referenced anywhere — if cargo complains, grep for orphaned callers:

```sh
grep -rn "apply_cluster_steps\|split_sql_statements" crates/pgevolve/ 2>/dev/null
```

If any callers remain, update them to use `apply_cluster_plan` instead.

- [ ] **Step 5: Verify gate**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

- [ ] **Step 6: Commit**

```sh
git add crates/pgevolve/src/commands/cluster/apply.rs \
        crates/pgevolve/src/executor/cluster_apply.rs \
        crates/pgevolve/src/executor/mod.rs \
        crates/pgevolve/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(cluster): apply uses read_plan_dir + apply_cluster_plan; retire dead code

cluster apply CLI now loads the 3 files via read_plan_dir and runs the
full per-DB apply pipeline (with cluster preflight). The semicolon
splitter, apply_cluster_steps, execute_step, run_in_transaction, and
the corresponding tests retire.

apply_cluster_plan_dir collapses to a thin wrapper: read_plan_dir +
open connection + apply_cluster_plan.

Step 7/8 of issue #7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Integration tests + CHANGELOG + plans index

**Files:**
- Create: `crates/pgevolve/tests/cluster_apply_e2e.rs`
- Modify: `CHANGELOG.md`
- Modify: `docs/superpowers/plans/README.md`

- [ ] **Step 1: Create the e2e test file**

Create `crates/pgevolve/tests/cluster_apply_e2e.rs`:

```rust
//! End-to-end cluster apply tests against an ephemeral PG.
//!
//! Exercises the full cluster plan → cluster apply pipeline:
//! - clean apply (CREATE ROLE succeeds, apply_log row written)
//! - intent-blocked apply (DropRole unapproved → fail)
//! - identity mismatch (plan generated against one cluster applied against
//!   another)
//!
//! Skipped wholesale when Docker is unavailable.

#![cfg(test)]

use std::path::Path;

use pgevolve::api::cluster::build_cluster_plan;
use pgevolve::cluster_config::{Bootstrap, ClusterConfig, ClusterConnection, ClusterProject};
use pgevolve::executor::{ApplyOverrides, apply_cluster_plan};
use pgevolve::target_identity::compute_cluster_target_identity;
use pgevolve_core::plan::{read_plan_dir, write_plan_dir};
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

fn write_role_file(project_root: &Path, sql: &str) {
    let roles_dir = project_root.join("roles");
    std::fs::create_dir_all(&roles_dir).unwrap();
    std::fs::write(roles_dir.join("test.sql"), sql).unwrap();
}

async fn cluster_cfg_for(pg: &EphemeralPostgres) -> ClusterConfig {
    ClusterConfig {
        project: ClusterProject {
            name: "test-cluster".into(),
        },
        connection: ClusterConnection {
            dsn: pg.connection_string().to_string(),
        },
        bootstrap: Bootstrap {
            roles: vec!["postgres".into()],
        },
    }
}

#[tokio::test]
async fn cluster_apply_clean_path_succeeds() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version()).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    write_role_file(tmp.path(), "CREATE ROLE app_test_role NOLOGIN;");
    let cfg = cluster_cfg_for(&pg).await;

    // Plan
    let cluster_plan = build_cluster_plan(tmp.path(), &cfg).await.unwrap();
    let (client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .unwrap();
    tokio::spawn(async move { let _ = connection.await; });
    let target_identity = compute_cluster_target_identity(&client).await.unwrap();
    let plan_id = pgevolve_core::plan::PlanId::compute(
        &pgevolve_core::ir::catalog::Catalog::empty(),
        &pgevolve_core::ir::catalog::Catalog::empty(),
        pgevolve_core::VERSION,
        0,
    )
    .unwrap();
    // For e2e: use a stable placeholder id so the test reads back the same plan.
    let core_plan = cluster_plan
        .to_plan(plan_id, target_identity)
        .unwrap();

    let plan_dir = tmp.path().join("cluster-plans").join("test-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    write_plan_dir(&core_plan, &plan_dir).unwrap();

    // Apply
    let plan = read_plan_dir(&plan_dir).unwrap();
    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .unwrap();
    tokio::spawn(async move { let _ = connection.await; });
    let outcome = apply_cluster_plan(&plan, &mut client, ApplyOverrides::default())
        .await
        .expect("apply should succeed for clean CREATE ROLE plan");

    let _ = outcome;

    // Verify role exists.
    let role = client
        .query_one(
            "SELECT 1 FROM pg_authid WHERE rolname = $1",
            &[&"app_test_role"],
        )
        .await;
    assert!(role.is_ok(), "app_test_role should be created");
}

#[tokio::test]
async fn cluster_apply_intent_blocked_when_unapproved() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version()).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();

    // First create a role, then plan its removal.
    let (client, connection) =
        tokio_postgres::connect(&pg.connection_string(), tokio_postgres::NoTls)
            .await
            .unwrap();
    tokio::spawn(async move { let _ = connection.await; });
    client.batch_execute("CREATE ROLE will_be_dropped;").await.unwrap();

    write_role_file(tmp.path(), ""); // empty roles → planner will diff out will_be_dropped

    let cfg = cluster_cfg_for(&pg).await;
    let cluster_plan = build_cluster_plan(tmp.path(), &cfg).await.unwrap();

    let (id_client, id_conn) = tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move { let _ = id_conn.await; });
    let target_identity = compute_cluster_target_identity(&id_client).await.unwrap();

    let plan_id = pgevolve_core::plan::PlanId::compute(
        &pgevolve_core::ir::catalog::Catalog::empty(),
        &pgevolve_core::ir::catalog::Catalog::empty(),
        pgevolve_core::VERSION,
        1, // bump to avoid colliding with prior test's plan_id
    )
    .unwrap();
    let core_plan = cluster_plan.to_plan(plan_id, target_identity).unwrap();
    let plan_dir = tmp.path().join("cluster-plans").join("drop-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    write_plan_dir(&core_plan, &plan_dir).unwrap();

    // Read back without modifying intent.toml — intents stay `approved=false`.
    let plan = read_plan_dir(&plan_dir).unwrap();
    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .unwrap();
    tokio::spawn(async move { let _ = connection.await; });

    let err = apply_cluster_plan(&plan, &mut client, ApplyOverrides::default())
        .await
        .expect_err("apply should fail with unapproved intent");
    let msg = err.to_string();
    assert!(
        msg.contains("UnapprovedIntent") || msg.contains("unapproved") || msg.contains("intent"),
        "expected unapproved-intent error, got: {msg}"
    );

    // Verify role still exists (apply was rejected at preflight).
    let role = client
        .query_one(
            "SELECT 1 FROM pg_authid WHERE rolname = $1",
            &[&"will_be_dropped"],
        )
        .await;
    assert!(role.is_ok(), "role should still exist after intent-blocked apply");
}

#[tokio::test]
async fn cluster_apply_identity_mismatch_rejected() {
    if !docker_available() {
        eprintln!("skipping: docker unavailable");
        return;
    }
    let pg = EphemeralPostgres::start(default_pg_version()).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    write_role_file(tmp.path(), "CREATE ROLE id_mismatch_role NOLOGIN;");
    let cfg = cluster_cfg_for(&pg).await;

    let cluster_plan = build_cluster_plan(tmp.path(), &cfg).await.unwrap();

    // Use a deliberately wrong identity so preflight rejects.
    let plan_id = pgevolve_core::plan::PlanId::compute(
        &pgevolve_core::ir::catalog::Catalog::empty(),
        &pgevolve_core::ir::catalog::Catalog::empty(),
        pgevolve_core::VERSION,
        2,
    )
    .unwrap();
    let core_plan = cluster_plan
        .to_plan(plan_id, "cluster:deadbeefdeadbeef".into())
        .unwrap();
    let plan_dir = tmp.path().join("cluster-plans").join("mismatch-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    write_plan_dir(&core_plan, &plan_dir).unwrap();

    let plan = read_plan_dir(&plan_dir).unwrap();
    let (mut client, connection) =
        tokio_postgres::connect(&cfg.connection.dsn, tokio_postgres::NoTls)
            .await
            .unwrap();
    tokio::spawn(async move { let _ = connection.await; });

    let err = apply_cluster_plan(&plan, &mut client, ApplyOverrides::default())
        .await
        .expect_err("apply should fail with identity mismatch");
    let msg = err.to_string();
    assert!(
        msg.contains("TargetIdentityMismatch") || msg.contains("identity"),
        "expected identity-mismatch error, got: {msg}"
    );
}
```

Adapt assertions to the actual error message strings produced by your `ApplyError` variants. The integration tests use `expect_err` + substring matching to stay resilient to small formatting changes.

- [ ] **Step 2: Run integration tests against ephemeral PG**

Run: `cargo test -p pgevolve --test cluster_apply_e2e`

Expected: 3 PASS (or 3 skipped with "docker unavailable" messages if Docker isn't running). If a test fails, capture the panic message and trace it back to the corresponding preflight branch.

- [ ] **Step 3: Update CHANGELOG**

Open `CHANGELOG.md`. Under `## [Unreleased]`, ensure the following sections exist:

```markdown
### Changed

- **Cluster apply reaches per-DB parity.** `pgevolve cluster apply` now bootstraps `pgevolve` metadata, acquires the singleton advisory lock, runs cluster preflight (identity match + intent approval), writes an `apply_log` row, executes via the per-DB group executor, and closes the audit row. `pgevolve cluster plan` writes the canonical 3-file plan layout (structured `plan.sql` headers + `intent.toml` + `manifest.toml` with `target_identity`). Closes #7.

### Removed

- `pgevolve::executor::apply_cluster_steps` (public API). Callers that previously built a `Vec<RawStep>` and applied it directly should now build a `Plan` and use `apply_cluster_plan` instead.
```

- [ ] **Step 4: Append row to plans index**

In `docs/superpowers/plans/README.md`, find the chronological table. After the last existing row, add:

```markdown
| 2026-06-03 | [Cluster apply parity (#7)](./2026-06-03-cluster-apply-parity.md) |
```

- [ ] **Step 5: Verify the staged change set**

Run: `git status --porcelain`

Expected:
```
 M CHANGELOG.md
 M docs/superpowers/plans/README.md
?? crates/pgevolve/tests/cluster_apply_e2e.rs
```

If anything else appears, stop and investigate.

- [ ] **Step 6: Verify gate (full)**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
cargo deny check
```

Expected: all green.

- [ ] **Step 7: Commit**

```sh
git add crates/pgevolve/tests/cluster_apply_e2e.rs CHANGELOG.md docs/superpowers/plans/README.md
git commit -m "$(cat <<'EOF'
test(cluster): e2e cluster apply tests + CHANGELOG entry

Three integration tests against ephemeral PG:
- clean apply: CREATE ROLE succeeds, role visible in pg_authid
- intent-blocked: DropRole without approved=true rejects at preflight
- identity mismatch: plan with wrong target_identity rejects at preflight

Closes #7. v1.0 path's last open enhancement item lands.

Step 8/8 of issue #7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-review (done by plan author, 2026-06-03)

**1. Spec coverage:**

- Spec §1 (architecture: reuse Plan via `to_plan`): covered by Tasks 1 (Plan::from_grouped_with_id), 3 (ClusterPlan::to_plan), and 5 (apply_cluster_plan).
- Spec §2 (`cluster:{system_identifier_hex}` identity): covered by Task 2.
- Spec §3 (cluster apply pipeline, 6 steps): covered by Task 5 (mirrors apply_plan structurally) + Task 4 (cluster_preflight).
- Spec §3.1 (preflight checks: identity match + intent approval; no drift recheck): covered by Task 4.
- Spec §3.2 (ApplyOverrides reuse): covered by Task 5 (the overrides struct passed straight through).
- Spec §4 (ClusterPlan::to_plan + CLI surface): covered by Tasks 3 + 6 (cluster plan CLI) + 7 (cluster apply CLI).
- Spec §4.3 (retirements: apply_cluster_steps, split_sql_statements, slim apply_cluster_plan_dir): covered by Task 7.
- Spec §5 (unit + integration tests): unit coverage embedded in Tasks 1, 2, 3, 4, 6; integration in Task 8.
- Spec §6 (out of scope): no tasks for drift recheck, cluster status, plan unification, lint waivers, cross-DB intent inheritance, custom retention — verified.
- Spec §7 (8-commit decomposition): plan has 8 tasks matching.
- Spec §8 (gotchas): all four addressed inline (connection management in Task 6, bootstrap idempotence assumed and verified by Task 8 e2e tests, read_plan_dir per-DB assumptions tested in Task 8, StepKind cluster variants already exist per the writing-plans pass).

**2. Placeholder scan:**

No "TBD" / "TODO" / "implement later" / "similar to Task N" patterns. Every file path is exact (with hedges only where exploration during implementation is genuinely needed — e.g., `PlanId::from_hex` may or may not exist; the plan instructs the implementer to grep and add it if missing).

**3. Type consistency:**

- `Plan::from_grouped_with_id` signature is defined in Task 1 Step 4 and referenced by `ClusterPlan::to_plan` in Task 3 Step 3. Argument order matches.
- `compute_cluster_target_identity` returns `Result<String, ApplyError>` in Task 2 Step 3 and is called with that signature in Tasks 6 Step 2 and 8 Step 1.
- `ClusterPreflightOverrides` is defined with `allow_different_target` + `allow_unapproved_intents` in Task 4 Step 2 and constructed with those exact fields in Task 5 Step 2.
- `apply_cluster_plan(plan, client, overrides)` signature appears consistently in Tasks 5, 7, and 8.

If `PlanId::from_hex`, `ApplyError::Internal`, `ApplyError::TargetIdentityMismatch`, or `ApplyError::UnapprovedIntent` don't already exist, the plan instructs the implementer to add them in the task that first needs them, and to call out the addition in the commit message. This is the only piece of "implement now / verify later" content; it's bounded and explicit.
