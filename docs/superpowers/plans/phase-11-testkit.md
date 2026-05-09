# Phase 11 — Testkit, generators, and Tier-5/6/7 tests

**Goal:** Build out `pgevolve-testkit` to its full v0.1 surface — IR generators, mutators, equivalence asserters, end-to-end harnesses, and a chaos harness — and use them to land Tier-5 (property-based) and Tier-7 (soak) tests. Light up the CI multi-version matrix.

**Spec coverage:** §14 (Tiers 5–7), `pgevolve-testkit` public surface block, §15.

**Depends on:** Phases 1, 2, 3, 4, 5, 6, 7, 8.

**Exit criteria:**

- `pgevolve-testkit` exports the full surface listed in spec §14.
- A property test passes the six properties from spec §14 Tier 5 across PG 14–17.
- The CI workflow's PG matrix is uncommented and green.
- A soak job is configured (manual trigger via `workflow_dispatch`); not run on every PR but available on demand.
- `IRGenerator` produces enough variety that on a 1000-iteration property test we see ≥ 5 distinct constraint kinds, ≥ 3 distinct index methods, ≥ 10 distinct column types.

---

## File structure

```
crates/pgevolve-testkit/src/
├── lib.rs                     # public re-exports
├── ephemeral_pg.rs            # already exists from phase 3
├── catalog_snapshotter.rs     # already exists from phase 3
├── migration_fixture.rs       # already exists from phase 8
├── apply_harness.rs           # already exists from phase 8
├── ir_generator.rs            # new: proptest strategy for random valid Catalog
├── ir_mutator.rs              # new: proptest strategy for random valid mutations
├── equivalence_asserter.rs    # new: produces human-readable IR diffs on failure
└── chaos_apply_harness.rs     # new: SIGKILL-mid-apply harness

crates/pgevolve-core/tests/
└── property_tests.rs          # Tier-5 entry point
```

---

### Task 11.1: `IRGenerator` — random valid `Catalog`

**File:** `crates/pgevolve-testkit/src/ir_generator.rs`

```rust
pub struct IRGeneratorConfig {
    pub schema_count_range:  (usize, usize),  // (min, max), inclusive
    pub tables_per_schema_range: (usize, usize),
    pub columns_per_table_range: (usize, usize),
    pub fk_density: f64,        // 0.0..=1.0 — probability a column is an FK
    pub index_per_table_range: (usize, usize),
    pub include_user_defined_types: bool, // false for v0.1 (UDTs are OOS for diff)
}

pub fn arbitrary_catalog(cfg: IRGeneratorConfig) -> impl Strategy<Value = Catalog>;

pub fn arbitrary_table(schemas: &[Identifier], cfg: &IRGeneratorConfig) -> impl Strategy<Value = Table>;
pub fn arbitrary_column_type(cfg: &IRGeneratorConfig) -> impl Strategy<Value = ColumnType>;
pub fn arbitrary_default_expr(ty: &ColumnType) -> impl Strategy<Value = DefaultExpr>;
// ... and so on for Index, Constraint, Sequence
```

Critical invariants the generator must preserve:
- FK references must point to a column set with a unique constraint (PK or UNIQUE) on the target.
- Generated columns can reference only earlier columns in the same table.
- Index column lists reference only columns that exist.
- Sequence `data_type` must be one of `SmallInt`, `Integer`, `BigInt`.
- Composite UNIQUE constraint columns exist.

**Strategy:** generate in dependency order — schemas first, then tables (with PKs), then build a pool of "FK-able columns", then add FK constraints / indexes referring to them.

Tests:
- Generator produces 100 catalogs without panic.
- Distribution check: across 1000 catalogs, every `ColumnType` v0.1-supported variant appears at least once.

Commit: `feat(testkit): IRGenerator proptest strategy producing valid Catalogs`

---

### Task 11.2: `IRMutator` — random valid mutation of an existing `Catalog`

**File:** `crates/pgevolve-testkit/src/ir_mutator.rs`

```rust
pub fn arbitrary_mutation(catalog: &Catalog, cfg: &IRGeneratorConfig) -> impl Strategy<Value = Catalog>;
```

Possible mutations (each generated with weighted probability):
- Add a column to a random table (with random type + nullability + default).
- Drop a non-PK column.
- Rename a column → drop+add (since v0.1 doesn't detect renames).
- Change a column's type to a compatible (widening) type.
- Toggle nullable.
- Change/remove a column default.
- Add an index.
- Drop a non-PK index.
- Add a constraint (FK to a random unique-keyed column elsewhere; CHECK on a random predicate).
- Drop a constraint.
- Add a table.
- Drop a table (cascade-drop its dependents in the IR).
- Add a schema.
- Drop a schema (cascade-drop everything in it).

The mutator's main job: produce a *valid* output IR. After mutation, validate via `Catalog::canonicalize()` + an internal consistency check; if validation fails, regenerate.

Tests: generate base + mutate 1000 times → all outputs validate.

Commit: `feat(testkit): IRMutator producing valid mutations`

---

### Task 11.3: `EquivalenceAsserter`

**File:** `crates/pgevolve-testkit/src/equivalence_asserter.rs`

```rust
pub fn assert_canonical_eq(a: &Catalog, b: &Catalog) -> anyhow::Result<()> {
    let diffs = a.diff(b);
    if diffs.is_empty() { return Ok(()); }
    let rendered = render_diffs(&diffs);
    Err(anyhow::anyhow!("catalogs differ:\n{rendered}"))
}

fn render_diffs(diffs: &[Difference]) -> String { ... }
```

Renders differences as a human-readable indented tree.

Commit: `feat(testkit): EquivalenceAsserter with rich diff rendering`

---

### Task 11.4: Tier-5 property tests

**File:** `crates/pgevolve-core/tests/property_tests.rs`

Use `proptest`. Each property runs against a `EphemeralPostgres`:

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn round_trip(catalog in arbitrary_catalog(default_config())) {
        // 1. apply(catalog) to fresh PG.
        // 2. introspect → catalog2.
        // 3. assert_canonical_eq(&catalog, &catalog2).
    }

    #[test]
    fn end_to_end_equivalence(
        initial in arbitrary_catalog(default_config()),
        final_  in arbitrary_catalog_evolution(initial.clone()),
    ) {
        // Apply initial; plan(final); apply; assert state == final.
    }

    #[test]
    fn idempotency(catalog in arbitrary_catalog(default_config())) {
        // Apply once; apply same plan a second time → second apply is a no-op (empty plan).
    }

    #[test]
    fn dag_invariant(catalog in arbitrary_catalog(default_config())) {
        let g = build_create_graph(&catalog);
        prop_assert!(g.topological_sort().is_ok() || only_fk_cycles(&g));
    }

    #[test]
    fn determinism(catalog in arbitrary_catalog(default_config())) {
        let plan1 = make_plan(&catalog);
        let plan2 = make_plan(&catalog);
        prop_assert_eq!(plan1.id, plan2.id);
        // Plus: serialize both to bytes; assert byte-equal.
    }
}
```

Drift-recovery property (kill-mid-apply) lives in the next task.

Tests need Docker — gated. Run only when `docker_available()`.

Commit: `test(core): tier-5 property tests for round-trip, e2e, idempotency, DAG, determinism`

---

### Task 11.5: `ChaosApplyHarness` — SIGKILL mid-apply, then re-plan and continue

**File:** `crates/pgevolve-testkit/src/chaos_apply_harness.rs`

```rust
pub struct ChaosApplyHarness;

impl ChaosApplyHarness {
    pub async fn run(initial: &Catalog, final_: &Catalog, version: PgVersion, kill_at_step: u32) -> anyhow::Result<()> {
        let pg = EphemeralPostgres::start(version).await?;
        // 1. Apply initial.
        plan_and_apply(&pg, initial).await?;
        // 2. Spawn a child process that runs `pgevolve apply` against the final plan.
        //    Use a wrapper binary that, after applying step N, raises SIGKILL on itself.
        let plan_dir = make_plan(&pg, initial, final_).await?;
        let child = std::process::Command::new("pgevolve")
            .args(["apply", plan_dir.path().to_str().unwrap(),
                   "--db-url", pg.dsn(), "--kill-after-step",
                   &kill_at_step.to_string()])
            .spawn()?;
        let _ = child.wait_with_output()?;  // expect non-zero
        // 3. Re-plan and re-apply.
        let plan_dir2 = make_plan(&pg, &introspect(&pg).await?, final_).await?;
        run_apply(&pg, plan_dir2).await?;
        // 4. Final state matches `final_`.
        let live = introspect(&pg).await?;
        assert_canonical_eq(final_, &live)?;
        Ok(())
    }
}
```

For this to work, the binary needs a `--kill-after-step <n>` debug flag (gated behind a `cfg(feature = "chaos-testing")` feature on `pgevolve`). Add the feature.

Property test:

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]   // chaos tests are slow

    #[test]
    fn drift_recovery(
        initial in arbitrary_catalog(default_config()),
        final_  in arbitrary_catalog_evolution(initial.clone()),
        kill_step in 1u32..=10u32,
    ) {
        ChaosApplyHarness::run(&initial, &final_, PgVersion::Pg16, kill_step).unwrap();
    }
}
```

Commit: `test(testkit): ChaosApplyHarness with SIGKILL-mid-apply property test`

---

### Task 11.6: Light up CI PG matrix

**File:** `.github/workflows/ci.yml`

Uncomment the `pg-matrix` job from phase 0. It now runs property tests + Tier-3 goldens + Tier-4 fixtures across PG 14, 15, 16, 17. Property tests are limited to `cases = 50` in CI (longer runs go in the soak job).

```yaml
pg-matrix:
  name: pg matrix (tier 3-5)
  needs: [test]
  runs-on: ubuntu-latest
  strategy:
    fail-fast: false
    matrix:
      pg: ["14", "15", "16", "17"]
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
      with: { toolchain: 1.85 }
    - uses: Swatinem/rust-cache@v2
    - run: cargo test --workspace --features pg-tests
      env:
        PGEVOLVE_TEST_PG_VERSION: ${{ matrix.pg }}
        PROPTEST_CASES: 50
```

`pg-tests` feature flag controls Docker-required tests; default is off so local `cargo test` doesn't require Docker.

Commit: `ci: enable PG matrix for tier-3/4/5 tests`

---

### Task 11.7: Soak job (manual trigger)

**File:** `.github/workflows/soak.yml`

```yaml
name: soak

on:
  workflow_dispatch:
  schedule:
    - cron: '0 4 * * 0'   # weekly Sunday 04:00 UTC

jobs:
  soak:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        pg: ["14", "15", "16", "17"]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: 1.85 }
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --features pg-tests --release
        env:
          PGEVOLVE_TEST_PG_VERSION: ${{ matrix.pg }}
          PROPTEST_CASES: 5000
        timeout-minutes: 240
```

Commit: `ci: weekly soak job runs property tests at 5000 cases per PG version`

---

### Task 11.8: Phase 11 self-review

- Tier-5 property tests pass on every supported PG major.
- `IRGenerator` distribution sanity check passes.
- Soak job dispatches manually without errors.
- `cargo test --workspace --features pg-tests` (with Docker) passes.

Phase 11 complete.
