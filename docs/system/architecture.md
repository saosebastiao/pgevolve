# Architecture

A guided tour of pgevolve's internals: the crates, the data flow, the
key invariants, and the design decisions that shaped each.

## TL;DR

pgevolve is built on a **declarative IR**. Source SQL and live-database
introspection both fold into the same `Catalog` type; the planner
computes the difference; the executor applies the difference under
strict transactional and audit guarantees.

```
schema/*.sql ──parse──►   Catalog (source) ─────┐
                                                │
                                                ├──diff──► ChangeSet ──order──► OrderedChangeSet
                                                │
live Postgres ──introspect──► Catalog (target) ─┘                                       │
                                                                                        │
                                                                                        │ rewrite
                                                                                        ▼
                                                                                  Vec<RawStep>
                                                                                        │
                                                                                        │ group_steps
                                                                                        ▼
                                                                                Vec<TransactionGroup>
                                                                                        │
                                                                                        │ Plan::from_grouped
                                                                                        ▼
                                                                                       Plan ───► plan.sql
                                                                                                 intent.toml
                                                                                                 manifest.toml
                                                                                        │
                                                                                        │ apply()
                                                                                        ▼
                                                                                  live Postgres
```

Every box and arrow is a module-level boundary. Sections below walk each.

## Crate layout

```
crates/
├── pgevolve-core/        I/O-free library: IR, parser, diff, planner, plan format, lint
├── pgevolve/             CLI binary + executor (the only crate that depends on tokio_postgres)
├── pgevolve-testkit/     Internal-only test infra (publish = false)
└── xtask/                `cargo xtask bless` for regenerating goldens
```

### `pgevolve-core` — the brain

| Module | Responsibility |
|---|---|
| `identifier` | `Identifier` (single SQL name) and `QualifiedName` (`schema.name`). Quoting / validation. |
| `ir/` | The data model. `Catalog`, `Schema`, `Table`, `Column`, `Index`, `Sequence`, `Constraint`, plus `ColumnType` (canonical type form), `DefaultExpr`, `NormalizedExpr`. |
| `parse/` | Source-side SQL → IR. Wraps `pg_query`. |
| `catalog/` | Live-PG → IR. Defines `CatalogQuerier` (sync trait) and the per-version SQL strings; the actual `tokio_postgres` adapter lives in the binary. |
| `diff/` | `Catalog × Catalog → ChangeSet`. Pair-by-qname semantics; destructiveness classification. |
| `plan/` | The planner: order → rewrite → group → write/read. Plan format and `PlanId` hashing. |
| `lint/` | Universal rules + four built-in layout profiles + custom-profile regex+assertion mechanism. |

**Invariant:** `pgevolve-core` does no I/O at the type level. The only
filesystem walk is `parse::parse_directory`, which is the explicit
entry point. Everything else is library-style data manipulation.

### `pgevolve` — the binary and the runtime

| Module | Responsibility |
|---|---|
| `cli` | clap subcommand definitions. |
| `commands/` | One file per subcommand. `init`, `lint`, `validate`, `diff`, `plan`, `apply`, `status`, `bootstrap`, `dump` (stub). |
| `config` | `pgevolve.toml` loader + validation. |
| `connection` | DSN resolution (CLI > env.url > env.url_env > `PGEVOLVE_DATABASE_URL` > libpq env). |
| `executor/` | The apply loop: bootstrap, lock, target-identity, preflight, audit, execute, status. |
| `pg_querier` | `tokio_postgres`-backed `CatalogQuerier`. Mirrors the testkit one to avoid pulling `testcontainers` into the binary. |
| `shadow_pg` | testcontainers wrapper used by `validate --shadow`. |
| `target_identity` | BLAKE3 hash of `(current_database, host, port, cluster_name, system_identifier)`. |

### `pgevolve-testkit` — internal-only test infra

Holds `EphemeralPostgres` (testcontainers wrapper), `PgCatalogQuerier`
(the same adapter the binary uses, exposed for tier-3 tests), the
`MigrationFixture` loader, the IR generator + mutator, and the
`assert_canonical_eq` helper. Not published; `publish = false` in
`Cargo.toml`.

### `xtask` — workspace-local tooling

A binary invoked via `cargo xtask <subcommand>`. Currently only
`bless`, which regenerates tier-3 catalog goldens by running the
fixtures against ephemeral containers and writing canonical JSON.

## Data flow, in more detail

### Parse → IR (source)

`parse_directory(root, ignores)`:

1. Walks `root` in sorted order, picking up `*.sql` files.
2. Runs `pg_query::parse` on each file.
3. Classifies every top-level statement against the v0.1 whitelist
   (`CREATE SCHEMA / TABLE / INDEX / SEQUENCE`, the FK-whitelist
   `ALTER TABLE`, `COMMENT ON`).
4. Builds an IR object per statement.
5. Tracks every object's `SourceLocation` for the linter.
6. Calls `Catalog::canonicalize` at the end (sorts collections, rejects
   duplicate qnames).

Output: a `Catalog`. Optionally, with
`parse_directory_with_locations`, a `(Catalog, HashMap<String,
SourceLocation>)` for the linter.

### Introspect → IR (target)

`pgevolve_core::catalog::read_catalog(querier, filter)`:

1. Detects the server version (PG 14/15/16/17).
2. For each `CatalogQuery` kind (Schemas, Tables, Columns, etc.) picks
   the per-version SQL string and runs it via the querier.
3. Decodes rows into typed `Value`s.
4. Assembles a `Catalog` and canonicalizes.

The `CatalogQuerier` is a synchronous trait — the binary's
`PgCatalogQuerier` bridges to async `tokio_postgres` via
`block_in_place`. This keeps `pgevolve-core` runtime-agnostic.

### Diff

`pgevolve_core::diff::diff(target, source) → ChangeSet`:

- Tables, indexes, sequences pair by qualified name.
- Columns and constraints inside a table pair by bare name.
- Each `ChangeEntry` carries a `Destructiveness` tag: `Safe`,
  `RequiresApproval`, or `RequiresApprovalAndDataLossWarning`.

### Planner: order

`pgevolve_core::plan::order(target, source, changes) →
OrderedChangeSet`. Three buckets:

1. **Creates and additive ops** — topo-sorted via the source-side
   dependency graph.
2. **Modify-in-place** — same graph (column-type changes, constraint
   replacements).
3. **Drops** — reverse-topo-sorted via the target-side graph.

The dependency graph has these edge sources (spec §6.4):

- `schema ← table ← column-default-using-sequence`
- `table ← index`
- `FK constraint ← both endpoints`
- `sequence ← owning table (OWNED BY)`

FK cycles (chicken-and-egg between two tables) are broken by
**extracting** offending FKs into a post-pass `DeferredFkAdd` list and
re-running the topo sort. The deferred FKs become `ALTER TABLE ADD
CONSTRAINT` steps after both tables are created.

### Planner: rewrite

`pgevolve_core::plan::rewrite(ordered, target, policy) → Vec<RawStep>`.
Each change becomes one or more `RawStep`s. Four documented online
rewrites (gated by `PlannerPolicy`):

1. **Concurrent index** — `CREATE INDEX CONCURRENTLY` for non-unique
   indexes on existing tables. Runs in its own non-transactional
   group.
2. **FK NOT VALID + VALIDATE** — Adding an FK on an existing table
   splits into two steps in two transaction groups.
3. **CHECK NOT VALID + VALIDATE** — Same shape for CHECK constraints.
4. **SET NOT NULL via CHECK pattern** — Four-step pattern that avoids
   the long `ACCESS EXCLUSIVE` of a naive `SET NOT NULL`.

`Strategy::Atomic` short-circuits every rewrite — one big transaction,
no online tricks. Useful for hermetic dev / test environments.

### Planner: group

`group_steps(steps) → Vec<TransactionGroup>` coalesces adjacent steps
with the same `TransactionConstraint`. Each transactional group runs
inside one `BEGIN; … COMMIT;`. Non-transactional groups host
`CONCURRENTLY` operations (autocommit singletons).

### Plan format

`Plan::from_grouped` assigns 1-indexed step numbers, allocates an
`intent_id` per destructive step, and computes the `PlanId`.

**`PlanId` derivation** (`pgevolve_core::plan::plan::PlanId::compute`):

```
BLAKE3(
    "pgevolve-plan-id-v1\n"
    || pgevolve_version || 0x00
    || planner_ruleset_version (big-endian u32) || 0x00
    || bincode(source_catalog) || 0x00
    || bincode(target_catalog)
)
```

Bincode is used because its encoding is deterministic across runs and
machines. Identical inputs produce identical bytes; the hash is the
identity. `serde_json` was rejected here because float / map orderings
aren't byte-deterministic across versions.

**Three-file on-disk format:**

- `plan.sql` — canonical artifact. Runs cleanly under `psql -f`.
  Directive comments (`-- @pgevolve ...`) carry the structured data the
  executor needs.
- `intent.toml` — destructive intents, `approved = false` by default.
- `manifest.toml` — plan id (full hex), version metadata, target
  identity, embedded pre-image catalog as JSON.

### Executor

`pgevolve::executor::apply(plan_dir, client, filter, overrides)`:

1. `read_plan_dir` — load the three files; cross-check the plan id.
2. `bootstrap_metadata` — idempotent install of `pgevolve.*` tables.
3. `try_acquire_lock` — `pg_try_advisory_lock(PGEVOLVE_LOCK_KEY)`.
4. `run_preflight` — target-identity check, drift recheck, intent
   approval check.
5. `open_apply_log` — insert `apply_log` row (status `running`),
   pre-populate `plan_steps` as `pending`.
6. `execute_plan` — per-group transactional or autocommit execution;
   audit each step's transition.
7. `close_apply_log` — set status `succeeded` / `failed` / `aborted`.
8. `release_lock` — clear the lock row + advisory unlock.

## Key invariants

These are testable, must-hold-or-the-build-breaks properties.

1. **`Catalog::diff` is byte-deterministic.** Identical IRs produce an
   empty diff. Two different IRs always produce the same diff.
2. **`PlanId::compute` is byte-deterministic.** Same inputs ⇒ same id,
   on any machine.
3. **`write_plan_dir` then `read_plan_dir` round-trips** (modulo
   destructive_reason, which is grafted from `intent.toml`).
4. **Topological order is deterministic.** Ties broken by the smallest
   node per `Ord`; the planner's output is byte-stable.
5. **No I/O in `pgevolve-core` at the type level.** The only fs walk is
   the explicit `parse_directory`.
6. **The advisory lock is singleton.** `try_acquire_lock` succeeds for
   at most one session at a time. Property-tested.
7. **No partial success.** Apply either succeeds end-to-end or reports
   the exact failed step in `pgevolve.plan_steps`.
8. **No silent data loss.** Destructive steps require approved
   intents; pre-flight refuses to run with `approved = false`.

## Design decisions worth knowing

### Why an IR (and not just diff SQL text)?

Postgres has many ways to write the same thing: `'foo'::text` vs
`'foo'`, `NUMERIC` vs `NUMERIC(38, 0)`, `int4` vs `integer`. A
text-level diff would noise-trip on every cosmetic difference. The IR
canonicalizes — paren folding, keyword case, type aliases, etc. — so
that semantically-equal inputs produce equal `Catalog` values.

### Why three files in a plan directory (vs. one)?

- `plan.sql` is the **review artifact**. Reviewers read SQL.
- `intent.toml` is the **approval artifact**. The diff in a PR for
  `intent.toml` is the exact destructive change being authorized.
- `manifest.toml` is the **integrity artifact**. The embedded pre-image
  + full hex hash + plan-id cross-check means the executor can refuse
  to run a tampered plan.

Splitting these means the right people review the right surface.

### Why three-phase ordering (vs. one topological sort)?

Drops have to run in **reverse** of creates. Modify ops can reference
either pre- or post-image. Splitting into three buckets with two
graphs (source for creates/modifies, target for drops) is the
smallest model that handles every case correctly.

### Why FK-cycle extraction (vs. deferred constraints or topological-sort failures)?

Inline FKs in `CREATE TABLE` create chicken-and-egg cycles when two
tables FK each other. Postgres supports `DEFERRABLE` constraints, but
that's a runtime semantics shift and not all FKs are deferrable.
Extracting the offending FKs into `ALTER TABLE ADD CONSTRAINT` after
both tables exist is the surgical fix.

### Why `bincode` for `PlanId`?

The hash payload doesn't need to be human-readable. Bincode is binary,
deterministic, and several times faster than the alternatives.
**Note:** pinned to v2 because v3 dropped the serde feature.

### Why does `pgevolve-core` not depend on `tokio_postgres`?

Keeps the library testable without a running runtime, and makes it
plausible to add other backends (file-based, raw libpq, etc.) without
restructuring. The `CatalogQuerier` trait is the integration point;
the binary's `pg_querier` is the only impl today.

### Why are advisory locks session-scoped, not transaction-scoped?

Apply spans multiple transactions (e.g., one transactional group + one
autocommit group). A transaction-scoped lock would release between
groups; a session-scoped one stays held for the whole apply.

### Why does `validate --shadow` re-implement parts of `apply`?

Because it has to apply the source IR to a fresh database from
scratch, with `target_identity` set to the live shadow's identity (not
whatever was in the source `pgevolve.toml`). It builds a plan
in-memory and writes to a tempdir, then calls the same `executor::apply`
the regular `apply` command uses.

## Where each invariant is tested

| Invariant | Test |
|---|---|
| Diff determinism | Tier 1 unit tests in `diff/` + tier 5 property test `plan_id_is_deterministic` (which transitively requires diff determinism). |
| `PlanId` determinism | `plan_id_is_deterministic` property test. |
| Plan round-trip | `read_plan_dir_round_trips_whole_plan` (unit) + `round_trip_property` (PG-bound property test). |
| Topo-sort determinism | `deterministic_under_insertion_order_changes` + property test on ordered changes. |
| `pgevolve-core` no-I/O | Compile-time: `pgevolve-core` has no `tokio` / `tokio_postgres` in its deps. |
| Lock singleton | `advisory_lock_contention` tier-4 test. |
| No partial success | `apply_rolls_back_transactional_group_on_failure` tier-4 test. |
| No silent data loss | Intent approval is checked at preflight (test pending; phase-9 follow-up). |
