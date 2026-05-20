# Testing

pgevolve's test surface is structured in seven tiers. Each tier catches
a different class of bug; together they form the gate for releases.

See [`../README.md`](./README.md) for the status legend.

## Tier matrix

| Tier | What it catches | Where it lives | Status | Needs Docker |
|------|-----------------|----------------|--------|--------------|
| 1 | Unit-level invariants — IR equality, parser output for one statement, single function behavior | Inline `#[cfg(test)] mod tests` in every src/ file | ✅ Implemented | no |
| 2 | Fixture corpora — parsing a real `*.sql` snippet, comparing IR vs expected | `crates/pgevolve-core/tests/parser_corpus.rs`, `crates/pgevolve-core/tests/parse_directory.rs` | ✅ Implemented | no |
| 3 | Conformance — fixture-driven regression gate (L1–L9); see below | `crates/pgevolve-conformance/` | ✅ Implemented | yes |
| 4 | Executor + CLI end-to-end — apply a plan against real PG, assert side effects + audit rows | `crates/pgevolve/tests/{executor_smoke,cli_e2e,chaos_apply,shadow_validate}.rs` | ✅ Implemented | yes |
| 5 | Property tests — random valid `Catalog`s exercised across the pipeline | `crates/pgevolve-core/tests/property_tests.rs` (pure) and `crates/pgevolve/tests/pg_property_tests.rs` (PG-bound) | ✅ Implemented | partial |
| 6 | Mutation tests — flip code, verify a test fails | not implemented | 🔮 Future | n/a |
| 7 | Soak — high-case property runs over multiple PG versions, weekly | `.github/workflows/soak.yml` | ✅ Implemented | yes |

## Tier C (conformance) assertion layers

Tier C is the canonical regression gate. Each fixture drives the full pipeline; assertion layers are evaluated in order. Layers L1–L4 shipped with v0.1; L5–L9 landed in v0.2 readiness.

| Layer | Name | Status | Notes |
|---|---|---|---|
| L1 | parse | ✅ Implemented | Source parses cleanly. |
| L2 | lint | ✅ Implemented | No lint errors. |
| L3 | plan | ✅ Implemented | Plan produces expected steps. |
| L4 | apply | ✅ Implemented | Plan applies cleanly against real PG. Runs in-process via `pgevolve::api::build_plan` + `pgevolve::executor::apply_plan` — no subprocesses, no per-fixture binary rebuild. |
| L5 | minimality | ✅ Implemented | Re-plan after L4 apply asserts empty diff and empty plan groups. |
| L6 | no-collateral-damage | ✅ Implemented | Opt-in `touches_only` allow-list; asserts no unlisted objects were modified. |
| L7 | intent-shape | ✅ Implemented | Mandatory-on-destructive; matches `[[expect.intent]]` against the generated `intent.toml`. |
| L8 | dep-graph golden | ✅ Implemented | Byte-compares rendered DOT against `expected/dep-graph.dot`. |
| L9 | topological-order | ✅ Implemented | Declared partial orders respected by step sequence. |

Full details and authoring contract: `crates/pgevolve-conformance/AUTHORING.md`.

## Fixture authoring subtrees

Each subtree under `crates/pgevolve-conformance/fixtures/` has a specific contract:

| Subtree | Contract |
|---|---|
| `objects/` | One fixture per object kind / change kind combination. Exercises L1–L5 at minimum. |
| `scenarios/` | Multi-object, multi-step scenarios (e.g., rename dance, online rewrite sequences). |
| `intent/` | Destructive-change fixtures; must include `[[expect.intent]]` blocks (L7). |
| `failure/` | Fixtures that must fail at a specific phase (parse error, lint error, plan error). Uses `[expect.failure]`. |
| `regressions/` | Scaffolded by `cargo xtask capture-regression`; each fixture is linked to a GitHub issue. |

### v0.2 view / MV fixture coverage (Tier C)

As of T11, there are 15 conformance fixtures covering views and materialized views:

| Subtree | Fixtures |
|---|---|
| `objects/views/` | 8 fixtures: `create-simple`, `create-with-aliases`, `comment-on-view`, `drop`, `replace-body-compatible`, `replace-body-incompatible`, `security-barrier-toggle`, `security-invoker-toggle` |
| `objects/materialized_views/` | 6 fixtures: `create-simple`, `create-no-unique-index-online`, `index-on-mv`, `refresh-concurrently`, `replace-body`, `with-no-data-override` |
| `intent/` | 1 fixture: `drop-view-requires-intent` |
| `scenarios/dependency-chains/` | 2 fixtures covering transitive view recreation |

Total: 15 fixtures across `objects/views/`, `objects/materialized_views/`, `intent/`, and `scenarios/dependency-chains/`.

## `fixture.toml` schema additions

| Key | Status | Notes |
|---|---|---|
| `[budget].seconds` | ✅ Implemented | Per-fixture time budget; exceeded fixtures fail in CI. |
| `[pg.expect].<major>` | ✅ Implemented | Per-PG-major expected output overrides. |
| `[expect.plan.per_pg.pgN]` | ✅ Implemented | Override plan expectations for a specific PG major. |
| `[[expect.intent]]` | ✅ Implemented | L7 intent-shape matching rows. |
| `[expect.dep_graph]` | ✅ Implemented | L8 dep-graph golden reference. |
| `[expect.failure]` | ✅ Implemented | Phase and message for expected-failure fixtures. |
| `expect.plan.touches_only` | ✅ Implemented | L6 collateral-damage allow-list. |
| `expect.plan.order` | ✅ Implemented | L9 partial-order declarations. |
| `expect.plan.minimality` | ✅ Implemented | L5 minimality assertion toggle (on by default). |

Full schema in `crates/pgevolve-conformance/AUTHORING.md`.

## `TestPgBackend` pluggability

| Mechanism | Status | Notes |
|---|---|---|
| `PGEVOLVE_TEST_PG_MODE` env var | ✅ Implemented | Selects backend: `testcontainers` (default), `compose`, or `dsn`. |
| `dev/docker-compose.pg.yml` | ✅ Implemented | Ships for `compose` mode; pre-warms containers across test runs. |

## xtask additions

| Task | Status | Notes |
|---|---|---|
| `cargo xtask coverage --check \| --gaps` | ✅ Implemented | (capability × change-kind × major) coverage matrix gate. `--check` fails if any cell is uncovered; `--gaps` prints the gap report. |
| `cargo xtask fixture-cost` | ✅ Implemented | Per-fixture timing report; helps identify slow fixtures. |
| `cargo xtask capture-regression --seed <hex> --issue <n>` | ✅ Implemented | Scaffold a regression fixture linked to a GitHub issue. |
| `cargo xtask verify-regression <fixture-dir>` | ✅ Implemented | Confirm a regression fixture exercises a real bug (fails without the fix). |
| `cargo xtask property-status --max-age-days N` | ✅ Implemented | Open-issue compliance gate; uses `gh` to check issue status. |
| `cargo xtask diagnose-pg-version <fixture-dir> --pg-major N` | ✅ Implemented | Per-PG-major fixture diagnostic for version-specific failures. |

## What each property test checks (Tier 5)

| Property | Status | Notes |
|---|---|---|
| `plan_id_is_deterministic` | ✅ Implemented | Two `PlanId::compute(source, target, ver, ruleset)` invocations return the same bytes; different ruleset returns different bytes. Pure; no Docker. |
| `create_graph_topo_sorts_or_only_fk_cycles` | ✅ Implemented | Either the create-graph topologically sorts cleanly, or every node in the cycle is an FK-bound `Table` / `Constraint`. Pure. |
| `view_canonicalization_closed_under_pg_rewrite` | ✅ Implemented | v0.2. For a fixed set of representative view bodies: create the view in an ephemeral PG, query `pg_get_viewdef`, canonicalize both source and catalog bodies via `NormalizedBody::from_sql`, assert the canonical texts match. Docker-gated; `#[ignore]`'d. A divergence indicates a canonicalization bug. |
| `round_trip_property` | ✅ Implemented | Apply a random catalog to PG; re-introspect; assert structural equality. Needs Docker. |
| `idempotency_property` | ✅ Implemented | Diff of (applied catalog, applied catalog) is empty. |
| `end_to_end_equivalence_property` | ✅ Implemented | Apply initial; apply a random mutation; introspect; assert equal to mutated. Exercises `IRMutator`. |
| `drift_recovery_property` | ✅ Implemented | Apply with `abort_after_step = N`; re-plan from partial state; apply to completion; assert equal to target. |
| `arb_view_dependency_graph` | 🔮 Deferred | Spec §12 step 12.2. Requires a non-trivial proptest generator for arbitrary view dep graphs. Not load-bearing for the closure invariant; deferred post-v0.2. |

`PGEVOLVE_PROPERTY_CASES` controls the Docker-bound test case count
(default 3 locally; CI's `pg-matrix` uses 50; soak uses 5000).

## What lives in pgevolve-testkit

| Module | Status | Notes |
|---|---|---|
| `EphemeralPostgres` (testcontainers wrapper, PG 14-17) | ✅ Implemented | The binary has a separate `ShadowPostgres` for `validate --shadow`. |
| `PgCatalogQuerier` (`tokio_postgres`-backed `CatalogQuerier`) | ✅ Implemented | Mirrored in the binary as `pgevolve::pg_querier`. |
| `catalog_snapshotter` (canonical JSON renderer) | ✅ Implemented | Powers tier-3 goldens. |
| `MigrationFixture` (tier-4 fixture loader) | ✅ Implemented | One seed fixture under `crates/pgevolve-core/tests/fixtures/e2e/`. |
| `arbitrary_catalog` IR generator | ✅ Implemented | Schemas + tables (with bigint PK) + indexes + sequences + a curated set of column types. Richer coverage (FKs, CHECK, multi-column UNIQUE, generated columns) lands in v0.2. |
| `arbitrary_mutation` IR mutator | ✅ Implemented | Nine mutation kinds with cascade semantics. |
| `assert_canonical_eq` | ✅ Implemented | Wraps `Catalog::diff`; renders failures as an indented diff list. |

## CI / soak

| Workflow | Status | Notes |
|---|---|---|
| `ci.yml` — fmt + clippy + tier 1+2 (no Docker) on every push / PR | ✅ Implemented | |
| `ci.yml` — `pg-matrix` job runs tier 3-5 on PG 14/15/16/17 with `PROPTEST_CASES=50` | ✅ Implemented | Needs Docker on the runner. |
| `ci.yml` — auto-capture on flaky property failure + `property-status` compliance gate | ✅ Implemented | Uses `cargo xtask capture-regression` and `cargo xtask property-status`. |
| `property-tests.yml` — dedicated property-test workflow with coverage matrix gate | ✅ Implemented | Runs `cargo xtask coverage --check` on every push to main and on PRs. |
| `soak.yml` — manual `workflow_dispatch` + weekly cron at `PROPTEST_CASES=5000` per PG major | ✅ Implemented | |
| Mutation testing (cargo-mutants, Stryker-style) | 🔮 Future | Once the spec stabilizes; mutation testing flags rules that aren't actually tested. |
| Code coverage badges | 🔮 Future | `cargo-llvm-cov` is the obvious tool. |
