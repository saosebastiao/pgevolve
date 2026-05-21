# pgevolve

Postgres-specific declarative schema management.

[![crates.io](https://img.shields.io/crates/v/pgevolve.svg)](https://crates.io/crates/pgevolve)
[![docs.rs](https://img.shields.io/docsrs/pgevolve-core)](https://docs.rs/pgevolve-core)
[![CI](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml/badge.svg)](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`pgevolve` treats a directory of `CREATE`-style SQL files as the source of
truth for one or more Postgres schemas, introspects a live database to
derive its current state, and computes ordered, dependency-aware migration
plans that bring the database to the desired state. It refuses to lose
data unless explicitly authorized in a per-plan intent file.

> **Status:** v0.1.0 tagged (the schemas + tables + indexes + sequences surface). v0.2 sub-spec series in progress — sub-specs #1 (views/MVs), #2 (types), #3 (extensions), #4 (functions/procedures), #5 (triggers), and #6 (declarative partitioning) merged. See [`CHANGELOG.md`](./CHANGELOG.md) for what's in each version.

## Install

From [crates.io](https://crates.io/crates/pgevolve) (recommended):

```sh
cargo install pgevolve
```

From source:

```sh
git clone https://github.com/saosebastiao/pgevolve
cd pgevolve
cargo install --path crates/pgevolve
```

`pgevolve` requires Rust 1.95+.

## Usage at a glance

```sh
# 1. Scaffold a project (creates `pgevolve.toml`, `schema/`, `plans/`).
pgevolve init

# 2. Author your schema as CREATE-style SQL under `schema/`.
#    File layout is enforced by the `layout_profile` setting; the default
#    (`schema-mirror`) wants `schema/<schema>/<kind>/<name>.sql`.

# 3. Plan a migration against a configured environment.
pgevolve plan --db dev
#   wrote plans/2026-05-12-<short-id>/{plan.sql, intent.toml, manifest.toml}

# 4. Review the generated plan.sql + intent.toml in code review.

# 5. Apply it.
pgevolve apply plans/2026-05-12-<short-id> --db dev
```

### CLI surface

| Command | What it does | Touches DB | Writes files |
|---|---|---|---|
| `pgevolve init` | Scaffold project files | no | yes |
| `pgevolve lint` | Run universal + layout-profile rules | no | no |
| `pgevolve validate` | Parse + build source IR; with `--shadow`, round-trip the IR through ephemeral Postgres; `--shadow-validate` cross-checks dep graph against `pg_depend` | shadow only | no |
| `pgevolve diff --db <env>` | Build source + catalog IR; print change set (`--format=human|json|sql`); `--shadow-validate` opt-in | read-only | no |
| `pgevolve plan --db <env> [-o <dir>]` | Full pipeline; write plan directory; gates on unwaived `LintAtPlan` findings; `--shadow-validate` opt-in | read-only | yes |
| `pgevolve apply <plan-dir> --db <env>` | Execute plan | read+write | no |
| `pgevolve status --db <env>` | Show recent applies and per-step state | read-only | no |
| `pgevolve dump --db <env> -o <dir>` | Introspect live DB and write source SQL | read-only | yes — *v0.1.x* |
| `pgevolve bootstrap --db <env>` | Install/upgrade the `pgevolve` metadata schema | read+write | no |
| `pgevolve graph` | Render source dep graph (`--graph-format dot\|mermaid`); no DB required | no | optional (`-o`) |
| `pgevolve doctor --db <env>` | Health check: bootstrap status, NOT VALID constraints, INVALID indexes, object counts, recent failures | read-only | no |
| `pgevolve rewrite-table <qname> --db <env> --confirm-rewrite` | Destructive table rewrite — *v0.2 skeleton; not yet implemented* | — | — |

Exit codes follow spec §13: `0` success, `1` lint/validation error, `2`
drift or pre-flight mismatch, `3` apply error, `4` config or CLI input
error.

## Configuration

`pgevolve.toml`:

```toml
[project]
name           = "myapp"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "schema-mirror"          # also: kind-grouped | feature-grouped | free-form | <path-to-custom.toml>

[managed]
schemas        = ["app", "billing"]       # schemas under pgevolve's control
ignore_objects = ["app.legacy_etl_*"]     # qname or glob

[planner]
strategy = "online"                       # "atomic" | "online"

[planner.online_rewrites]
create_index_concurrent       = true
fk_not_valid_then_validate    = true
check_not_valid_then_validate = true
not_null_via_check_pattern    = true

[environments.dev]
url = "postgres://localhost/myapp_dev"

[environments.prod]
url_env  = "DATABASE_URL_PROD"            # read DSN from env var
strategy = "online"
```

Connection precedence (mirrors `psql`):
`--url` CLI flag → `[environments.<env>].url` →
`[environments.<env>].url_env` → `PGEVOLVE_DATABASE_URL` →
libpq env (`PGHOST`, `PGUSER`, ...).

## Documentation

- [`docs/user/`](./docs/user/) — **user guide**: installation, getting
  started, configuration reference, command reference, cookbook,
  troubleshooting, plan-format walkthrough. Start here if you're
  operating pgevolve on a project.
- [`docs/system/`](./docs/system/) — **architecture and internals**:
  the dedicated [architecture chapter](./docs/system/architecture.md),
  deep dives on the IR, planner, and executor. Start here if you want
  to understand how pgevolve works under the hood.
- [`docs/spec/`](./docs/spec/) — **living capability catalogue**: every
  object kind, column type, constraint, index option, CLI command,
  etc., with its implementation status (Implemented / Partial /
  Planned / Future / Not planned). Start here to find out whether
  pgevolve does / will do a given thing.
- [`docs/superpowers/specs/2026-05-09-pgevolve-design.md`](./docs/superpowers/specs/2026-05-09-pgevolve-design.md) — original v0.1 design doc.
- [`docs/superpowers/plans/`](./docs/superpowers/plans/) —
  phase-by-phase implementation plans.

### v0.1 phase progress

| Phase | Title                        | Status   |
|-------|------------------------------|----------|
| 0     | Workspace                    | done     |
| 1     | IR                           | done     |
| 2     | Source parser                | done     |
| 3     | Catalog reader               | done     |
| 4     | Differ                       | done     |
| 5     | Planner                      | done     |
| 6     | Rewrites                     | done     |
| 7     | Plan format                  | done     |
| 8     | Executor                     | done     |
| 9     | CLI                          | done     |
| 10    | Linter                       | done     |
| 11    | Testkit                      | done     |
| 12    | Shadow                       | done     |

### v0.2 sub-spec progress

Per the [arch-readiness spec §16](./docs/superpowers/specs/2026-05-15-v0.2-architecture-review-design.md), v0.2 ships as a sequence of per-object-family sub-specs on the v0.2-readiness foundation.

| # | Sub-spec | Status |
|---|---|---|
| 0 | Architecture readiness (foundation) | ✅ Landed `26d8ebc..ec774ff` |
| 1 | Views and materialized views | ✅ Landed `0e2a7a0`; T13 shadow-validate `b96899f` |
| 2 | Types (enums, domains, composites) | ✅ Landed `6127bdd` |
| 3 | Extensions | ✅ Landed `c17b3bc..8522c95` |
| 4 | Functions and procedures | ✅ Landed `3c3f6ee` |
| 5 | Triggers | ✅ Landed `25ca3a5` |
| 6 | Declarative partitioning + table reloptions | ✅ Landed `fbff481` |

v0.3+ work (cluster-level surface — roles, GRANTs, `postgresql.conf`, RLS) is sketched in the arch spec §17 but not yet designed.

### v0.2 views/MVs — what's in `0e2a7a0`

| Feature | Status |
|---|---|
| Views (`CREATE VIEW`, `DROP VIEW`, `CREATE OR REPLACE VIEW`) | ✅ Implemented |
| Materialized views (`CREATE MATERIALIZED VIEW`, `REFRESH [CONCURRENTLY]`) | ✅ Implemented |
| AST body canonicalization (`NormalizedBody::from_sql`) | ✅ Implemented |
| OR-REPLACE compatibility predicate | ✅ Implemented |
| Dependent-view recreation cascade (transitive, topologically ordered) | ✅ Implemented |
| Online rewrite: `REFRESH CONCURRENTLY` (when unique index present) | ✅ Implemented |
| `[[step_override]]` in `intent.toml` | ✅ Implemented |
| 3 new lint rules (`view-shadows-table`, `mv-no-unique-index`, `view-body-references-unmanaged-schema`) | ✅ Implemented |
| 15 conformance fixtures (views, MVs, intent, dep-chains) | ✅ Implemented |
| `--shadow-validate` cross-check extended for view bodies | 📋 Deferred past v0.2.0 — [plan filed](./docs/superpowers/plans/2026-05-18-t13-shadow-validate-views.md) |

### v0.2 types — what's in `6127bdd`

| Feature | Status |
|---|---|
| Enum types (`CREATE TYPE … AS ENUM`, `ALTER TYPE … ADD VALUE`, `RENAME VALUE`) | ✅ Implemented |
| Domain types (`CREATE DOMAIN`, `ALTER DOMAIN ADD/DROP CONSTRAINT`, `SET/DROP DEFAULT`, `SET/DROP NOT NULL`) | ✅ Implemented |
| Composite types (`CREATE TYPE … AS (…)`, `ALTER TYPE … ADD/DROP/ALTER ATTRIBUTE`) | ✅ Implemented |
| `ReplaceWithCascade` fallback (drop + recreate when in-place ALTER is unsafe) | ✅ Implemented |
| `NodeId::Type` + dep-graph edges (type → column, type → type) | ✅ Implemented |
| Catalog reader for all three type families | ✅ Implemented |
| 4 new lint rules (`type-shadows-table`, `enum-value-collision`, `composite-attribute-collision`, `domain-check-references-unmanaged-type`) | ✅ Implemented |
| 20 conformance fixtures (enums, domains, composites, cascades, lints) | ✅ Implemented |
| Property test `enum_add_value_preserves_existing_values` (pure, `#[ignore]`'d) | ✅ Implemented |

### v0.2 extensions — what's in `c17b3bc..8522c95`

| Feature | Status |
|---|---|
| `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']` parser | ✅ Implemented |
| Catalog reader for `pg_extension` (joined with `pg_namespace`, `pg_description`) | ✅ Implemented |
| `pg_depend deptype='e'` filter on tables/indexes/sequences/views/MVs/types/functions queries | ✅ Implemented |
| `ExtensionChange` variants: Create, Drop, AlterUpdate, ReplaceWithCascade, CommentOn | ✅ Implemented |
| 4 new step kinds: CreateExtension, DropExtension (destructive), AlterExtensionUpdate, CommentOnExtension | ✅ Implemented |
| Source-`None` symmetry for schema/version/comment (unpinned = "any") | ✅ Implemented |
| `NodeId::Extension` + Extension → Schema dep edges | ✅ Implemented |
| `COMMENT ON EXTENSION` parser arm | ✅ Implemented |
| 2 new lint rules (`extension-version-unpinned`, `extension-references-unmanaged-schema`) | ✅ Implemented |
| 11 conformance fixtures (objects + scenarios + lint integration) | ✅ Implemented |

### v0.2 functions and procedures — what's in `3c3f6ee`

| Feature | Status |
|---|---|
| SQL functions (`CREATE FUNCTION … LANGUAGE sql`) — full attribute matrix | ✅ Implemented |
| PL/pgSQL functions (`CREATE FUNCTION … LANGUAGE plpgsql`) — body parsed via `pg_query::parse_plpgsql` | ✅ Implemented |
| Procedures (`CREATE PROCEDURE`) — qname-only identity | ✅ Implemented |
| Function overloads — identity = `(qname, NormalizedArgTypes)` per arch Decision 2 | ✅ Implemented |
| `CREATE OR REPLACE` for body / attribute changes | ✅ Implemented |
| `ReplaceWithCascade` fallback (return-type kind change, language change, OUT-param shape change) | ✅ Implemented |
| `NodeId::Function` + `NodeId::Procedure` + dep-graph edges (routine → schema, routine → type, routine → body deps) | ✅ Implemented |
| Static-SQL dep extraction from PL/pgSQL bodies (`SELECT INTO`, embedded queries) | ✅ Implemented |
| `-- @pgevolve dep: <qname>` directives close the dynamic-SQL gap per arch Decision 11 | ✅ Implemented |
| Auto-detected `COMMIT`/`ROLLBACK` → step runs with `transactional=OutsideTransaction` | ✅ Implemented |
| Catalog reader via `pg_proc` + `pg_get_functiondef` body re-parse (`prokind IN 'f','p'`) | ✅ Implemented |
| Unsupported-language rows surface as `DriftReport::UnmanagedLanguageRoutine`, skipped silently | ✅ Implemented |
| 3 new lint rules (`pl-pgsql-dynamic-sql`, `procedure-contains-commit`, `function-references-unmanaged-schema`) | ✅ Implemented |
| 22 conformance fixtures (functions, procedures, intent, scenarios) | ✅ Implemented |
| Property test `plpgsql_canonicalization_is_idempotent` (pure, `#[ignore]`'d) | ✅ Implemented |

### v0.2 triggers — what's in `25ca3a5`

| Feature | Status |
|---|---|
| `CREATE TRIGGER` parser — BEFORE/AFTER/INSTEAD OF, ROW/STATEMENT, WHEN clause, REFERENCING transition tables | ✅ Implemented |
| `CONSTRAINT TRIGGER` with `DEFERRABLE [INITIALLY DEFERRED\|IMMEDIATE]` | ✅ Implemented |
| `UPDATE OF column, …` column-list variant | ✅ Implemented |
| `COMMENT ON TRIGGER` parser arm | ✅ Implemented |
| Catalog reader via `pg_trigger` (filtered: `NOT tgisinternal`, NOT extension-owned) | ✅ Implemented |
| `TriggerChange` variants: Create, Drop (destructive; intent required), CommentOn | ✅ Implemented |
| Any structural diff → Drop + Create (no `ALTER TRIGGER` beyond rename, which is not supported) | ✅ Implemented |
| 3 new step kinds: `CreateTrigger`, `DropTrigger` (destructive), `CommentOnTrigger` | ✅ Implemented |
| `NodeId::Trigger` + dep-graph edges (trigger → table/view/MV target; trigger → function) | ✅ Implemented |
| 2 new lint rules (`trigger-references-unmanaged-table`, `trigger-references-unmanaged-function`) | ✅ Implemented |
| Conformance fixtures (Tier C): `objects/triggers/` covering create/drop/comment and all trigger variants | ✅ Implemented |

### v0.2 partitioning — what's in `fbff481`

| Feature | Status |
|---|---|
| `partition_by: Option<PartitionBy>` field on `Table` — strategy (Range/List/Hash) + key columns/expressions | ✅ Implemented |
| `partition_of: Option<PartitionOf>` field on `Table` — parent reference + partition bounds | ✅ Implemented |
| `ir/partition.rs` — `PartitionBy`, `PartitionOf`, `PartitionStrategy`, `PartitionColumn`, `PartitionBounds` (Range/List/Hash/Default), `BoundDatum` (Literal/MinValue/MaxValue) | ✅ Implemented |
| Source Form 1: `CREATE TABLE child PARTITION OF parent FOR VALUES …` — declarative inline form | ✅ Implemented |
| Source Form 2: standalone `CREATE TABLE child PARTITION OF parent …` in separate file | ✅ Implemented |
| Source Form 3: `ALTER TABLE parent ATTACH PARTITION child FOR VALUES …` form (Form 2 + attach) | ✅ Implemented |
| Form 2 and Form 3 unify into the same IR; equivalence verified by conformance fixture | ✅ Implemented |
| Sub-partitioning: `PARTITION BY` on a child that is itself `PARTITION OF` another parent | ✅ Implemented |
| Catalog reader — `pg_class.relkind='p'` + `pg_get_partkeydef` for parents; `relispartition + pg_get_expr(relpartbound)` for children | ✅ Implemented |
| `TableChange::AttachPartition` + `TableChange::DetachPartition` differ variants | ✅ Implemented |
| Rebound partition (bounds change) → DetachPartition + AttachPartition | ✅ Implemented |
| Parent `partition_by` rekey → `UnsupportedDiff` (no safe in-place path) | ✅ Implemented |
| Child column/constraint diff suppressed when either side is a partition | ✅ Implemented |
| 2 new step kinds: `AttachPartition`, `DetachPartition` (destructive; intent required) | ✅ Implemented |
| Dep edge: child partition → parent table (child created after parent) | ✅ Implemented |
| `partition-references-unmanaged-parent` lint rule (Error) | ✅ Implemented |
| 14 conformance fixtures: create range/list/hash parents, add/drop/replace partitions, attach-from-standalone, Form 2 vs Form 3 equivalence, sub-partitioning, reject paths, lint | ✅ Implemented |

## Workspace layout

- `crates/pgevolve-core` — IR, parser, diff, planner, plan I/O, lint
  engine. Pure library; no I/O at the type level (the parser owns the only
  filesystem walk).
- `crates/pgevolve` — CLI binary + executor. Holds the `tokio_postgres`
  adapter, advisory lock, audit writers, and the apply loop.
- `crates/pgevolve-testkit` — internal test infra: `EphemeralPostgres`
  (testcontainers wrapper), `PgCatalogQuerier`, IR generator and mutator,
  equivalence asserter, migration-fixture loader.
- `crates/pgevolve-conformance` — deterministic fixture-driven
  conformance suite: one directory per fixture, asserts diff / plan /
  plan.sql golden / apply roundtrip. New goldens via
  `cargo xtask bless --conformance`.
- `xtask` — `cargo xtask bless` regenerates tier-3 catalog goldens.

## Test tiers

| Tier | Where | Runs | Needs Docker | CI gate |
|------|-------|------|--------------|---------|
| 1 | unit tests in `src/` | `cargo test --workspace --lib` | no | yes |
| 2 | parser/IR fixture corpora | `cargo test --workspace --tests` | no | yes |
| 3 | catalog round-trip goldens | `cargo test --workspace --tests` | yes | yes |
| C | **conformance suite (`crates/pgevolve-conformance`)** | `cargo test -p pgevolve-conformance` | yes (apply layer) | **yes — canonical CI gate** |
| 5 | property tests | `cargo test --workspace --tests -- --ignored` | partial | no — nightly only |
| 7 | weekly soak | manual / cron | yes | no |

Tier C is the canonical CI gate; every deterministic correctness
expectation lives there as a fixture. Fixture authoring subtrees under
`crates/pgevolve-conformance/tests/cases/`:

| Subtree | Purpose |
|---|---|
| `objects/` | Per-object-kind DDL coverage (tables, indexes, sequences, etc.) |
| `scenarios/` | Multi-step migration scenarios (add column, reorder, FK cycle, etc.) |
| `intent/` | Destructive-intent approval and waiver flows |
| `failure/` | Error paths: bad SQL, cycle, NOT VALID drift, INVALID index |
| `regressions/` | Fixtures captured from property-test failures |

Layers L1–L9 within each fixture:

| Layer | What it asserts |
|---|---|
| L1 | Parse (no panics, canonical IR) |
| L2 | Lint (finding set) |
| L3 | Diff (ChangeSet golden) |
| L4 | Plan order (OrderedChangeSet golden) |
| L5 | Rewrite (RawStep golden) |
| L6 | Plan SQL golden (`plan.sql`) |
| L7 | Apply round-trip (apply + re-diff → empty) |
| L8 | Dep-graph golden (DOT output from `pgevolve graph`) |
| L9 | Doctor output golden |

**Backend selection** for Docker-gated layers: set
`PGEVOLVE_TEST_PG_MODE=testcontainers|compose|dsn`.
- `testcontainers` (default) — pulls `postgres:<major>-alpine`.
- `compose` — connects to a running container started by
  `dev/docker-compose.pg.yml` (faster in local dev).
- `dsn` — connects to an explicit DSN from `PGEVOLVE_TEST_PG_URL`.

Set `PGEVOLVE_DISABLE_DOCKER_TESTS=1` to skip every Docker-gated test;
the suite skips cleanly when `docker info` fails.

Regenerate tier-3 catalog goldens with `cargo xtask bless` (runs against
ephemeral containers per PG major). Regenerate tier-C conformance goldens
with `cargo xtask bless --conformance`.

## Dependencies

The workspace deliberately avoids unmaintained / archived crates. Notable
choices:

- **Parsing**: [`pg_query`](https://crates.io/crates/pg_query) (official
  Postgres parser bindings).
- **Hashing**: [`blake3`](https://crates.io/crates/blake3) — plan id,
  target identity, and IR canonical hashing.
- **Encoding**: bincode for the plan-id input hash payload (binary &
  deterministic), serde_json for the embedded catalog snapshot in
  `manifest.toml`. **`serde_yaml` is deliberately not used** (upstream
  archived it in 2024).
- **CLI**: clap v4 derive.
- **Async runtime**: tokio multi-thread; tokio-postgres for the executor.
- **Property tests**: proptest v1, with `EphemeralPostgres` from
  testcontainers for tier-4/5.

## License

Dual-licensed under MIT or Apache-2.0, at your option.

### License + dependency policy

pgevolve is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
All dependencies must be MIT/Apache-2.0/BSD-style licensed (no copyleft); the
policy is enforced by `cargo deny check` (see [`deny.toml`](deny.toml)) and verified
in CI. Only permissive licenses are allowed: MIT, Apache-2.0, BSD-2-Clause,
BSD-3-Clause, ISC, Unicode-DFS-2016, Unicode-3.0, CC0-1.0, and Zlib. Vulnerable
and yanked crates are denied; the check enforces that all dependencies come
from crates.io.
