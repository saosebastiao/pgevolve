# Changelog

All notable changes to pgevolve are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [0.1.0] — Unreleased

First tagged release. The v0.1 surface manages **schemas, tables (with
columns/constraints/comments), indexes, and sequences** against Postgres
14, 15, 16, and 17.

### Added — pipeline

- **Parser** (`pgevolve-core::parse`) — `*.sql` → IR via `pg_query`. Tracks
  source locations for every parsed object. Recognises `-- @pgevolve`
  directives (`schema=…`, `dep:…`).
- **AST resolution pass** — runs between parse and canonicalize.
  Validates structural references (FKs against declared tables;
  default-using sequences against declared sequences). Surfaces
  unresolved references with source-located errors before any DB
  touch.
- **Catalog reader** (`pgevolve-core::catalog`) — live PG → IR via
  per-PG-major SQL strings and a sync `CatalogQuerier` trait. Returns
  `(Catalog, DriftReport)`. The drift report captures NOT VALID
  constraints and INVALID indexes for auto-recovery.
- **Differ** (`pgevolve-core::diff`) — pair-by-qname, structural;
  `ChangeSet` plus higher-level `Change` enum. Drift entries fold into
  `Change::ValidateConstraint` / `Change::RecreateIndex`.
- **Planner** (`pgevolve-core::plan`) — order → rewrite → group → wrap.
  Deterministic topo sort (Kahn + min-heap tiebreak). FK cycle
  extraction via `DeferredFkAdd`. Four online rewrites:
  `CREATE INDEX CONCURRENTLY`, `FK NOT VALID + VALIDATE`, `CHECK NOT
  VALID + VALIDATE`, `SET NOT NULL via CHECK pattern`.
- **Plan format** — three-file directory (`plan.sql`, `intent.toml`,
  `manifest.toml`); deterministic `PlanId` (BLAKE3 over bincoded
  canonical IRs); `[[intent]]` rows with `approved: bool`;
  `[[lint_waiver]]` rows to acknowledge `LintAtPlan` findings;
  `RecordedFinding` rows in `manifest.toml` for apply-time waiver
  recheck.
- **Executor** (`pgevolve::executor`) — bootstrap, advisory lock,
  per-step audit, preflight (target identity, drift recheck, intent
  approval, lint-waiver recheck). Per-group transactional or
  autocommit execution.
- **Linter** (`pgevolve-core::lint`) — universal rules + four built-in
  layout profiles (`schema-mirror`, `kind-grouped`, `feature-grouped`,
  `free-form`) plus a regex+assertion custom-profile mechanism. New
  `Severity::LintAtPlan` tier (gates plan with exit code 2 unless
  waived) and a new `column-position-drift` rule.

### Added — IR

- Top-level types: `Catalog`, `Schema`, `Table`, `Column`,
  `Constraint`, `Index`, `Sequence`, plus `ColumnType`, `DefaultExpr`,
  `NormalizedExpr`, `NormalizedBody` (the statement-scope counterpart
  for v0.2 body-bearing objects).
- Dep-graph types: `DepEdge { from, to, source: DepSource }` with
  `Structural` (v0.1) + `AstExtracted` / `AstDeclared` (v0.2)
  provenance.

### Added — CLI

- `pgevolve init` — scaffold project files.
- `pgevolve lint [--format human|json]` — universal + layout-profile
  rules.
- `pgevolve validate [--shadow] [--shadow-validate] [--shadow-strict]`
  — source-tree validation.
- `pgevolve diff --db <env> [--format human|json|sql]
  [--shadow-validate]` — print the change set.
- `pgevolve plan --db <env> [-o <dir>] [--shadow-validate]` — write a
  plan directory. Refuses with exit 2 on unwaived `LintAtPlan`
  findings.
- `pgevolve apply <plan-dir> --db <env>` — execute a plan.
- `pgevolve status --db <env>` — recent applies and per-step state.
- `pgevolve dump --db <env> -o <dir>` — introspect a live DB and write
  a fully-populated `schema/` tree via the new IR → SQL emitter
  (`pgevolve-core::render`).
- `pgevolve bootstrap --db <env>` — install/upgrade the metadata
  schema.
- `pgevolve graph [--graph-format dot|mermaid] [-o <path>]` — render
  the dep graph.
- `pgevolve doctor --db <env>` — project health check (drift, dangling
  intents, recent apply failures).
- `pgevolve rewrite-table <qname> --db <env> --confirm-rewrite` —
  skeleton; full implementation lands with a v0.2 sub-spec.

### Added — config

- `pgevolve.toml` with `[project]`, `[managed]`, `[planner]`,
  `[planner.online_rewrites]`, `[environments.<env>]`, and a new
  `[shadow]` block (`backend = auto | testcontainers | dsn`; per-backend
  `url`, `url_env`, `reset`, `extensions`, `postgres_version`).

### Added — test infrastructure

- `pgevolve-testkit` — `EphemeralPostgres`, `PgCatalogQuerier`,
  `MigrationFixture`, IR generator + mutator, `TestPgBackend`
  pluggable backend trait with testcontainers / compose / dsn impls
  (selected via `PGEVOLVE_TEST_PG_MODE`).
- `pgevolve-conformance` — Tier C suite with five fixture authoring
  subtrees (`objects/`, `scenarios/`, `intent/`, `failure/`,
  `regressions/`) and nine assertion layers (L1 diff, L2 plan
  structural, L3 plan-SQL golden, L4 apply roundtrip, L5 minimality,
  L6 no-collateral-damage, L7 intent shape, L8 dep-graph golden, L9
  topological order). Runtime budgets enforced per-fixture and
  suite-total.
- `dev/docker-compose.pg.yml` — PG 14/15/16/17 on stable ports for
  fast local test iteration in compose mode.
- `cargo xtask` subcommands: `bless --conformance`,
  `coverage --check|--gaps`, `fixture-cost`, `capture-regression`,
  `verify-regression`, `property-status`, `diagnose-pg-version`.

### Added — workflows

- `ci.yml` — fmt, clippy, unit + tier-2 tests, conformance matrix
  across PG 14/15/16/17, property-status compliance gate.
- `property-tests.yml` — nightly property test runs with
  auto-capture of failures into `regressions/`.
- `soak.yml` — weekly high-case property runs.

### Known limitations of v0.1

- `pgevolve rewrite-table` is a CLI skeleton — invoking it errors with
  "not yet implemented." The implementation lands with a v0.2 sub-spec
  (partitioning / column-type-change).
- `pgevolve dump` writes a single `schema.sql` file. Multi-file layout
  following `[project].layout_profile` is deferred to v0.1.x+.
- Views, materialized views, functions, procedures, triggers,
  user-defined types, extensions, and declarative partitioning are
  **not** in v0.1; they land per v0.2 sub-spec series.
- `--shadow-validate` is a scaffold cross-check. v0.1 has no body-
  bearing objects so the cross-check has nothing to do beyond a
  trivial structural-edge count; v0.2 sub-specs deepen it.

[Unreleased]: https://github.com/saosebastiao/pgevolve/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/saosebastiao/pgevolve/releases/tag/v0.1.0
