# Changelog

All notable changes to pgevolve are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] — Unreleased

Extends the v0.1 surface with **views, materialized views, and user-defined types** as fully-managed objects. The differ, planner, linter, conformance suite, and property tests all cover the new object kinds.

### Added — IR (user-defined types)

- `UserType { qname, kind: UserTypeKind, comment }` flat IR type in `pgevolve-core::ir::user_type`.
- `UserTypeKind::Enum { values: Vec<EnumValue> }` — ordered label list with `sort_order: f32` mirroring `pg_enum.enumsortorder`.
- `UserTypeKind::Domain { base, nullable, default, check_constraints, collation }` — domain defaults and CHECK expressions use `NormalizedExpr` for canonical comparison.
- `UserTypeKind::Composite { attributes: Vec<CompositeAttribute> }` — each attribute carries name, type, and optional collation.
- `Catalog::types: Vec<UserType>` — flat collection, sorted by `qname` after `canonicalize()`.

### Added — pipeline (user-defined types)

- **Source parser** — `CREATE TYPE … AS ENUM`, `CREATE DOMAIN`, `CREATE TYPE … AS (…)` all parse into the `UserType` IR. Duplicate labels / attributes rejected at parse time.
- **AST resolution** — `UserDefined(QualifiedName)` column type references resolved against `Catalog::types` after the source parse pass.
- **Catalog reader** — queries `pg_type`, `pg_enum`, `pg_attribute` (for composites), and `pg_constraint` / `pg_attrdef` (for domains) to reconstruct `UserType` from a live database.
- **Differ** — `UserTypeChange` variants: `Create`, `Drop`, `EnumAddValue`, `EnumRenameValue`, `DomainAddCheck`, `DomainDropCheck`, `DomainSetDefault`, `DomainSetNotNull`, `CompositeAddAttribute`, `CompositeDropAttribute`, `CompositeAlterAttributeType`, `CommentOn`, `ReplaceWithCascade`.
- **Compatibility predicates** — `enum_can_alter_in_place` (preserved labels maintain relative order; renames position-paired) and `composite_can_alter_in_place` (preserved attributes maintain relative order). Both fall back to `ReplaceWithCascade` when the predicate returns `false`.
- **Planner** — 12 new step kinds: `CreateType`, `DropType`, `AlterTypeAddValue`, `AlterTypeRenameValue`, `AlterDomainAddConstraint`, `AlterDomainDropConstraint`, `AlterDomainSetDefault`, `AlterDomainSetNotNull`, `AlterTypeAddAttribute`, `AlterTypeDropAttribute`, `AlterTypeAlterAttributeType`, `CommentOnType`.
- **`NodeId::Type`** — added to the dep graph; edges from type → column (column's `ColumnType::UserDefined`) and type → type (domain base type) drive correct creation/drop ordering.

### Added — lint rules (user-defined types)

- `type-shadows-table` (Error) — a user-defined type shares a qualified name with a table, view, or MV.
- `enum-value-collision` (Error) — an enum type declares duplicate value labels.
- `composite-attribute-collision` (Error) — a composite type declares duplicate attribute names.
- `domain-check-references-unmanaged-type` (Warning) — a domain's CHECK expression references a schema outside `[managed].schemas`.

### Added — tests (user-defined types)

- **20 conformance fixtures** (Tier C): `objects/enums/` (8), `objects/domains/` (6), `objects/composites/` (4), `objects/user_type_lints/` (2).
- **Property test** `enum_add_value_preserves_existing_values` (`#[ignore]`, pure, no Docker) — for any random initial label list and a new distinct label, `diff_user_types` emits exactly one `EnumAddValue` change.

### Added — IR (views and materialized views)

- `View` and `MaterializedView` flat IR types in `pgevolve-core::ir::view`.
- `ViewColumn` — named column with resolved type and optional comment; used by both views and MVs.
- `body_canonical: NormalizedBody` — parsed-and-deparsed SELECT body in canonical form. Enables cosmetically-different but semantically-identical view bodies to diff equal.
- `body_dependencies: Vec<DepEdge>` — dependency edges extracted from the body AST with `DepSource::AstExtracted` provenance. Powers the dependent-recreation walk and the `view-body-references-unmanaged-schema` lint.
- `security_barrier` and `security_invoker` reloptions on `View`.

### Added — pipeline (views and materialized views)

- **AST canonicalization pass** (`parse/ast_canon.rs`) — runs after source parse; calls `NormalizedBody::from_sql` on each view body, extracts `DepEdge`s, resolves references against the provisional catalog, and fills in column types.
- **Catalog reader** — `read_views` and `read_materialized_views` query `pg_views` / `pg_matviews`, call `pg_get_viewdef`, and feed the result through `NormalizedBody::from_sql`. Source-side and catalog-side canonical texts are directly comparable.
- **Differ** — `ViewChange` and `MvChange` variants. OR-REPLACE compatibility predicate (`body_is_or_replace_compatible`) determines whether a body change emits `CREATE OR REPLACE VIEW` (compatible) or `DROP + CREATE` (incompatible).
- **Planner** — 7 new step kinds: `CreateView`, `DropView`, `CreateMaterializedView`, `DropMaterializedView`, `RefreshMaterializedView`, `AlterViewSetReloption`, `CommentOnView`.
- **Online rewrites** — `REFRESH MATERIALIZED VIEW CONCURRENTLY` upgrade (when unique index present); dependent-view recreation cascade (`recreate_views::extend_with_dependent_recreations`).

### Added — configuration

- `[planner.online_rewrites].refresh_mv_concurrently` (default `true`) — upgrade `REFRESH` to `REFRESH CONCURRENTLY` when the MV has a unique index.
- `[planner.online_rewrites].view_drop_create_dependents` (default `true`) — cascade dependent-view recreations; set `false` to error instead of auto-cascading.
- `[[step_override]]` rows in `intent.toml` — suppress individual plan steps by kind + target.

### Added — lint rules (views and materialized views)

- `view-shadows-table` (Error) — a view or MV shares a qualified name with a managed table.
- `mv-no-unique-index` (Warning) — an MV has no unique index; `REFRESH CONCURRENTLY` unavailable.
- `view-body-references-unmanaged-schema` (Warning) — a view body dependency edge points to an unmanaged schema.

### Added — tests (views and materialized views)

- **15 conformance fixtures** (Tier C): `objects/views/` (8), `objects/materialized_views/` (6), `intent/drop-view-requires-intent` (1), `scenarios/dependency-chains/` (2).
- **Property test** `view_canonicalization_closed_under_pg_rewrite` (`#[ignore]`, Docker-gated) — verifies `NormalizedBody::from_sql` closure under the PG rewrite for a fixed set of representative view bodies.

### Deferred

- `arb_view_dependency_graph` proptest (spec §12 step 12.2) — deferred post-v0.2; requires substantial generator engineering and is not load-bearing for the closure invariant.

## [0.1.0] — 2026-05-17

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

[Unreleased]: https://github.com/saosebastiao/pgevolve/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/saosebastiao/pgevolve/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/saosebastiao/pgevolve/releases/tag/v0.1.0
