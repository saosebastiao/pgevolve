# Executor

The apply loop. Source lives in `crates/pgevolve/src/executor/`.

## Entry point

`pgevolve::executor::apply(plan_dir, client, filter, overrides)`:

```rust
pub async fn apply(
    plan_dir: &Path,
    client: &mut tokio_postgres::Client,
    filter: &CatalogFilter,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError>;
```

The flow, mapped to spec §8:

1. `read_plan_dir(plan_dir)` — load the three files; cross-check plan id.
2. `bootstrap_metadata(client)` — install or upgrade the `pgevolve.*`
   tables. Idempotent.
3. `try_acquire_lock(client, &actor)` — `pg_try_advisory_lock` on the
   singleton key.
4. `run_preflight(client, &plan, filter, preflight_overrides)` —
   target-identity match, drift recheck, intent approval.
5. `open_apply_log(client, &plan, &actor)` — `apply_log` row +
   `plan_steps` rows.
6. `execute_plan(client, &plan, apply_id, abort_after_step)` — execute
   each group in order.
7. `close_apply_log(client, apply_id, status, error_message)`.
8. `release_lock(client)` — clear the lock row + `pg_advisory_unlock`.

On failure: the `apply_log` row gets status `failed` / `aborted`, the
lock is released, and the error bubbles up.

## The `pgevolve` metadata schema

Installed by `bootstrap_metadata` (`executor::bootstrap`). All tables
live in the `pgevolve` schema.

| Table | Role |
|---|---|
| `pgevolve.bootstrap_version` | Append-only list of applied schema migrations. Bootstrap is idempotent because it consults this table before running any DDL. |
| `pgevolve.apply_log` | One row per apply attempt. Status, plan id + hash, version metadata, actor, started/finished timestamps, error message. |
| `pgevolve.plan_steps` | One row per step within an apply. Status, SQL text, targets, error message. |
| `pgevolve.lock` | Singleton row tracking who holds the advisory lock (informational; the actual lock is the session-scoped advisory lock). |

The metadata schema is **append-only at the migration level**.
`bootstrap_metadata` looks at `bootstrap_version`, runs migrations
whose version exceeds the current, and inserts a new
`bootstrap_version` row. Schema changes to the `pgevolve` schema itself
ship as new entries in `BOOTSTRAP_MIGRATIONS`.

## Singleton advisory lock

The lock key is derived from the ASCII bytes `b"PGEVOLVE"`:

```rust
pub const PGEVOLVE_LOCK_KEY: i64 = i64::from_be_bytes(*b"PGEVOLVE");
```

`try_acquire_lock` calls `pg_try_advisory_lock(PGEVOLVE_LOCK_KEY)` and
updates the `pgevolve.lock` row on success. The lock is **session-scoped
** — Postgres releases it automatically when the session disconnects,
which means a crashed apply releases the lock without any cleanup
action. The `pgevolve.lock` row is purely informational: the next
acquirer's UPDATE overwrites it.

`release_lock` clears the lock row and calls `pg_advisory_unlock` for
clean shutdowns.

## Target-identity check

`compute_target_identity(client)`:

```rust
let row = client.query_one(
    "SELECT current_database(),
            inet_server_addr()::text,
            inet_server_port(),
            current_setting('cluster_name', true),
            (SELECT system_identifier::text FROM pg_control_system())",
    &[],
).await?;
```

The five fields are BLAKE3-hashed with NUL separators and a domain
prefix; the first 16 hex characters are the target identity. Stable
across reconnects to the same DB; different across different DBs.

The preflight check fails with `ApplyError::TargetIdentityMismatch`
when the live identity doesn't match `plan.metadata.target_identity`
unless `overrides.allow_different_target` is set.

## Preflight

`run_preflight(client, &plan, filter, overrides)` runs three checks:

1. **Target-identity match.** Always enforced unless `allow_different_target`.
2. **Drift recheck.** Re-introspect the live catalog and diff against
   `plan.metadata.target_snapshot`. Fails with
   `ApplyError::DriftDetected(n)` if any drift is found, unless
   `allow_drift` is set.
3. **Intent enforcement.** Iterate `plan.intents`, refuse to run any
   destructive step whose intent isn't approved.

> **v0.1 status.** The drift recheck is stubbed
> (`read_live_catalog` returns `Catalog::empty()`). The CLI's `apply`
> command forces `allow_drift = true` internally to compensate. The
> intent recheck is also currently a TODO — `Plan::read_from_dir`
> reads `intent.toml`'s approval state but the executor doesn't
> consult it. Both lands in v0.1.x once the binary-side catalog
> reader is threaded into preflight.

## Execution

`execute_plan(client, plan, apply_id, abort_after_step)`:

```rust
for group in &plan.groups {
    if group.transactional {
        execute_transactional_group(client, apply_id, group, abort_after_step).await?;
    } else {
        execute_autocommit_group(client, apply_id, group, abort_after_step).await?;
    }
}
```

### Transactional groups

```rust
let tx = client.transaction().await?;
for step in &group.steps {
    mark_step_running(tx.client(), apply_id, step.step_no).await?;
    if let Err(e) = tx.batch_execute(&step.sql).await {
        let err_msg = render_pg_error(&e);
        tx.rollback().await?;
        // After rollback: write final audit rows on the bare client.
        mark_step_failed(client, apply_id, step.step_no, &err_msg).await?;
        mark_steps_rolled_back(client, apply_id, group.id).await?;
        return Err(ApplyError::StepFailed { … });
    }
    mark_step_succeeded(tx.client(), apply_id, step.step_no).await?;
    if abort_after_step == Some(step.step_no) {
        // Break out of the loop AFTER the success mark.
        return Err(ApplyError::AbortedAfterStep { step_no: step.step_no });
    }
}
tx.commit().await?;
```

Key subtleties:

- **Audit updates ride inside the transaction.** `mark_step_running`
  and `mark_step_succeeded` are run on `tx.client()`, so they're part
  of the same transaction as the DDL. If the transaction rolls back,
  the audit updates also revert.
- **After rollback, audit is re-marked on the bare client.** The
  failing step gets `mark_step_failed`; every other step in the
  group gets `mark_steps_rolled_back` (which updates `pending` *and*
  `running` *and* `succeeded` rows in the group — because the
  pre-rollback `succeeded` state was reverted by the rollback).
- **`render_pg_error` extracts SQLSTATE + server message** from the
  `tokio_postgres::Error`. Without it, the error display is just
  "db error" — useless for debugging.

### Autocommit groups

Each step runs on the bare client (no transaction). On failure: mark
the failing step `failed`, return `StepFailed`. Earlier steps stay
`succeeded`; later steps stay `pending`.

This is the right semantics for `CONCURRENTLY` groups: each
`CONCURRENTLY` operation is its own atomic Postgres operation, and a
failure mid-group doesn't roll back the predecessors.

## The `abort_after_step` testkit hook

`ApplyOverrides::abort_after_step: Option<u32>` is the **chaos hook**.
When set to `Some(n)`, the executor cleanly aborts after the step
whose `step_no == n` succeeds. The error is `ApplyError::AbortedAfterStep`,
and `close_apply_log` sets the row to `aborted` rather than `failed`.

Used by the testkit's chaos harness to validate recovery semantics
without the full ceremony of SIGKILL. The recovery property the
harness tests:

1. `apply(target, source, abort_after_step=N)` — runs through step N,
   then aborts.
2. Introspect the live database → partial state.
3. `apply(partial, source)` — re-plans from the partial state and runs
   to completion.
4. Live state == source.

## `ApplyError` taxonomy

```rust
pub enum ApplyError {
    Postgres(tokio_postgres::Error),
    PlanIo(PlanIoError),
    Catalog(CatalogError),
    LockHeld,
    TargetIdentityMismatch { plan: String, live: String },
    DriftDetected(usize),
    UnapprovedIntents { count: usize, details: Vec<(u32, String, String)> },
    StepFailed { step_no: u32, group_no: u32, error: String },
    AbortedAfterStep { step_no: u32 },
}
```

The CLI's `commands/apply.rs` maps these to exit codes:

| `ApplyError` variant | Exit code |
|---|---|
| `TargetIdentityMismatch` / `DriftDetected` / `UnapprovedIntents` | 2 (preflight) |
| `LockHeld` / `StepFailed` | 3 (apply error) |
| Anything else | 1 |
| `AbortedAfterStep` | (testkit-only; should not reach the CLI) |

## Audit rows in detail

### `apply_log` lifecycle

```sql
INSERT INTO pgevolve.apply_log (..., status) VALUES (..., 'running');
-- … execution …
UPDATE pgevolve.apply_log SET status = 'succeeded',           finished_at = now() WHERE …;
                                         'failed',
                                         'aborted',
```

The CHECK constraint enforces the four valid statuses.

### `plan_steps` lifecycle

`open_apply_log` pre-populates every step row as `pending`. Per-step
transitions:

```
pending  ──mark_step_running──►  running  ──mark_step_succeeded──►  succeeded
                                    │
                                    └──mark_step_failed──►  failed
                                    │
                                    └──mark_steps_rolled_back──►  rolled_back
```

`mark_steps_rolled_back` is the cleanup after a transactional rollback:
it flips every step in the group whose status is `pending` /
`running` / `succeeded` to `rolled_back`. The "include pending" is the
subtle bit — when audit rows are part of the rolled-back transaction,
their `succeeded` update vanishes and they revert to `pending`.

## Recovery from partial apply

When an apply fails or aborts mid-flight:

1. The `apply_log` row stays around with status `failed` / `aborted`
   and an `error_message`.
2. `pgevolve.plan_steps` records which steps succeeded, which failed,
   which rolled back.
3. **The next `pgevolve plan` re-reads the live catalog**, which now
   reflects whatever DDL committed. The new plan diffs from that state.
4. **No special recovery command is needed.** Re-planning produces a
   plan that picks up where the previous one stopped.

This is what `drift_recovery_property` tests: random catalog, abort
after random step, re-plan, re-apply, assert live state matches the
target.

## Concurrency semantics

- Two `pgevolve apply` invocations on the same database serialize via
  the advisory lock. The second one fails with `ApplyError::LockHeld`.
- An apply and unrelated user DDL (e.g., someone running `psql` at
  the same time) do *not* serialize. pgevolve's lock is namespaced; it
  doesn't take a relation lock. Concurrent DDL from outside pgevolve
  may cause the apply to see drift, which the preflight check would
  catch (once it's wired up; see the v0.1 status note above).
- Concurrent `pgevolve plan` runs against the same database are safe:
  `plan` is read-only.

## Why session-scoped advisory locks?

A transaction-scoped advisory lock would release at every
`COMMIT`. pgevolve's apply spans multiple transactional groups plus
non-transactional `CONCURRENTLY` groups; a transaction-scoped lock
would release between them and let a second apply in.

Session-scoped advisory locks hold across the entire session, which is
exactly the right scope. The single client connection's session is
the apply's lifespan; disconnecting (or finishing cleanly) releases.
