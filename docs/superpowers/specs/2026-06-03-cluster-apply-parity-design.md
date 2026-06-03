---
status: design
target: v1.0
issue: 7
---

# Cluster apply parity — design

Closes [#7](https://github.com/saosebastiao/pgevolve/issues/7). Brings
`apply_cluster_plan_dir` from its current "split on semicolons, run each
in its own transaction" state to full parity with the per-DB
`apply_plan` flow: structured plan.sql header parsing, advisory lock,
intent enforcement, manifest cross-check, and `apply_log` audit.

This is a real feature, not a polish pass. The cluster planner side
changes too — `ClusterPlan` learns to materialize a `pgevolve_core::plan::Plan`,
the `cluster plan` CLI command writes the same three-file plan
directory layout per-DB plans use, and the cluster `apply` reads
through the same plan loader.

---

## §1. Architecture

`ClusterPlan` grows a `to_plan(target_identity) -> Result<Plan, …>`
method. The result is the same `pgevolve_core::plan::Plan` struct
per-DB uses, threaded through the existing
`write_plan_dir` / `read_plan_dir` machinery.

**Why reuse `Plan`** instead of building a parallel
`ClusterPlanSerializer`:

- The structured `-- @pgevolve step` headers in `plan.sql` are
  RawStep-agnostic; cluster steps slot in unchanged.
- The `intent.toml` row format (`step_no`, `kind`, `targets`,
  `approved`) describes any destructive operation; cluster's
  `DropRole` fits the same shape as per-DB's `DropTable`.
- The `manifest.toml` schema (plan_id, target_identity, planner
  version, step count, etc.) is data, not type-bound.
- Cluster apply gets all the per-DB safety machinery for free.

Differences between cluster and per-DB are pushed to:

- **`target_identity` composition** — `cluster:{system_identifier}`
  for cluster plans vs `hash(host,port,dbname,system_identifier)`
  for per-DB plans (see §2).
- **Preflight** — a new `executor::cluster_preflight` module mirrors
  `executor::preflight` but checks cluster identity (`pg_control_system().system_identifier`)
  instead of per-DB identity (see §3).

Other layers (bootstrap, lock, audit, execute) are reused as-is.

---

## §2. Cluster `target_identity`

| Plan kind | Identity composition |
|---|---|
| Per-DB | `hash(host, port, dbname, system_identifier)` — opaque string |
| **Cluster (new)** | `cluster:{system_identifier_lowercase_hex}` |

**Rationale for `system_identifier` only:**

- `system_identifier` is unique per `initdb`; it's the canonical
  cluster fingerprint. A failover replica retains the same identifier.
- Including `host` / `port` would block legitimate re-targeting via
  load balancers, DNS swaps, or applying from a different bastion than
  where planning happened.
- `dbname` is irrelevant for cluster ops — they target the cluster,
  not a particular database.

**Rationale for the `cluster:` prefix:**

- Distinguishes cluster plan_id rows from per-DB rows at a glance in
  `apply_log` queries.
- Prevents accidental cross-matching with per-DB identity strings
  that happen to be the same length.

**How the executor computes it:**

```sql
SELECT system_identifier FROM pg_control_system();
```

The value is a `uint64` returned by libpq as a textual integer; we
hex-encode it lowercase and prefix with `cluster:`. No other PG
queries needed.

---

## §3. Cluster apply pipeline

`apply_cluster_plan(plan, client, cfg, overrides)` mirrors
`apply_plan` step-by-step:

1. **Bootstrap** — `bootstrap_metadata(client)` creates the
   `pgevolve` schema in whatever DB the cluster DSN connects to
   (typically `postgres`).
2. **Acquire singleton advisory lock** — `try_acquire_lock` with
   `PGEVOLVE_LOCK_KEY`. PG advisory locks are cluster-scoped at the
   lock-manager level, so cluster apply correctly serializes against
   any per-DB apply running anywhere in the same cluster.
3. **Cluster preflight** — see §3.1.
4. **Open `apply_log` row** — `open_apply_log` writes the row in the
   maintenance DB's `pgevolve.apply_log` table. `plan_id` is the
   cluster plan_id so cluster runs are filterable.
5. **Execute groups** — reuse `execute::execute_plan`. Cluster
   plans use `RawStep::transactional == InTransaction` for every step
   today (per `cluster_apply::execute_step`'s current code path); the
   group executor handles both transactional and autocommit groups so
   future CREATE INDEX-style cluster ops fit naturally.
6. **Close `apply_log` row** — `close_apply_log` with `succeeded`
   / `failed` / `aborted`.

### §3.1. Cluster preflight checks

New module `executor/cluster_preflight.rs`:

- **Identity match.** Compute live cluster identity, compare to
  `plan.metadata.target_identity`. Mismatch returns
  `ApplyError::TargetIdentityMismatch`. Bypassed by
  `overrides.allow_different_target = true`.
- **Manifest cross-check.** `read_plan_dir` already validates that
  `plan.sql`, `intent.toml`, and `manifest.toml` carry the same
  `plan_id`. No extra check needed at apply time — if the plan
  loaded, the manifest matches.
- **Intent approval.** For every step whose RawStep `destructive`
  flag is set (today: cluster steps for `DropRole`), look up the
  corresponding `intent.toml` row. If `approved == false`, return
  `ApplyError::UnapprovedIntent`. Bypassed by
  `overrides.allow_unapproved_intents = true` (test harness only).
- **No drift recheck.** v1.0 ships without this; cluster catalog
  re-read at apply time doubles connection cost, and cluster-level
  drift is rare in practice (humans don't `CREATE ROLE` mid-deploy
  the way they `CREATE TABLE`). File a follow-up enhancement if
  drift becomes a real concern.

The preflight returns `Result<(), ApplyError>` matching per-DB
preflight. On failure the lock is released before the error
propagates, identical to `apply_plan` failure handling.

### §3.2. What goes in `ApplyOverrides`

Reuse the existing per-DB `ApplyOverrides` struct as-is. The cluster
preflight ignores `allow_drift` (no drift recheck), `allow_unwaived_lint`
(cluster lints stay advisory-only — see Out of Scope), and
`abort_after_step` (kept for test harness symmetry; cluster execute
respects it identically to per-DB).

---

## §4. Cluster plan emission

`ClusterPlan::to_plan(target_identity: String) -> Result<Plan, PlanError>`:

- Regroup `self.steps` into transactional groups via the existing
  `group_steps`. The grouper is kind-agnostic — it consumes
  `Vec<RawStep>` and produces the structure `Plan::from_grouped`
  expects.
- Extract destructive intents from `self.changes` — currently only
  `ClusterChange::DropRole` is destructive. Build the
  `Vec<DestructiveIntent>` the `Plan` constructor wants.
- Call `Plan::from_grouped(groups, source, target, target_identity,
  None, VERSION, planner_ruleset_version)`. The `None` is for the
  optional plan-id seed (existing default).

**`build_cluster_plan` is unchanged.** It still returns the rich
`ClusterPlan` (with `advisory_findings`, raw changeset, ClusterCatalog
intermediates) for callers that want lint output and human-readable
diff inspection. `to_plan` is a separate finalize step for callers
that want serialization.

### §4.1. CLI surface — `cluster plan`

`crates/pgevolve/src/commands/cluster/plan.rs` (file may need to be
created; current code only has `commands/cluster/apply.rs`. Will
confirm exact path during implementation):

1. Connect to the cluster DSN.
2. Read live `system_identifier` via `pg_control_system()` →
   construct `target_identity`.
3. Call `build_cluster_plan(project_root, cfg).await` →
   `ClusterPlan`.
4. Surface `clusterplan.advisory_findings` to stderr (existing
   convention).
5. Call `clusterplan.to_plan(target_identity)?` → `Plan`.
6. Call `write_plan_dir(&plan, &plan_dir)?` — writes plan.sql +
   intent.toml + manifest.toml.

### §4.2. CLI surface — `cluster apply`

`crates/pgevolve/src/commands/cluster/apply.rs` changes:

- Drop the bare `apply_cluster_plan_dir(plan_dir, cfg)` call.
- Use `read_plan_dir(plan_dir)?` → `Plan`.
- Open a connection to `cfg.connection.dsn`.
- Call `apply_cluster_plan(&plan, &mut client, cfg, overrides).await`.
- Surface the `ApplyOutcome` to stdout / stderr identically to
  per-DB.

Add the same `ApplyOverrides` flags the per-DB CLI exposes:
`--allow-different-target`, `--allow-unapproved-intents` (gated
internally; not a user-facing flag), `--actor`.

### §4.3. What retires

- **`apply_cluster_steps(steps, cfg)`** public API — callers that
  have a `Vec<RawStep>` should build a `Plan` and use
  `apply_cluster_plan`. Two apply paths in parallel was acceptable at
  v0.3.0 prototype scope; not at v1.0.
- **`split_sql_statements`** and the naive `run_in_transaction` loop
  in `cluster_apply.rs` — replaced by `execute::execute_plan` reuse.
- **`apply_cluster_plan_dir`** stays as a thin wrapper:
  `read_plan_dir` + open connection + `apply_cluster_plan`. The
  current ~140-line body collapses to ~20 lines.

---

## §5. Tests

### §5.1. Unit tests

- `ClusterPlan::to_plan` happy path — given a synthetic `ClusterPlan`
  with one CREATE ROLE + one DROP ROLE step, the resulting `Plan` has
  the right groups, destructive_intents, and target_identity.
- `ClusterPlan::to_plan` empty case — empty `steps` produces an
  empty `Plan` (no groups), no panic.
- Cluster preflight identity match — fail when live identity doesn't
  match plan; succeed when it does; bypass when
  `allow_different_target = true`.
- Cluster preflight intent approval — fail when a DropRole step has
  `approved = false` in intent.toml; succeed when approved; bypass
  when `allow_unapproved_intents = true`.

### §5.2. Integration tests

Against an ephemeral PG. Tests live in
`crates/pgevolve/tests/cluster_apply_e2e.rs` (new file; existing
`tests/cluster_api.rs` covers the `build_cluster_plan` library entry
point but not the executor).

- **Clean apply** — `cluster plan` → edit intent.toml to approve →
  `cluster apply` → CREATE ROLE succeeded in pg_authid, apply_log row
  written with status `succeeded`.
- **Intent-blocked apply** — `cluster plan` produces a DropRole →
  apply without editing intent.toml → fail with `UnapprovedIntent`
  error, apply_log row written with status `failed` (or no row if
  preflight failed before opening the log; match per-DB behavior).
- **Identity mismatch** — generate plan against cluster A, apply
  against cluster B (different `system_identifier`). Fail with
  `TargetIdentityMismatch`.
- **Lock contention** — open one connection that holds the apply
  lock manually; second concurrent `cluster apply` fails with
  `LockHeld` error.

### §5.3. Conformance

The conformance suite at
`crates/pgevolve-conformance/tests/cases/cluster/` (if it exists; will
confirm) should grow at least one fixture exercising the new
end-to-end flow. If no cluster conformance tier exists yet, that's a
follow-up.

---

## §6. Out of scope

Deferred to separate enhancements after #7 ships:

- **Drift recheck at apply time.** §3.1 rationale.
- **`pgevolve cluster status`** command — per-DB has `status`;
  cluster doesn't. Useful but separate.
- **Unifying per-DB and cluster `Plan` structures.** Discussed as
  scope option C during brainstorming; rejected as premature.
- **Lint waiver mechanism for cluster.** Cluster lint findings stay
  advisory-only. If cluster lints become blocking in a future
  release, port the per-DB `[[lint_waiver]]` mechanism then.
- **Cross-DB intent inheritance** — e.g., a per-DB plan that
  references a role declared by a cluster plan should validate that
  the role exists. Today the two plans are independent. Worth
  revisiting post-v1.0.
- **Custom `apply_log` retention or rotation.** Existing per-DB
  behavior applies as-is to cluster rows.

---

## §7. What this design produces

A focused multi-commit feature, sized for a writing-plans pass. Rough
commit decomposition (writing-plans will firm this up):

1. `ClusterPlan::to_plan` + unit tests.
2. `executor::cluster_preflight` module + unit tests.
3. `executor::apply_cluster_plan` + integration tests against
   ephemeral PG.
4. CLI: rewrite `cluster plan` to call `write_plan_dir`.
5. CLI: rewrite `cluster apply` to call `read_plan_dir` +
   `apply_cluster_plan`; surface `ApplyOverrides` flags.
6. Retire `apply_cluster_steps` + `split_sql_statements` +
   collapse `apply_cluster_plan_dir` to a thin wrapper.
7. Update `docs/spec/cluster.md` (or equivalent — confirm during
   implementation) to describe the new 3-file plan layout and apply
   flow.
8. CHANGELOG entry: `[Unreleased]` → `### Changed: cluster apply
   reaches per-DB parity (intent enforcement, advisory lock,
   apply_log, manifest cross-check).` and a `### Removed` line for
   the retired `apply_cluster_steps` public API.

Estimated effort: 1-2 days of focused work. Each commit lands with
its tests green; the verify gate runs after every commit; CI gates on
push.

---

## §8. Implementation gotchas to surface during writing-plans

(Notes for the planning pass — don't relitigate during brainstorming.)

- The current cluster apply uses `tokio_postgres::connect` directly
  (no connection pool, no PgCatalogQuerier wrapping). Per-DB
  `apply_plan` takes a `&mut Client`. The cluster CLI command will
  need to open its own client; reuse the existing pattern from
  `commands/cluster/apply.rs:20-30`.
- `bootstrap_metadata` should be a no-op when the schema already
  exists; check that it's idempotent enough to survive cluster apply
  running against a DB that already has per-DB rows.
- `read_plan_dir` validates plan_id consistency across the 3 files.
  Confirm during implementation that it doesn't impose any per-DB-specific
  schema assumptions (e.g., requiring at least one schema in the
  diff); cluster plans only carry cluster-level steps.
- `Plan::from_grouped` may have constraints on `source`/`target`
  catalog non-emptiness. Read its validation; if it requires non-empty
  per-DB catalogs, we need either to allow empty or to attach the
  ClusterCatalog as a stand-in. This is the most likely "surprise"
  during implementation.
- The cluster `RawStep`s carry `kind` values from `StepKind` — if
  there's no `CreateRole` / `DropRole` variant on `StepKind` today
  (cluster uses generic `kind: ALTER_SQL` or similar), the structured
  header serialization may need new variants. Confirm during
  exploration.
