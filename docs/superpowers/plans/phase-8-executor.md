# Phase 8 — Metadata schema bootstrap and executor

**Goal:** Ship the runtime that takes a serialized plan and applies it to a live Postgres database, with the `pgevolve` metadata schema, advisory-lock concurrency control, target-identity verification, drift recheck, intent enforcement, and per-step audit logging.

**Spec coverage:** §8, §9.

**Depends on:** Phases 3 (catalog reader), 7 (plan format).

**Exit criteria:**

- `pgevolve::executor::bootstrap_metadata(client) -> Result<...>` idempotently installs/upgrades the `pgevolve` schema (3 tables: `apply_log`, `plan_steps`, `lock`).
- `pgevolve::executor::apply(plan_dir, db_options) -> Result<ApplyOutcome, ApplyError>` runs the documented apply flow.
- Pre-flight checks (lock acquisition, identity match, drift recheck, intent enforcement) all reject with appropriate exit codes.
- Per-step audit rows in `pgevolve.plan_steps` show every step with its final status.
- Tier-4 end-to-end migration fixture harness in `pgevolve-testkit` runs at least one fixture green.

---

## File structure

```
crates/pgevolve/src/
├── connection.rs              # DSN resolution, tokio_postgres::Client management, runtime sharing
├── querier_pg.rs              # PgCatalogQuerier (already from phase 3)
├── target_identity.rs         # hash (host, port, dbname, system_identifier)
└── executor/
    ├── mod.rs                 # apply() entry point + ApplyOutcome
    ├── error.rs               # ApplyError
    ├── bootstrap.rs           # pgevolve schema install/upgrade
    ├── lock.rs                # advisory lock acquisition + lock-row write
    ├── identity.rs            # call into target_identity.rs
    ├── preflight.rs           # drift recheck + intent enforcement
    ├── audit.rs               # apply_log + plan_steps writers
    ├── status.rs              # status command queries
    └── execute.rs             # group/step execution loop

crates/pgevolve-testkit/src/
├── apply_harness.rs           # ApplyHarness for tier-4
└── migration_fixture.rs       # MigrationFixture loader

crates/pgevolve-core/tests/fixtures/e2e/
└── 0001-create-and-alter-table/
    ├── initial_source/        # contents of source-of-truth dir at step 1
    ├── final_source/          # contents at step 2
    ├── expected_changes.yaml
    └── data_assertions.sql    # optional
```

---

### Task 8.1: `bootstrap_metadata` — install or upgrade the `pgevolve` schema

**File:** `crates/pgevolve/src/executor/bootstrap.rs`

```rust
pub async fn bootstrap_metadata(client: &tokio_postgres::Client) -> Result<(), ApplyError> {
    // Use a separate "version" table to track bootstrap migrations.
    client.batch_execute(SQL_CREATE_SCHEMA_AND_VERSION_TABLE).await?;

    let current: i32 = client.query_one(
        "SELECT COALESCE(max(version), 0) FROM pgevolve.bootstrap_version",
        &[]
    ).await?.get(0);

    for migration in BOOTSTRAP_MIGRATIONS.iter().filter(|m| m.version > current) {
        let tx = client.transaction().await?;
        tx.batch_execute(migration.sql).await?;
        tx.execute(
            "INSERT INTO pgevolve.bootstrap_version (version, applied_at) VALUES ($1, now())",
            &[&migration.version]
        ).await?;
        tx.commit().await?;
    }
    Ok(())
}

struct BootstrapMigration { version: i32, sql: &'static str }

const SQL_CREATE_SCHEMA_AND_VERSION_TABLE: &str = r#"
CREATE SCHEMA IF NOT EXISTS pgevolve;
CREATE TABLE IF NOT EXISTS pgevolve.bootstrap_version (
    version int PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);
"#;

const BOOTSTRAP_MIGRATIONS: &[BootstrapMigration] = &[
    BootstrapMigration { version: 1, sql: r#"
        CREATE TABLE pgevolve.apply_log (
          apply_id          uuid        PRIMARY KEY,
          plan_id           text        NOT NULL,
          plan_hash         text        NOT NULL,
          pgevolve_version  text        NOT NULL,
          source_rev        text,
          target_identity   text        NOT NULL,
          actor             text,
          started_at        timestamptz NOT NULL DEFAULT now(),
          finished_at       timestamptz,
          status            text        NOT NULL CHECK (status IN ('running','succeeded','failed','aborted')),
          error_message     text
        );
        CREATE INDEX apply_log_started_at_idx ON pgevolve.apply_log (started_at DESC);

        CREATE TABLE pgevolve.plan_steps (
          apply_id      uuid         NOT NULL REFERENCES pgevolve.apply_log(apply_id) ON DELETE CASCADE,
          step_no       int          NOT NULL,
          group_no      int          NOT NULL,
          kind          text         NOT NULL,
          destructive   boolean      NOT NULL,
          targets       text[]       NOT NULL,
          sql_text      text         NOT NULL,
          started_at    timestamptz,
          finished_at   timestamptz,
          status        text         NOT NULL CHECK (status IN ('pending','running','succeeded','failed','rolled_back','skipped')),
          error_message text,
          PRIMARY KEY (apply_id, step_no)
        );

        CREATE TABLE pgevolve.lock (
          singleton         boolean     PRIMARY KEY DEFAULT true CHECK (singleton),
          held_by           text,
          held_since        timestamptz,
          pgevolve_version  text
        );
        INSERT INTO pgevolve.lock (singleton) VALUES (true);
    "# },
];
```

Tests with `EphemeralPostgres`: bootstrap on a fresh DB → all four tables exist; bootstrap a second time → no error and no duplicate rows. Bootstrap migrations are append-only; future versions append `BootstrapMigration { version: 2, ... }`.

Commit: `feat(cli): bootstrap_metadata installs/upgrades pgevolve schema idempotently`

---

### Task 8.2: Advisory-lock acquisition

**File:** `crates/pgevolve/src/executor/lock.rs`

```rust
const PGEVOLVE_LOCK_KEY: i64 = 0x_50_47_45_56_4F_4C_56_45_i64;  // bytes "PGEVOLVE" packed

pub async fn try_acquire_lock(client: &tokio_postgres::Client, actor: &str) -> Result<LockGuard, ApplyError> {
    // pg_try_advisory_lock returns false if the lock is held by another session.
    let row = client.query_one("SELECT pg_try_advisory_lock($1)", &[&PGEVOLVE_LOCK_KEY]).await?;
    let acquired: bool = row.get(0);
    if !acquired {
        return Err(ApplyError::LockHeld);
    }
    client.execute(
        "UPDATE pgevolve.lock SET held_by=$1, held_since=now(), pgevolve_version=$2 WHERE singleton=true",
        &[&actor, &pgevolve_core::VERSION],
    ).await?;
    Ok(LockGuard { /* ... */ })
}

pub struct LockGuard { /* holds the client reference; releases on drop */ }
```

Drop impl releases via `pg_advisory_unlock` and clears the row.

Tests with `EphemeralPostgres`: two sessions try to lock simultaneously → only one wins.

Commit: `feat(cli): advisory-lock acquisition with audit row`

---

### Task 8.3: Target-identity hashing

**File:** `crates/pgevolve/src/target_identity.rs`

```rust
pub async fn compute_target_identity(client: &tokio_postgres::Client) -> Result<String, ApplyError> {
    let row = client.query_one(
        "SELECT current_database(), inet_server_addr()::text, inet_server_port(), \
         current_setting('cluster_name', true), pg_catalog.system_identifier() \
         FROM pg_control_system()", &[]
    ).await?;
    // Concatenate fields and BLAKE3-hash.
    let mut h = blake3::Hasher::new();
    h.update(b"pgevolve-target-id-v1\n");
    for i in 0..5 {
        let s: Option<&str> = row.try_get(i).ok();
        if let Some(v) = s { h.update(v.as_bytes()); }
        h.update(&[0]);
    }
    Ok(hex::encode(h.finalize().as_bytes())[..16].to_string())
}
```

> `pg_control_system()` and `system_identifier()` are read-only and present from PG 9.6+ — sufficient for v0.1.

Tests: identity is stable for the same DB across reconnects; differs between two distinct DBs (use two `EphemeralPostgres` instances).

Commit: `feat(cli): target-identity hashing of (host, port, dbname, system_identifier)`

---

### Task 8.4: Pre-flight checks

**File:** `crates/pgevolve/src/executor/preflight.rs`

```rust
pub async fn run_preflight(
    client: &tokio_postgres::Client,
    plan: &Plan,
    filter: &CatalogFilter,
    overrides: PreflightOverrides,
) -> Result<(), ApplyError> {
    // 1. Identity match
    let live_identity = compute_target_identity(client).await?;
    if live_identity != plan.metadata.target_identity && !overrides.allow_different_target {
        return Err(ApplyError::TargetIdentityMismatch {
            plan: plan.metadata.target_identity.clone(),
            live: live_identity,
        });
    }

    // 2. Drift recheck: re-introspect, compare to embedded snapshot
    let querier = PgCatalogQuerier::new(client);
    let live_catalog = pgevolve_core::catalog::read_catalog(&querier, filter)?;
    let drift = pgevolve_core::diff::diff(&plan.metadata.target_snapshot, &live_catalog);
    if !drift.is_empty() && !overrides.allow_drift {
        return Err(ApplyError::DriftDetected { changes: drift });
    }

    // 3. Intent enforcement
    let unapproved: Vec<_> = plan.intents.iter()
        .filter(|i| !i.approved)
        .collect();
    if !unapproved.is_empty() {
        return Err(ApplyError::UnapprovedIntents(
            unapproved.iter().map(|i| (i.id, i.target.clone(), i.reason.clone())).collect()
        ));
    }

    Ok(())
}

pub struct PreflightOverrides {
    pub allow_different_target: bool,
    pub allow_drift: bool,
}
```

Tests with `EphemeralPostgres`:
- Identity mismatch → error.
- Drift detected (modify the DB after planning) → error with rendered drift.
- Unapproved intent → error listing each.
- All clean → OK.

Commit: `feat(cli): preflight checks for identity, drift, and intent approval`

---

### Task 8.5: Audit writers

**File:** `crates/pgevolve/src/executor/audit.rs`

```rust
pub async fn open_apply_log(
    client: &tokio_postgres::Client,
    plan: &Plan,
    actor: &str,
) -> Result<Uuid, ApplyError> {
    let apply_id = Uuid::new_v4();
    client.execute(
        "INSERT INTO pgevolve.apply_log
         (apply_id, plan_id, plan_hash, pgevolve_version, source_rev, target_identity, actor, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'running')",
        &[
            &apply_id,
            &plan.id.short(),
            &plan.id.to_string(),
            &plan.metadata.pgevolve_version,
            &plan.metadata.source_rev,
            &plan.metadata.target_identity,
            &actor,
        ],
    ).await?;

    for group in &plan.groups {
        for step in &group.steps {
            client.execute(
                "INSERT INTO pgevolve.plan_steps
                 (apply_id, step_no, group_no, kind, destructive, targets, sql_text, status)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending')",
                &[
                    &apply_id, &(step.step_no as i32), &(group.id as i32),
                    &kind_name(&step.body.kind), &step.body.destructive,
                    &step.body.targets.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    &step.body.sql,
                ],
            ).await?;
        }
    }
    Ok(apply_id)
}

pub async fn mark_step_running(client: &tokio_postgres::Client, apply_id: Uuid, step_no: u32) -> Result<(), tokio_postgres::Error>;
pub async fn mark_step_succeeded(client: &tokio_postgres::Client, apply_id: Uuid, step_no: u32) -> Result<(), tokio_postgres::Error>;
pub async fn mark_step_failed(client: &tokio_postgres::Client, apply_id: Uuid, step_no: u32, err: &str) -> Result<(), tokio_postgres::Error>;
pub async fn mark_steps_rolled_back(client: &tokio_postgres::Client, apply_id: Uuid, group_no: u32) -> Result<(), tokio_postgres::Error>;

pub async fn close_apply_log(client: &tokio_postgres::Client, apply_id: Uuid, status: &str, err: Option<&str>) -> Result<(), tokio_postgres::Error>;
```

Tests: a small flow → assert rows exist with expected statuses.

Commit: `feat(cli): audit writers for apply_log and plan_steps`

---

### Task 8.6: Group/step execution loop

**File:** `crates/pgevolve/src/executor/execute.rs`

```rust
pub async fn execute_plan(
    client: &mut tokio_postgres::Client,
    plan: &Plan,
    apply_id: Uuid,
) -> Result<(), ApplyError> {
    for group in &plan.groups {
        if group.transactional {
            execute_transactional_group(client, apply_id, group).await?;
        } else {
            execute_autocommit_group(client, apply_id, group).await?;
        }
    }
    Ok(())
}

async fn execute_transactional_group(
    client: &mut tokio_postgres::Client,
    apply_id: Uuid,
    group: &TransactionGroup,
) -> Result<(), ApplyError> {
    let tx = client.transaction().await?;

    for step in &group.steps {
        mark_step_running(&tx.client(), apply_id, step.step_no).await?;
        match tx.batch_execute(&step.body.sql).await {
            Ok(()) => {
                mark_step_succeeded(&tx.client(), apply_id, step.step_no).await?;
            }
            Err(e) => {
                let err_msg = format!("{e}");
                tx.rollback().await?;
                // Re-open a fresh connection segment to mark rollback (rollback closed our tx).
                mark_step_failed(client, apply_id, step.step_no, &err_msg).await?;
                mark_steps_rolled_back(client, apply_id, group.id).await?;
                return Err(ApplyError::StepFailed {
                    step_no: step.step_no, group_no: group.id, error: err_msg,
                });
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn execute_autocommit_group(
    client: &tokio_postgres::Client,
    apply_id: Uuid,
    group: &TransactionGroup,
) -> Result<(), ApplyError> {
    for step in &group.steps {
        mark_step_running(client, apply_id, step.step_no).await?;
        match client.batch_execute(&step.body.sql).await {
            Ok(()) => mark_step_succeeded(client, apply_id, step.step_no).await?,
            Err(e) => {
                let err_msg = format!("{e}");
                mark_step_failed(client, apply_id, step.step_no, &err_msg).await?;
                return Err(ApplyError::StepFailed {
                    step_no: step.step_no, group_no: group.id, error: err_msg,
                });
            }
        }
    }
    Ok(())
}
```

Tests: a 3-step transactional group fails on step 2 → all 3 marked rolled_back. A 3-step non-tx group fails on step 2 → step 1 marked succeeded, step 2 marked failed, step 3 marked pending.

Commit: `feat(cli): transactional and autocommit step-execution loops with audit updates`

---

### Task 8.7: `apply()` entry point

**File:** `crates/pgevolve/src/executor/mod.rs`

```rust
pub async fn apply(
    plan_dir: &Path,
    db_options: DbOptions,
    overrides: ApplyOverrides,
) -> Result<ApplyOutcome, ApplyError> {
    let plan = pgevolve_core::plan::read_plan_dir(plan_dir)?;

    let (mut client, runtime) = connection::connect(&db_options).await?;
    bootstrap_metadata(&client).await?;

    let actor = compute_actor();
    let _lock = try_acquire_lock(&client, &actor).await?;
    run_preflight(&client, &plan, &db_options.filter, overrides.into()).await?;

    let apply_id = open_apply_log(&client, &plan, &actor).await?;
    match execute_plan(&mut client, &plan, apply_id).await {
        Ok(()) => {
            close_apply_log(&client, apply_id, "succeeded", None).await?;
            Ok(ApplyOutcome::Succeeded { apply_id })
        }
        Err(e) => {
            close_apply_log(&client, apply_id, "failed", Some(&format!("{e}"))).await?;
            Err(e)
        }
    }
}

pub enum ApplyOutcome {
    Succeeded { apply_id: Uuid },
}
```

Tests with `EphemeralPostgres`: end-to-end happy path; failure path produces non-zero exit + persisted error.

Commit: `feat(cli): apply() orchestrates bootstrap, lock, preflight, audit, execute`

---

### Task 8.8: `MigrationFixture` and `ApplyHarness`

**Files:**
- `crates/pgevolve-testkit/src/migration_fixture.rs`
- `crates/pgevolve-testkit/src/apply_harness.rs`

```rust
pub struct MigrationFixture {
    pub initial_source_dir: PathBuf,
    pub final_source_dir: PathBuf,
    pub expected_changes: Option<ChangeSummaryDoc>,
    pub data_assertions_sql: Option<String>,
}

pub struct ApplyHarness;

impl ApplyHarness {
    pub async fn run(fixture: &MigrationFixture, version: PgVersion) -> anyhow::Result<()> {
        let pg = EphemeralPostgres::start(version).await?;
        // 1. Plan and apply initial.
        let plan_dir_1 = tempdir()?;
        plan_and_apply(&pg, &fixture.initial_source_dir, plan_dir_1.path()).await?;
        let live_1 = read_catalog(&pg).await?;
        let parsed_initial = pgevolve_core::parse::parse_directory(&fixture.initial_source_dir, &[])?;
        assert_canonical_eq(&live_1, &parsed_initial)?;

        // 2. Plan and apply final.
        let plan_dir_2 = tempdir()?;
        plan_and_apply(&pg, &fixture.final_source_dir, plan_dir_2.path()).await?;
        let live_2 = read_catalog(&pg).await?;
        let parsed_final = pgevolve_core::parse::parse_directory(&fixture.final_source_dir, &[])?;
        assert_canonical_eq(&live_2, &parsed_final)?;

        // 3. Optional change-summary check.
        if let Some(expected) = &fixture.expected_changes { ... }

        // 4. Optional data assertions.
        if let Some(sql) = &fixture.data_assertions_sql {
            pg.exec_sql(sql).await?;
        }
        Ok(())
    }
}
```

Tier-4 fixture seed (one fixture sufficient for v0.1 exit):

`crates/pgevolve-core/tests/fixtures/e2e/0001-create-and-alter-table/`:
- `initial_source/schema/app/0001-init.sql`:
  ```sql
  CREATE SCHEMA app;
  CREATE TABLE app.users (
    id bigserial PRIMARY KEY,
    email text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now()
  );
  ```
- `final_source/schema/app/0001-init.sql`: same plus `display_name text;` column.
- `expected_changes.yaml` describing one `AddColumn`.

Commit: `feat(testkit): MigrationFixture loader and ApplyHarness for tier-4`

---

### Task 8.9: `status` command queries

**File:** `crates/pgevolve/src/executor/status.rs`

```rust
pub async fn fetch_recent_applies(client: &tokio_postgres::Client, limit: i32) -> Result<Vec<ApplyRecord>, ApplyError>;
pub async fn fetch_apply_steps(client: &tokio_postgres::Client, apply_id: Uuid) -> Result<Vec<StepRecord>, ApplyError>;
```

Plus formatters: `format_status_human`, `format_status_json`.

Tests with `EphemeralPostgres`: run an apply, fetch recent applies → 1 row, fetch its steps → all expected.

Commit: `feat(cli): status command queries and formatters`

---

### Task 8.10: Phase 8 self-review

- Walk spec §8 step-by-step against the code path in `apply()`. Every numbered step in §8 maps to a function call.
- Tier-4 fixture passes against PG 16 in CI.
- Lock contention test passes.
- Drift recheck test passes.

Phase 8 complete.
