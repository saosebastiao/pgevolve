# Phase 6 — Online-rewrite pass and step grouping

**Goal:** Transform `OrderedChangeSet` into `Vec<RawStep>` with online-friendly rewrites applied, then partition into `TransactionGroup`s ready to be serialized to a `Plan`.

**Spec coverage:** §6.5.

**Depends on:** Phase 5.

**Exit criteria:**

- `pgevolve_core::plan::rewrite(ordered: OrderedChangeSet, target: &Catalog, policy: &PlannerPolicy) -> Vec<RawStep>` applies the four documented v0.1 rewrites.
- Each rewrite is gated on `PlannerPolicy` so phase-2 per-environment overrides plug in cleanly.
- `group_steps(steps: Vec<RawStep>) -> Vec<TransactionGroup>` partitions by transactional/non-transactional boundary.
- > 20 unit tests across each rewrite path.

---

## File structure

```
crates/pgevolve-core/src/
└── plan/
    ├── policy.rs                 # PlannerPolicy + Strategy enum
    ├── raw_step.rs               # RawStep + StepKind
    ├── transaction_group.rs      # TransactionGroup
    ├── rewrite/
    │   ├── mod.rs                # rewrite() entry; iterates rule application
    │   ├── concurrent_index.rs
    │   ├── fk_not_valid_validate.rs
    │   ├── check_not_valid_validate.rs
    │   └── set_not_null_check_pattern.rs
    └── grouping.rs               # group_steps()
```

---

### Task 6.1: `PlannerPolicy`

**File:** `crates/pgevolve-core/src/plan/policy.rs`

```rust
pub enum Strategy { Atomic, Online }

pub struct PlannerPolicy {
    pub strategy: Strategy,
    pub online: OnlineRewrites,
    pub planner_ruleset_version: u32,
}

pub struct OnlineRewrites {
    pub create_index_concurrent:    bool,
    pub fk_not_valid_then_validate: bool,
    pub check_not_valid_then_validate: bool,
    pub not_null_via_check_pattern: bool,
}

impl Default for PlannerPolicy {
    fn default() -> Self {
        Self {
            strategy: Strategy::Online,
            online: OnlineRewrites {
                create_index_concurrent:    true,
                fk_not_valid_then_validate: true,
                check_not_valid_then_validate: true,
                not_null_via_check_pattern: true,
            },
            planner_ruleset_version: 1,
        }
    }
}
```

`Strategy::Atomic` overrides every `online.*` switch to `false` regardless of values — atomic mode is "single transaction, no rewrites." This makes the env-override story (per-env "atomic" or "online") trivially implementable later.

Tests: `Atomic` disables online rewrites; `Online` respects individual switches.

Commit: `feat(core): PlannerPolicy with strategy and per-rewrite switches`

---

### Task 6.2: `RawStep` and `StepKind`

**File:** `crates/pgevolve-core/src/plan/raw_step.rs`

A `RawStep` is the smallest unit of work the executor can attempt. After rewrites, every step's SQL is fixed — no further transformation happens between here and execution.

```rust
pub struct RawStep {
    pub kind: StepKind,
    pub destructive: bool,
    pub destructive_reason: Option<String>,
    pub intent_id: Option<u32>,           // populated later, in plan/serialize.rs
    pub targets: Vec<QualifiedName>,
    pub sql: String,
    pub transactional: TransactionConstraint,
}

pub enum TransactionConstraint {
    InTransaction,         // can be inside a BEGIN/COMMIT
    OutsideTransaction,    // CONCURRENTLY etc.
}

pub enum StepKind {
    CreateSchema, DropSchema, AlterSchemaComment,
    CreateTable, DropTable, AlterTableSetComment,
    AddColumn, DropColumn, AlterColumnType,
    SetColumnNullable,
    SetColumnDefault, SetColumnComment,
    SetColumnIdentity, SetColumnGenerated,
    AddConstraint, AddConstraintNotValid, ValidateConstraint, DropConstraint, SetConstraintComment,
    CreateIndex, CreateIndexConcurrent, DropIndex, DropIndexConcurrent,
    CreateSequence, DropSequence, AlterSequence,
    AddCheckForNotNull, // intermediate step in the SET NOT NULL pattern
}
```

`StepKind` is `serde::Serialize`-able — used as the `kind=` value in the step directive comments.

Tests: enum round-trips through serde-string.

Commit: `feat(core): RawStep + StepKind enums`

---

### Task 6.3: `rewrite()` entry point + non-rewriting passthrough

**File:** `crates/pgevolve-core/src/plan/rewrite/mod.rs`

```rust
pub fn rewrite(
    ordered: OrderedChangeSet,
    target: &Catalog,
    policy: &PlannerPolicy,
) -> Vec<RawStep> {
    let mut out = Vec::new();
    for entry in ordered.creates_and_adds {
        emit_steps_for(&entry, target, policy, &mut out);
    }
    for entry in ordered.modifies {
        emit_steps_for(&entry, target, policy, &mut out);
    }
    for entry in ordered.drops {
        emit_steps_for(&entry, target, policy, &mut out);
    }
    for fk in ordered.deferred_fks {
        emit_steps_for_deferred_fk(&fk, target, policy, &mut out);
    }
    out
}
```

`emit_steps_for` dispatches each `Change` / `TableOp` / `SequenceOp` to a SQL emitter. For most changes, the emitter produces one `RawStep`. For the rewritten changes (next four tasks), it produces multiple.

This task lands the dispatcher and the SQL emitters for **non-rewritten** changes:
- `CreateSchema(Schema)` → `CREATE SCHEMA app;`
- `CreateTable(t)` → `CREATE TABLE app.users (...);`
- `DropTable { qname }` → `DROP TABLE app.users;`
- `CreateIndex(i)` *for new tables* (i.e., not destined for the CONCURRENTLY rewrite — see task 6.4) → `CREATE INDEX ...;`
- ... and so on for every non-rewritten variant.

Helper: `Catalog::table_exists(qname) -> bool` to decide "are we building a brand-new table, or altering an existing one?"

Tests: each non-rewritten change emits the expected single `RawStep` with the right `StepKind`, `transactional = InTransaction`, and SQL.

Commit: `feat(core): rewrite() dispatcher with single-step SQL emission for non-rewritten changes`

---

### Task 6.4: Concurrent-index rewrite

**File:** `crates/pgevolve-core/src/plan/rewrite/concurrent_index.rs`

When emitting a `Change::CreateIndex(idx)`:

- If `target.table_exists(idx.table)` AND `policy.online.create_index_concurrent` AND `!idx.unique` → emit `CREATE INDEX CONCURRENTLY` as a `RawStep { kind: CreateIndexConcurrent, transactional: OutsideTransaction }`.
- Otherwise → emit `CREATE INDEX` as `kind: CreateIndex, transactional: InTransaction`.

Unique indexes are NOT rewritten in v0.1 because `CREATE UNIQUE INDEX CONCURRENTLY` can leave behind an INVALID index that's hard to clean up; v0.1 plays safe and uses the locking variant. Document this as a known limitation; it's a candidate for opt-in policy later.

Same logic for `Change::DropIndex` → `DROP INDEX CONCURRENTLY` when policy permits and the index is non-unique.

Tests:
- Index on existing table, non-unique → CONCURRENT step.
- Unique index → non-concurrent.
- Index on a table being created in the same plan → non-concurrent (because it can be inside the same transaction as the CREATE TABLE).
- Atomic policy → non-concurrent regardless.

Commit: `feat(core): rewrite CreateIndex/DropIndex on existing tables to CONCURRENTLY`

---

### Task 6.5: FK NOT VALID + VALIDATE rewrite

**File:** `crates/pgevolve-core/src/plan/rewrite/fk_not_valid_validate.rs`

When emitting `TableOp::AddConstraint(Constraint { kind: ForeignKey, .. })` on an existing table:

- Step A: `ALTER TABLE x ADD CONSTRAINT name ... NOT VALID;` → `kind: AddConstraintNotValid, transactional: InTransaction`.
- Step B: `ALTER TABLE x VALIDATE CONSTRAINT name;` → `kind: ValidateConstraint, transactional: InTransaction`. **Step B is in a separate `RawStep`** so the planner can put it in a different transaction group from step A — splitting them lets the user observe step A's outcome before committing to the long-running step B.

Without rewriting (atomic policy or for an FK on a brand-new table): single `AddConstraint` step.

Tests:
- FK on existing table → 2 steps.
- FK on new table (table being created in same plan) → 1 step inline in the `CREATE TABLE`.
- FK on existing table with `Atomic` policy → 1 step.

Commit: `feat(core): rewrite FK adds on existing tables to NOT VALID + VALIDATE`

---

### Task 6.6: CHECK NOT VALID + VALIDATE rewrite

Same shape as FK rewrite but for `Constraint { kind: ConstraintKind::Check { .. } }`.

Commit: `feat(core): rewrite CHECK adds on existing tables to NOT VALID + VALIDATE`

---

### Task 6.7: SET NOT NULL via CHECK pattern

**File:** `crates/pgevolve-core/src/plan/rewrite/set_not_null_check_pattern.rs`

When emitting `TableOp::SetColumnNullable { name, nullable: false }` on a column that exists in `target` (i.e., not just being added) AND policy permits AND `target` has rows estimated > 0:

1. `ALTER TABLE x ADD CONSTRAINT __pgevolve_chk_<col> CHECK (col IS NOT NULL) NOT VALID;` → `AddConstraintNotValid` (using a synthesized constraint name; document the prefix).
2. `ALTER TABLE x VALIDATE CONSTRAINT __pgevolve_chk_<col>;` → `ValidateConstraint`.
3. `ALTER TABLE x ALTER COLUMN col SET NOT NULL;` → `SetColumnNullable`. (Cheap once the CHECK is validated — Postgres uses the CHECK to skip the table scan.)
4. `ALTER TABLE x DROP CONSTRAINT __pgevolve_chk_<col>;` → `DropConstraint`.

If the column is being added in the same plan (i.e., the parent change is `AddColumn(c)` where `c.nullable = false`), no rewrite — just include `NOT NULL` inline in the `ADD COLUMN` SQL.

Tests:
- Existing populated column → 4 steps.
- Newly added column → 0 extra steps (NOT NULL inline).
- Atomic policy → single `SET NOT NULL` step (with appropriate destructive flag).

Commit: `feat(core): rewrite SET NOT NULL on populated columns to CHECK pattern`

---

### Task 6.8: Step grouping

**File:** `crates/pgevolve-core/src/plan/grouping.rs`

```rust
pub struct TransactionGroup {
    pub id: u32,             // 1-indexed
    pub transactional: bool,
    pub steps: Vec<RawStep>, // step numbers assigned at serialize time
}

pub fn group_steps(steps: Vec<RawStep>) -> Vec<TransactionGroup> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut current_kind: Option<TransactionConstraint> = None;

    for step in steps {
        match (current_kind, step.transactional) {
            (None, k) => {
                current.push(step);
                current_kind = Some(k);
            }
            (Some(prev), k) if prev == k => {
                current.push(step);
            }
            (Some(_prev), k) => {
                groups.push(make_group(groups.len() + 1, current_kind.unwrap(), std::mem::take(&mut current)));
                current.push(step);
                current_kind = Some(k);
            }
        }
    }
    if !current.is_empty() {
        groups.push(make_group(groups.len() + 1, current_kind.unwrap(), current));
    }
    groups
}

fn make_group(id: usize, c: TransactionConstraint, steps: Vec<RawStep>) -> TransactionGroup {
    TransactionGroup {
        id: id as u32,
        transactional: matches!(c, TransactionConstraint::InTransaction),
        steps,
    }
}
```

Within a non-transactional group, each step actually runs autocommit (the spec calls these "singleton groups"). The grouping logic puts consecutive non-tx steps in the same `TransactionGroup` for organizational reasons but the executor treats each step as autocommit.

Tests:
- All-in-tx → 1 group.
- Tx → non-tx → tx → 3 groups.
- Empty input → 0 groups.

Commit: `feat(core): group_steps partitions by transactional boundary`

---

### Task 6.9: Phase 6 self-review

- Spec §6.5: each of the four documented rewrites has its own task and tests.
- Default `PlannerPolicy` produces v0.1's expected behavior.
- `Atomic` strategy short-circuits all rewrites.
- `cargo test -p pgevolve-core` passes; clippy clean.

Phase 6 complete.
