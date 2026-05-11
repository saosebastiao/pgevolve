# Testing

pgevolve's test surface is structured in seven tiers. Each tier catches
a different class of bug; together they form the gate for releases.

See [`../README.md`](./README.md) for the status legend.

## Tier matrix

| Tier | What it catches | Where it lives | Status | Needs Docker |
|------|-----------------|----------------|--------|--------------|
| 1 | Unit-level invariants — IR equality, parser output for one statement, single function behavior | Inline `#[cfg(test)] mod tests` in every src/ file | ✅ Implemented | no |
| 2 | Fixture corpora — parsing a real `*.sql` snippet, comparing IR vs expected | `crates/pgevolve-core/tests/parser_corpus.rs`, `crates/pgevolve-core/tests/parse_directory.rs` | ✅ Implemented | no |
| 3 | Catalog round-trip goldens — apply known SQL, introspect, snapshot to canonical JSON | `crates/pgevolve-core/tests/catalog_round_trip.rs` per PG major (14/15/16/17) | ✅ Implemented | yes |
| 4 | Executor + CLI end-to-end — apply a plan against real PG, assert side effects + audit rows | `crates/pgevolve/tests/{executor_smoke,cli_e2e,chaos_apply,shadow_validate}.rs` | ✅ Implemented | yes |
| 5 | Property tests — random valid `Catalog`s exercised across the pipeline | `crates/pgevolve-core/tests/property_tests.rs` (pure) and `crates/pgevolve/tests/pg_property_tests.rs` (PG-bound) | ✅ Implemented | partial |
| 6 | Mutation tests — flip code, verify a test fails | not implemented | 🔮 Future | n/a |
| 7 | Soak — high-case property runs over multiple PG versions, weekly | `.github/workflows/soak.yml` | ✅ Implemented | yes |

## What each property test checks (Tier 5)

| Property | Status | Notes |
|---|---|---|
| `plan_id_is_deterministic` | ✅ Implemented | Two `PlanId::compute(source, target, ver, ruleset)` invocations return the same bytes; different ruleset returns different bytes. Pure; no Docker. |
| `create_graph_topo_sorts_or_only_fk_cycles` | ✅ Implemented | Either the create-graph topologically sorts cleanly, or every node in the cycle is an FK-bound `Table` / `Constraint`. Pure. |
| `round_trip_property` | ✅ Implemented | Apply a random catalog to PG; re-introspect; assert structural equality. Needs Docker. |
| `idempotency_property` | ✅ Implemented | Diff of (applied catalog, applied catalog) is empty. |
| `end_to_end_equivalence_property` | ✅ Implemented | Apply initial; apply a random mutation; introspect; assert equal to mutated. Exercises `IRMutator`. |
| `drift_recovery_property` | ✅ Implemented | Apply with `abort_after_step = N`; re-plan from partial state; apply to completion; assert equal to target. |

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
| `soak.yml` — manual `workflow_dispatch` + weekly cron at `PROPTEST_CASES=5000` per PG major | ✅ Implemented | |
| Mutation testing (cargo-mutants, Stryker-style) | 🔮 Future | Once the spec stabilizes; mutation testing flags rules that aren't actually tested. |
| Code coverage badges | 🔮 Future | `cargo-llvm-cov` is the obvious tool. |
