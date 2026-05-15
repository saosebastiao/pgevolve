# Test Strategy v2 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand the Tier C conformance suite from proof-of-life (one fixture, four assertion layers) to comprehensive (five fixture subtrees, nine assertion layers, pluggable test PG backend, capture-regression tooling, runtime budgets).

**Architecture:** Ten phased landings (T0 through T7.5) into `pgevolve-conformance`, `pgevolve-testkit`, and `xtask`. Each phase is independently testable. T0–T3 are sequential prerequisites; T4–T7.5 can land in parallel. T2.5 depends on Task 10 of the architecture-readiness plan landing (`pgevolve graph` command).

**Tech stack:** Rust 1.78+, toml, serde, testcontainers, tokio_postgres, walkdir, anyhow, clap (xtask), pg_query.

**Source spec:** `docs/superpowers/specs/2026-05-15-test-strategy-v2-design.md`

**Companion plan:** `docs/superpowers/plans/2026-05-15-v0.2-architecture-readiness.md` — Task 10 (`pgevolve graph`) is a prerequisite for T2.5.

---

## Pre-flight

- [ ] **Step 1: Confirm clean state**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests
```

Expected: all green.

- [ ] **Step 2: Confirm architecture plan Task 10 has landed**

```bash
cargo run -p pgevolve -- graph --help 2>&1 | head -10
```

Expected: help text mentioning `--format`. If the command is absent, complete Task 10 of the architecture-readiness plan before continuing past T2.

- [ ] **Step 3: Read the source spec**

Open `docs/superpowers/specs/2026-05-15-test-strategy-v2-design.md` and skim §3 (layer table), §4 (taxonomy), §5 (layer details), §6 (fixture.toml schema), §9 (TestPgBackend), §11 (capture tooling), §14 (phasing). Each phase below quotes the load-bearing section.

---

## File structure

```
crates/pgevolve-conformance/
├── AUTHORING.md                       NEW — T0 — per-subtree authoring contract
├── src/
│   ├── lib.rs                         MODIFY — re-exports
│   ├── fixture.rs                     MODIFY — T0, T3, T2.5, T4, T6 — schema additions
│   ├── walk.rs                        NEW — T0 — multi-subtree walker
│   ├── planning.rs                    MODIFY — T2.5 — graph rendering hookup
│   ├── failure.rs                     NEW — T4 — failure-fixture runner
│   └── assertions/
│       ├── mod.rs                     MODIFY — re-exports
│       ├── diff.rs                    UNCHANGED
│       ├── plan.rs                    UNCHANGED (L2/L3)
│       ├── apply.rs                   MODIFY — T1 — L5 minimality follow-up
│       ├── minimality.rs              NEW — T1
│       ├── touches_only.rs            NEW — T2
│       ├── dep_graph.rs               NEW — T2.5
│       ├── topological_order.rs       NEW — T2.5
│       └── intent_shape.rs            NEW — T3
└── tests/
    ├── run.rs                         MODIFY — T0 — dispatch into subtrees
    └── cases/
        ├── objects/                   T0 — existing fixture moves here
        ├── scenarios/                 T0 — new subtree
        │   └── dependency-chains/     T2.5 — first fixture lands here
        ├── intent/                    T0 — new subtree
        ├── failure/                   T0 — new subtree
        │   ├── parse/
        │   ├── ast-resolution/
        │   ├── cycle/
        │   └── lint-at-plan/
        └── regressions/               T0 — new subtree

crates/pgevolve-testkit/src/
├── test_pg_backend.rs                 NEW — T5 — TestPgBackend trait + 3 impls
└── lib.rs                             MODIFY — re-export

dev/
└── docker-compose.pg.yml              NEW — T5 — shipped compose file

xtask/src/
├── main.rs                            MODIFY — T2.5, T7, T7.5 — new subcommands
├── new_fixture.rs                     NEW — T0 — scaffold a fixture directory
├── coverage.rs                        NEW — T6 — coverage check + gaps
├── fixture_cost.rs                    NEW — T7 — timing report
├── capture_regression.rs              NEW — T7.5 — proptest seed → fixture
├── verify_regression.rs               NEW — T7.5
├── property_status.rs                 NEW — T7.5
└── diagnose_pg_version.rs             NEW — T7.5

.github/workflows/
├── ci.yml                             MODIFY — T6, T7.5 — coverage + property-status gates
└── property-tests.yml                 MODIFY — T7.5 — auto-open capture PR
```

---

## Phase T0: Authoring tree split

**Goal:** Move the existing `tables/add-column-nullable` fixture into `objects/tables/add-column-nullable/`. Create empty `scenarios/`, `intent/`, `failure/`, `regressions/` directories. Walker recognizes all five subtrees and routes by `authoring` key.

**Files:** see file-structure section above; T0-flagged rows.

**Load-bearing spec section:** §4 (Fixture taxonomy).

- [ ] **Step 1: Read existing walker**

```bash
sed -n '1,200p' crates/pgevolve-conformance/tests/run.rs
```

Note how it currently discovers fixtures and constructs test cases.

- [ ] **Step 2: Move the existing fixture**

```bash
mkdir -p crates/pgevolve-conformance/tests/cases/objects
git mv crates/pgevolve-conformance/tests/cases/tables crates/pgevolve-conformance/tests/cases/objects/tables
mkdir -p crates/pgevolve-conformance/tests/cases/{scenarios,intent,regressions}
mkdir -p crates/pgevolve-conformance/tests/cases/failure/{parse,ast-resolution,cycle,lint-at-plan}
touch crates/pgevolve-conformance/tests/cases/{scenarios,intent,regressions}/.gitkeep
touch crates/pgevolve-conformance/tests/cases/failure/{parse,ast-resolution,cycle,lint-at-plan}/.gitkeep
```

- [ ] **Step 3: Add the `authoring` field to `FixtureMeta`**

In `crates/pgevolve-conformance/src/fixture.rs`, find the existing `FixtureMeta` struct and add:

```rust
/// One of: "objects" | "scenarios" | "intent" | "failure" | "regressions".
/// Drives which assertion layers fire. Defaults to "objects" for
/// backward compatibility.
#[serde(default = "default_authoring")]
pub authoring: String,
```

```rust
fn default_authoring() -> String { "objects".to_string() }
```

Update the existing fixture's `fixture.toml` to include `authoring = "objects"` explicitly (informational; matches the default).

- [ ] **Step 4: Create the walker module**

Create `crates/pgevolve-conformance/src/walk.rs`:

```rust
//! Multi-subtree fixture walker.

use std::path::{Path, PathBuf};
use anyhow::Result;
use walkdir::WalkDir;

use crate::fixture::Fixture;

/// One discovered fixture plus its `authoring` routing key.
#[derive(Debug, Clone)]
pub struct DiscoveredFixture {
    pub fixture: Fixture,
    pub authoring: Authoring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authoring {
    Objects,
    Scenarios,
    Intent,
    Failure,
    Regressions,
}

impl Authoring {
    pub fn from_meta(s: &str) -> Result<Self> {
        Ok(match s {
            "objects" => Authoring::Objects,
            "scenarios" => Authoring::Scenarios,
            "intent" => Authoring::Intent,
            "failure" => Authoring::Failure,
            "regressions" => Authoring::Regressions,
            other => anyhow::bail!("unknown authoring key: {other}"),
        })
    }
}

/// Walk a cases root and return every fixture found, keyed by authoring.
pub fn discover(cases_root: &Path) -> Result<Vec<DiscoveredFixture>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(cases_root).into_iter().filter_map(|e| e.ok()) {
        if entry.file_name() != "fixture.toml" { continue; }
        let dir = entry.path().parent().unwrap().to_path_buf();
        let fixture = Fixture::load(&dir)?;
        let authoring = Authoring::from_meta(&fixture.meta.authoring)
            .map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        // Cross-check: the subtree the fixture lives in should match its
        // declared authoring key. e.g. fixture in objects/... should have
        // authoring="objects".
        let inferred = infer_authoring(&dir, cases_root)
            .ok_or_else(|| anyhow::anyhow!("{}: cannot infer authoring from path", dir.display()))?;
        if inferred != authoring {
            anyhow::bail!(
                "{}: declared authoring {:?} but lives under {:?} subtree",
                dir.display(), authoring, inferred,
            );
        }
        out.push(DiscoveredFixture { fixture, authoring });
    }
    out.sort_by_key(|f| f.fixture.dir.clone());
    Ok(out)
}

fn infer_authoring(dir: &Path, cases_root: &Path) -> Option<Authoring> {
    let rel = dir.strip_prefix(cases_root).ok()?;
    let top = rel.iter().next()?.to_str()?;
    Authoring::from_meta(top).ok()
}
```

Re-export in `crates/pgevolve-conformance/src/lib.rs`:

```rust
pub mod walk;
```

- [ ] **Step 5: Update `tests/run.rs` to use the walker**

Replace the existing single-tree iteration with `walk::discover`. For T0, only `Authoring::Objects` and trivially-empty subtrees should produce test cases — other authoring kinds fall through to a `not yet implemented` test that the existing layer set doesn't run. Wire in subsequent phases.

```rust
// Pseudocode for the new dispatcher:
let cases_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
let fixtures = pgevolve_conformance::walk::discover(&cases_root)?;
for f in fixtures {
    match f.authoring {
        Authoring::Objects => {
            run_objects_fixture(&f.fixture)?; // existing 4 layers
        }
        Authoring::Scenarios | Authoring::Intent | Authoring::Failure | Authoring::Regressions => {
            // Other authoring kinds will be wired by later phases. For
            // T0 we still discover and validate them; running their
            // layer sets is T2.5 / T3 / T4 work.
            eprintln!("skip {}: authoring kind {:?} not yet wired", f.fixture.dir.display(), f.authoring);
        }
    }
}
```

- [ ] **Step 6: Add `AUTHORING.md`**

Create `crates/pgevolve-conformance/AUTHORING.md`:

```markdown
# Conformance fixture authoring

Each fixture directory contains:

- `fixture.toml` — metadata + expectations
- `before.sql` — "what's already in the DB"
- `after.sql` — "desired state"

The directory it lives under (and the `authoring` key in `fixture.toml`)
determines which assertion layers apply.

## Authoring subtrees

### `objects/<kind>/<change>/`

One feature × one change-kind. Runs L1 (diff), L2 (plan structural),
L3 (plan SQL golden), L4 (apply roundtrip), L5 (minimality). L6
(no-collateral-damage) is opt-in via `touches_only`. L7 (intent shape)
fires when the change is destructive.

### `scenarios/`

Multi-feature workflows. Same as `objects/` except L6 is not asserted
(scenarios are meant to touch multiple objects). L8 (dep-graph
golden) and L9 (topological-order) are opt-in.

### `intent/`

Primary contract is the intent or lint-waiver surface. L1, L7 fire.
L4 applies the plan after the runner sets `[[intent]].approved = true`
and `[[lint_waiver]]` rows as the fixture declares.

### `failure/`

Primary contract is that pgevolve *refuses*. L1–L9 skipped; the
failure layer asserts on stage, exit code, and stderr substrings.
Stages: `parse`, `ast_resolution`, `order` (body-cycle), `lint_at_plan`.

### `regressions/`

One-off captures from property-test failures. Same shape as `objects/`;
references the originating issue in `[meta].issue`.

## Minimum fields per `authoring` kind

| Authoring | Mandatory | Layers fired |
|---|---|---|
| objects | [meta], [pg], [expect.diff], [expect.plan] | L1–L5, L7 (if destructive) |
| scenarios | [meta], [pg], [expect.diff], [expect.plan] | L1–L5, L7 |
| intent | [meta], [pg], [expect.diff], [[expect.intent]] | L1, L4, L7 |
| failure | [meta], [pg], [expect.failure] | failure layer only |
| regressions | [meta] with issue, [pg], standard expects | per authoring of the captured shape |

## When to bless vs investigate

`cargo xtask bless --conformance` is the right move when:
- You changed planner SQL emission deliberately and the diff matches.
- A canonicalizer change re-formatted bodies you expected.

It's the wrong move when:
- The byte change has no obvious source. Investigate first.
- The diff includes things you didn't touch. Investigate first.

## Capturing a property-test failure

Run `cargo xtask capture-regression --seed <hex> --issue <n>` (see
the xtask docs). The tool re-runs the proptest case to materialize
the minimized IR pair, renders both to SQL, and scaffolds
`regressions/issue-<n>/{fixture.toml, before.sql, after.sql}`.

## Adding a per-version override

Either use `[pg.expect].<N> = "failure"` (skip / fail per major) or
`[expect.plan.pg<N>]` (override individual structural fields). Cap at
one override per fixture; more than that is a fixture-split signal.
```

- [ ] **Step 7: Run the suite**

```bash
cargo test -p pgevolve-conformance 2>&1 | tail -20
```

Expected: existing fixture still passes in its new location; empty subtrees produce no test cases or trivial skip messages.

- [ ] **Step 8: Commit**

```bash
git add -A crates/pgevolve-conformance/
git commit -m "$(cat <<'EOF'
test(conformance): split tests/cases into 5 authoring subtrees (T0)

Existing tables/add-column-nullable moves to
objects/tables/add-column-nullable/. New subtrees scaffolded:
scenarios/, intent/, failure/{parse,ast-resolution,cycle,lint-at-plan}/,
regressions/. FixtureMeta gains an `authoring` field; the new walker
in src/walk.rs cross-checks declared authoring against subtree
location. AUTHORING.md documents the per-subtree contract.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T1: L5 minimality layer

**Goal:** After L4 applies the plan and asserts post-apply IR equivalence, re-plan against the just-applied DB. Assert the result is empty.

**Files:**
- Create: `crates/pgevolve-conformance/src/assertions/minimality.rs`
- Modify: `crates/pgevolve-conformance/src/assertions/mod.rs`
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (add `minimality` opt-out)
- Modify: `crates/pgevolve-conformance/tests/run.rs` (call L5 after L4)

**Load-bearing spec section:** §5.1.

- [ ] **Step 1: Add `minimality` to `ExpectPlan`**

In `crates/pgevolve-conformance/src/fixture.rs`, extend `ExpectPlan`:

```rust
/// L5 opt-out. Default true. Set to false for fixtures whose change
/// is itself a no-op (rare).
#[serde(default = "default_true")]
pub minimality: bool,
```

Update the existing `Default for ExpectPlan` impl to set `minimality: true`.

- [ ] **Step 2: Create the assertion module**

Create `crates/pgevolve-conformance/src/assertions/minimality.rs`:

```rust
//! L5 — minimality.
//!
//! After L4's apply, re-read the catalog into IR and re-plan against
//! the after.sql source IR. Assert the resulting diff is empty and
//! the plan has zero groups.

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;

pub struct MinimalityInput<'a> {
    pub post_apply_catalog: &'a Catalog,
    pub post_apply_drift: &'a pgevolve_core::catalog::DriftReport,
    pub after_source: &'a Catalog,
}

pub fn assert_minimal(input: MinimalityInput<'_>) -> Result<()> {
    let changes = pgevolve_core::diff::diff(
        input.post_apply_catalog,
        input.after_source,
        input.post_apply_drift,
    );
    // ChangeSet API: at the time of this plan, `ChangeSet` exposes its
    // changes via a public field or a method. Read crates/pgevolve-core/src/diff/changeset.rs
    // to confirm; the current shape is `pub struct ChangeSet { pub entries: Vec<ChangeEntry> }`,
    // so `.entries.is_empty()` is the v0.1 spelling.
    let n = changes.entries.len();
    if n > 0 {
        anyhow::bail!(
            "L5 minimality: re-plan against post-apply state produced {n} change(s); plan is not minimal:\n{}",
            render_changes(&changes),
        );
    }
    Ok(())
}

fn render_changes(_changes: &pgevolve_core::diff::changeset::ChangeSet) -> String {
    // Reuse the existing renderer that L1 uses; placeholder shape.
    String::from("(use existing change-set debug renderer)")
}
```

- [ ] **Step 3: Wire L5 into the runner**

In `crates/pgevolve-conformance/tests/run.rs`, after L4's apply succeeds:

```rust
if fixture.expect.plan.minimality {
    // After L4's apply, re-read the catalog and run minimality.
    let querier = /* the same querier used by L4 */;
    let (catalog, drift) = read_catalog(&querier, &Default::default())?;
    let after_source = parse_after_sql(&fixture)?;
    pgevolve_conformance::assertions::minimality::assert_minimal(
        pgevolve_conformance::assertions::minimality::MinimalityInput {
            post_apply_catalog: &catalog,
            post_apply_drift: &drift,
            after_source: &after_source,
        },
    )?;
}
```

- [ ] **Step 4: Confirm existing fixture passes L5**

```bash
cargo test -p pgevolve-conformance 2>&1 | tail -20
```

Expected: PASS for `objects/tables/add-column-nullable/`. If it fails, the planner is emitting a follow-up step (e.g., redundant COMMENT) — fix the planner before claiming T1 done.

- [ ] **Step 5: Write a negative test**

Add an artificial fixture that *is* a no-op intentionally:

```bash
mkdir -p crates/pgevolve-conformance/tests/cases/objects/tables/noop-explicit-opt-out
```

`fixture.toml`:
```toml
[meta]
title = "noop fixture exercises L5 opt-out"
authoring = "objects"

[expect.plan]
minimality = false
steps = 0
```

`before.sql` and `after.sql`: identical schemas.

```bash
cargo test -p pgevolve-conformance 2>&1 | tail -10
```

Expected: PASS — the opt-out lets the fixture skip L5 cleanly.

- [ ] **Step 6: Add a property test for minimality**

In `crates/pgevolve-core/tests/property_tests.rs`, add:

```rust
proptest! {
    /// Property: planning C → C for any random catalog C produces an
    /// empty plan. Pure; no Docker.
    #[test]
    #[ignore]
    fn plan_minimality_under_no_op_mutations(catalog in arbitrary_catalog()) {
        let drift = pgevolve_core::catalog::DriftReport::default();
        let changes = pgevolve_core::diff::diff(&catalog, &catalog, &drift);
        prop_assert!(changes.entries().next().is_none(), "C → C produced {:?}", changes);
    }
}
```

- [ ] **Step 7: Full suite check**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add -A crates/pgevolve-conformance/ crates/pgevolve-core/tests/property_tests.rs
git commit -m "$(cat <<'EOF'
test(conformance): L5 minimality assertion (T1)

After L4's apply succeeds, re-plan against the just-applied catalog
and assert the result is empty. Catches planners that emit
follow-up no-op steps on every run. Opt-out via
expect.plan.minimality = false for fixtures whose change is itself
a no-op.

Adds matching property test plan_minimality_under_no_op_mutations
under #[ignore] (runs in the nightly property-tests workflow).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T2: L6 no-collateral-damage layer

**Goal:** Assert every step's target qname is in `expect.plan.touches_only`. Catches "changing one view recreated five untouched ones."

**Files:**
- Create: `crates/pgevolve-conformance/src/assertions/touches_only.rs`
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (add `touches_only` field)
- Modify: `crates/pgevolve-conformance/tests/run.rs`

**Load-bearing spec section:** §5.2.

- [ ] **Step 1: Add field to `ExpectPlan`**

```rust
/// L6 input. Absent / empty = layer skipped.
#[serde(default)]
pub touches_only: Vec<String>,
```

- [ ] **Step 2: Create the assertion module**

```rust
//! L6 — no collateral damage.

use anyhow::Result;
use std::collections::BTreeSet;
use pgevolve_core::plan::plan::Plan;

pub fn assert_touches_only(plan: &Plan, allowed: &[String]) -> Result<()> {
    if allowed.is_empty() { return Ok(()); }
    let allowed: BTreeSet<_> = allowed.iter().cloned().collect();
    let mut violations = Vec::new();
    for group in &plan.groups {
        for step in &group.steps {
            // RawStep::target is the public field carrying the
            // step's primary qname. See crates/pgevolve-core/src/plan/raw_step.rs
            // for the current field name; if it's `target_label` or
            // similar, use that.
            let target = step.target.to_string();
            if !allowed.contains(&target) {
                violations.push(format!("step {} → {}", step.step_no, target));
            }
        }
    }
    if !violations.is_empty() {
        anyhow::bail!(
            "L6 no-collateral-damage: {} step(s) touch targets outside the allowed set:\n  {}",
            violations.len(),
            violations.join("\n  "),
        );
    }
    Ok(())
}
```

- [ ] **Step 3: Wire L6 into the runner**

After L4 / before L5 (order doesn't matter; pick consistently):

```rust
if !fixture.expect.plan.touches_only.is_empty() {
    pgevolve_conformance::assertions::touches_only::assert_touches_only(
        &plan, &fixture.expect.plan.touches_only,
    )?;
}
```

- [ ] **Step 4: Write the affirmative + negative test fixtures**

Create `crates/pgevolve-conformance/tests/cases/objects/columns/add-column-touches-only/`:

`fixture.toml`:
```toml
[meta]
title = "add column touches only the target table"
authoring = "objects"

[expect.diff]
contains = ["app.users.email"]

[expect.plan]
steps = 1
touches_only = ["app.users", "app.users.email"]
```

`before.sql`:
```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (id bigint PRIMARY KEY);
```

`after.sql`:
```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id bigint PRIMARY KEY,
    email text
);
```

Run and confirm green:

```bash
cargo test -p pgevolve-conformance objects::columns::add_column_touches_only 2>&1 | tail -10
```

For negative confirmation, temporarily edit `touches_only` to `["app.products"]` and confirm the test fails with the right message; revert.

- [ ] **Step 5: Full suite check**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add -A crates/pgevolve-conformance/
git commit -m "$(cat <<'EOF'
test(conformance): L6 no-collateral-damage assertion (T2)

Opt-in via expect.plan.touches_only. Assertions over each step's
target qname catch the class of bug where touching one object
forces unrelated objects to recreate. First fixture:
objects/columns/add-column-touches-only.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T2.5: L8 dep-graph golden + L9 topological-order

**Goal:** L8 byte-compares the rendered DOT output of `pgevolve graph` against `expected/dep-graph.dot`. L9 asserts that declared partial orders in `expect.plan.order` are respected.

**Files:**
- Create: `crates/pgevolve-conformance/src/assertions/dep_graph.rs`
- Create: `crates/pgevolve-conformance/src/assertions/topological_order.rs`
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (new fields)
- Modify: `xtask/src/main.rs` (extend `bless` to also regenerate `expected/dep-graph.dot`)

**Load-bearing spec sections:** §5.5, §5.6.

**Prerequisite:** Architecture-readiness plan Task 10 (`pgevolve graph`) must be landed.

- [ ] **Step 1: Add `dep_graph` and `order` fields**

In `crates/pgevolve-conformance/src/fixture.rs`:

```rust
/// L8 — dep-graph golden.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectDepGraph {
    /// Default true; opt out for trivial fixtures.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Golden file path; default expected/dep-graph.dot.
    #[serde(default = "default_dep_graph_golden")]
    pub golden: String,
}

fn default_dep_graph_golden() -> String { "expected/dep-graph.dot".to_string() }

impl Default for ExpectDepGraph {
    fn default() -> Self {
        Self { enabled: true, golden: default_dep_graph_golden() }
    }
}

// Add to FixtureExpect:
#[serde(default)]
pub dep_graph: ExpectDepGraph,
```

For L9, extend `ExpectPlan`:

```rust
/// L9 input. Each entry is "A < B" — when both targets appear,
/// A must precede B. Empty = layer skipped.
#[serde(default)]
pub order: Vec<String>,
```

- [ ] **Step 2: Implement L8 (dep-graph golden)**

```rust
//! L8 — dep-graph golden.

use std::path::Path;
use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::plan::edges::build_create_graph;

pub fn assert_dep_graph_golden(
    source: &Catalog,
    fixture_dir: &Path,
    golden_rel: &str,
) -> Result<()> {
    let graph = build_create_graph(source);
    let mut edges: Vec<_> = graph.dep_edges().collect();
    edges.sort();
    // Reuse the rendering from pgevolve binary's `graph` command by
    // calling the public renderer. We expose it from pgevolve-core or
    // duplicate the small renderer here; latter is simpler and isolates
    // conformance from binary-internal changes.
    let actual = render_dot(&edges);
    let golden_path = fixture_dir.join(golden_rel);
    let expected = std::fs::read_to_string(&golden_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", golden_path.display()))?;
    let actual_norm = normalize_dot(&actual);
    let expected_norm = normalize_dot(&expected);
    if actual_norm != expected_norm {
        anyhow::bail!(
            "L8 dep-graph golden mismatch.\n--- expected ({})\n+++ actual\n{}",
            golden_path.display(),
            unified_diff(&expected_norm, &actual_norm),
        );
    }
    Ok(())
}

fn render_dot(edges: &[pgevolve_core::plan::edges::DepEdge]) -> String {
    // Duplicate of pgevolve/src/commands/graph.rs's render_dot. Inlined
    // rather than imported because pgevolve-conformance shouldn't depend
    // on the binary crate. If it drifts from the binary's version,
    // `cargo xtask bless --conformance` catches it because the same
    // edges produce different DOT text.
    use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};
    let mut out = String::from(
        "digraph pgevolve_deps {\n  rankdir=LR;\n  node [shape=box, fontname=Helvetica];\n",
    );
    let mut nodes: std::collections::BTreeSet<String> = Default::default();
    let node_label = |n: &NodeId| -> String {
        match n {
            NodeId::Schema(s) => format!("schema:{}", s.as_str()),
            NodeId::Table(q) => format!("table:{q}"),
            NodeId::Index(q) => format!("index:{q}"),
            NodeId::Sequence(q) => format!("sequence:{q}"),
            NodeId::Constraint { table, name } => {
                format!("constraint:{table}.{}", name.as_str())
            }
        }
    };
    for e in edges {
        nodes.insert(node_label(&e.from));
        nodes.insert(node_label(&e.to));
    }
    for n in &nodes {
        out.push_str(&format!("  \"{n}\";\n"));
    }
    let mut sorted: Vec<DepEdge> = edges.to_vec();
    sorted.sort();
    for e in sorted {
        let style = match e.source {
            DepSource::Structural => "solid",
            DepSource::AstExtracted => "dashed",
            DepSource::AstDeclared => "dotted",
        };
        out.push_str(&format!(
            "  \"{}\" -> \"{}\" [style={style}];\n",
            node_label(&e.from),
            node_label(&e.to),
        ));
    }
    out.push_str("}\n");
    out
}

fn normalize_dot(s: &str) -> String {
    // Strip blank lines, sort the edge lines lexicographically so insertion
    // order is irrelevant.
    let mut header = Vec::new();
    let mut edges = Vec::new();
    let mut in_body = false;
    for line in s.lines() {
        let t = line.trim();
        if t.starts_with('}') { break; }
        if t.is_empty() { continue; }
        if t.starts_with("digraph") { header.push(line.to_string()); in_body = true; continue; }
        if !in_body { continue; }
        edges.push(line.to_string());
    }
    edges.sort();
    let mut out = header.join("\n");
    out.push('\n');
    for e in &edges { out.push_str(e); out.push('\n'); }
    out.push_str("}\n");
    out
}

fn unified_diff(_a: &str, _b: &str) -> String {
    // Use the `similar` crate if available; for now, simple line dump.
    String::from("(diff)")
}
```

- [ ] **Step 3: Implement L9 (topological-order)**

```rust
//! L9 — topological-order assertion.

use anyhow::Result;
use pgevolve_core::plan::plan::Plan;

pub fn assert_order(plan: &Plan, declared: &[String]) -> Result<()> {
    let step_targets: Vec<String> = plan.groups.iter()
        .flat_map(|g| g.steps.iter().map(|s| s.target.to_string()))
        .collect();
    let position = |target: &str| step_targets.iter().position(|t| t == target);
    for decl in declared {
        // Each declared entry is "A < B".
        let (a, b) = decl.split_once('<').ok_or_else(|| anyhow::anyhow!("bad order entry: {decl}"))?;
        let (a, b) = (a.trim(), b.trim());
        if let (Some(ai), Some(bi)) = (position(a), position(b)) {
            if ai >= bi {
                anyhow::bail!(
                    "L9 topological-order: {a} must precede {b}, but {a} at step {} and {b} at step {}",
                    ai + 1, bi + 1,
                );
            }
        }
        // If either target is absent from the plan, the partial order is
        // vacuously satisfied; do not error.
    }
    Ok(())
}
```

- [ ] **Step 4: Wire L8 and L9 into the runner**

In `tests/run.rs`:

```rust
if fixture.expect.dep_graph.enabled {
    pgevolve_conformance::assertions::dep_graph::assert_dep_graph_golden(
        &source_catalog, &fixture.dir, &fixture.expect.dep_graph.golden,
    )?;
}
if !fixture.expect.plan.order.is_empty() {
    pgevolve_conformance::assertions::topological_order::assert_order(
        &plan, &fixture.expect.plan.order,
    )?;
}
```

- [ ] **Step 5: Extend `cargo xtask bless --conformance` to regenerate `expected/dep-graph.dot`**

In `xtask/src/main.rs`, the existing `bless --conformance` regenerates `expected/plan.sql`. Add: for each fixture whose `expect.dep_graph.enabled = true`, render and write `expected/dep-graph.dot`.

- [ ] **Step 6: Bless existing fixtures**

```bash
cargo xtask bless --conformance
git diff crates/pgevolve-conformance/tests/cases/
```

Expected: new `expected/dep-graph.dot` files appear under each `objects/` fixture. Review the diff; the contents should match the underlying schema's dep relationships.

- [ ] **Step 7: Add the first dependency-chains fixture**

Create `crates/pgevolve-conformance/tests/cases/scenarios/dependency-chains/linear-3-layer-create/`:

`fixture.toml`:
```toml
[meta]
title = "function → MV → view: create order"
authoring = "scenarios"

# IMPORTANT: This fixture exercises v0.2 functions/MVs/views which
# are not yet implemented. It is gated by [pg.expect] until the v0.2
# function/view sub-specs land. Update once those land.
[pg.expect]
"14" = "skip"
"15" = "skip"
"16" = "skip"
"17" = "skip"

[expect.diff]
contains = ["app.fns.make_summary", "app.mvs.summary", "app.views.summary_dashboard"]

[expect.plan]
steps = 3
order = [
  "app.fns.make_summary < app.mvs.summary",
  "app.mvs.summary < app.views.summary_dashboard",
]
```

`before.sql`: empty schema.

`after.sql`: function + MV + view per the title.

> **Note:** The fixture lands skipped because v0.2 sub-specs haven't implemented the underlying objects. Reviewing the fixture confirms the L9 syntax is well-formed; future sub-specs flip the `[pg.expect]` to `"success"`.

- [ ] **Step 8: Run the suite**

```bash
cargo test -p pgevolve-conformance 2>&1 | tail -15
```

Expected: existing fixtures pass L8 with the blessed `dep-graph.dot`; dependency-chains fixture skips.

- [ ] **Step 9: Full suite check**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test(conformance): L8 dep-graph golden + L9 topological-order (T2.5)

L8 byte-compares the rendered dep-graph DOT against
expected/dep-graph.dot per fixture; default on, opt-out via
expect.dep_graph.enabled = false. L9 asserts declared partial orders
in expect.plan.order are respected by the emitted step sequence;
opt-in.

cargo xtask bless --conformance now also regenerates dep-graph.dot
goldens. Existing v0.1 fixtures blessed. First scenarios fixture
linear-3-layer-create lands skipped pending v0.2 function/view
sub-specs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T3: L7 intent-shape layer

**Goal:** Mandatory-on-destructive layer that asserts every entry in `[[expect.intent]]` matches a generated `intent.toml` row.

**Files:**
- Create: `crates/pgevolve-conformance/src/assertions/intent_shape.rs`
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (new field)
- Modify: `crates/pgevolve-conformance/tests/run.rs`

**Load-bearing spec section:** §5.3.

- [ ] **Step 1: Add `expect.intent` field**

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectIntentRow {
    pub kind: String,
    pub target: String,
    #[serde(default)]
    pub reason_contains: Vec<String>,
}

// Inside FixtureExpect:
#[serde(default, rename = "intent")]
pub intent: Vec<ExpectIntentRow>,
```

Note: this collides with the existing `intent` passthrough table that flows verbatim into `intent.toml`. Resolve by renaming the existing passthrough to `intent_overrides` or using a distinct top-level key. Easiest path: the existing passthrough is at the top of `fixture.toml` (e.g., `[intent]` block); the new field is `[[expect.intent]]` rows under the `[expect]` table. The toml serialization is unambiguous; verify by reading both sections separately.

- [ ] **Step 2: Implement the assertion**

```rust
//! L7 — intent shape.

use anyhow::Result;
use pgevolve_core::plan::plan::{IntentFile, DestructiveIntent};
use crate::fixture::ExpectIntentRow;

pub fn assert_intent_shape(
    expected: &[ExpectIntentRow],
    generated: &IntentFile,
    is_destructive: bool,
) -> Result<()> {
    if is_destructive && expected.is_empty() {
        anyhow::bail!("L7: destructive fixture must declare at least one [[expect.intent]] row");
    }
    if !is_destructive && !expected.is_empty() {
        anyhow::bail!("L7: non-destructive fixture must not declare [[expect.intent]]");
    }
    if expected.len() != generated.intents.len() {
        anyhow::bail!(
            "L7 intent count mismatch: expected {} but planner generated {}",
            expected.len(), generated.intents.len(),
        );
    }
    for (i, exp) in expected.iter().enumerate() {
        let Some(matched) = generated.intents.iter().find(|g| g.kind == exp.kind && g.target == exp.target) else {
            anyhow::bail!(
                "L7: no generated intent matches expected #{i}: kind={} target={}",
                exp.kind, exp.target,
            );
        };
        for needle in &exp.reason_contains {
            if !matched.reason.contains(needle) {
                anyhow::bail!(
                    "L7: intent #{i} reason {:?} missing substring {:?}",
                    matched.reason, needle,
                );
            }
        }
    }
    Ok(())
}
```

> **Verification:** `IntentFile::intents`, `DestructiveIntent::{kind,target,reason}` — adapt to the actual field names in `crates/pgevolve-core/src/plan/plan.rs`.

- [ ] **Step 3: Wire L7 into the runner**

L7 runs after L2 (we have the plan and its intents) and before L4 (so the runner can decide whether to mark `approved = true` for the apply path).

```rust
// `plan.is_destructive` is a derived predicate: true iff any step's
// `destructive: bool` field is set. If the existing Plan API doesn't
// expose a predicate, inline the check:
//   let is_destructive = plan.groups.iter().flat_map(|g| &g.steps).any(|s| s.destructive);
let is_destructive = plan.groups.iter().flat_map(|g| &g.steps).any(|s| s.destructive);
if matches!(fixture.authoring, Authoring::Objects | Authoring::Scenarios | Authoring::Intent | Authoring::Regressions) {
    pgevolve_conformance::assertions::intent_shape::assert_intent_shape(
        &fixture.expect.intent, &intent_file, is_destructive,
    )?;
}
```

- [ ] **Step 4: Create the first `intent/` fixture**

`crates/pgevolve-conformance/tests/cases/intent/drop-column-requires-intent/`:

`fixture.toml`:
```toml
[meta]
title = "DROP COLUMN requires an approved intent"
authoring = "intent"

[expect.diff]
contains = ["app.users.legacy_id"]

[expect.plan]
steps = 1

[[expect.intent]]
kind = "drop_column"
target = "app.users.legacy_id"
reason_contains = ["removes column"]
```

`before.sql`:
```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id bigint PRIMARY KEY,
    legacy_id text
);
```

`after.sql`:
```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (id bigint PRIMARY KEY);
```

The runner sets `approved = true` automatically for `intent/` fixtures.

- [ ] **Step 5: Create the negative fixture under `failure/lint-at-plan/`**

`crates/pgevolve-conformance/tests/cases/failure/lint-at-plan/drop-column-without-intent/`:

`fixture.toml`:
```toml
[meta]
title = "DROP COLUMN without [[intent]] approval refuses at preflight"
authoring = "failure"

[expect.failure]
stage = "lint_at_plan"
error_code = 2
stderr_contains = ["intent", "drop_column"]
```

`before.sql` and `after.sql`: same as above.

> **Note:** This fixture asserts the failure-fixture machinery from T4. It lands now as a co-dependent; T4 implements the actual failure runner.

- [ ] **Step 6: Run the suite**

```bash
cargo test -p pgevolve-conformance 2>&1 | tail -20
```

Expected: `intent/drop-column-requires-intent` passes; `failure/lint-at-plan/drop-column-without-intent` is discovered but skipped (no failure runner yet — wired in T4).

- [ ] **Step 7: Full suite check**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test(conformance): L7 intent-shape assertion (T3)

Mandatory-on-destructive layer matches generated intent.toml rows
against the fixture's [[expect.intent]] entries. Non-destructive
fixtures must not declare [[expect.intent]]. First intent/ fixture
drop-column-requires-intent lands; the negative counterpart in
failure/lint-at-plan/ lands skipped pending T4 failure runner.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T4: `failure/` subtree runner

**Goal:** Implement the failure-fixture layer. Runs the appropriate pipeline stage (parse / parse+ast_resolution / parse+ast_resolution+order / full pipeline through plan), asserts exit code and stderr substrings.

**Files:**
- Create: `crates/pgevolve-conformance/src/failure.rs`
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (add `expect.failure` block)
- Modify: `crates/pgevolve-conformance/tests/run.rs`

**Load-bearing spec section:** §5.4.

- [ ] **Step 1: Add `ExpectFailure` shape**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectFailure {
    /// Stage: parse | ast_resolution | order | lint_at_plan.
    pub stage: String,
    /// Exit code from the CLI.
    pub error_code: i32,
    /// Substrings that must appear in stderr.
    #[serde(default)]
    pub stderr_contains: Vec<String>,
}

// In FixtureExpect:
#[serde(default)]
pub failure: Option<ExpectFailure>,
```

- [ ] **Step 2: Implement the failure runner**

```rust
//! Failure-fixture runner. Invokes the appropriate pipeline stage and
//! asserts the failure shape.

use std::path::Path;
use anyhow::Result;
use crate::fixture::{Fixture, ExpectFailure};

pub fn run_failure_fixture(fixture: &Fixture) -> Result<()> {
    let exp = fixture.expect.failure.as_ref()
        .ok_or_else(|| anyhow::anyhow!("failure fixture missing [expect.failure]"))?;
    match exp.stage.as_str() {
        "parse" => run_parse_stage(fixture, exp),
        "ast_resolution" => run_ast_resolution_stage(fixture, exp),
        "order" => run_order_stage(fixture, exp),
        "lint_at_plan" => run_lint_at_plan_stage(fixture, exp),
        other => anyhow::bail!("unknown failure stage: {other}"),
    }
}

fn run_parse_stage(fixture: &Fixture, exp: &ExpectFailure) -> Result<()> {
    let tmp = stage_source(fixture, "after.sql")?;
    let err = pgevolve_core::parse::parse_directory(tmp.path(), &[]).unwrap_err();
    assert_substrings(&err.to_string(), &exp.stderr_contains)?;
    // We can't exit-code check in-process; the in-process runner only
    // covers stage + substrings. A separate test (failure_e2e) runs the
    // CLI and checks exit code.
    Ok(())
}

fn run_ast_resolution_stage(fixture: &Fixture, exp: &ExpectFailure) -> Result<()> {
    let tmp = stage_source(fixture, "after.sql")?;
    let err = pgevolve_core::parse::parse_directory(tmp.path(), &[]).unwrap_err();
    let msg = err.to_string();
    if !msg.contains("AST resolution failed") {
        anyhow::bail!("expected AST resolution error, got: {msg}");
    }
    assert_substrings(&msg, &exp.stderr_contains)?;
    Ok(())
}

fn run_order_stage(fixture: &Fixture, exp: &ExpectFailure) -> Result<()> {
    let tmp = stage_source(fixture, "after.sql")?;
    let source = pgevolve_core::parse::parse_directory(tmp.path(), &[])?;
    let empty = pgevolve_core::ir::catalog::Catalog::default();
    let drift = pgevolve_core::catalog::DriftReport::default();
    let changes = pgevolve_core::diff::diff(&empty, &source, &drift);
    let err = pgevolve_core::plan::order(&empty, &source, &changes).unwrap_err();
    let msg = err.to_string();
    if !msg.contains("body-derived") && !msg.contains("UnbreakableCycle") {
        anyhow::bail!("expected cycle error, got: {msg}");
    }
    assert_substrings(&msg, &exp.stderr_contains)?;
    Ok(())
}

fn run_lint_at_plan_stage(_fixture: &Fixture, _exp: &ExpectFailure) -> Result<()> {
    // lint-at-plan failure happens in the binary, not the library. Use
    // the CLI runner.
    anyhow::bail!("lint-at-plan failure stage is tested via the CLI runner; see failure_e2e.rs")
}

fn stage_source(fixture: &Fixture, which: &str) -> Result<tempfile::TempDir> {
    let tmp = tempfile::tempdir()?;
    let path = if which == "after.sql" { &fixture.after_sql } else { &fixture.before_sql };
    let dir = tmp.path().join("schema/app");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("0001.sql"), path)?;
    Ok(tmp)
}

fn assert_substrings(msg: &str, needles: &[String]) -> Result<()> {
    for n in needles {
        if !msg.contains(n) {
            anyhow::bail!("expected substring {n:?} in: {msg}");
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Wire into runner**

In `tests/run.rs`:

```rust
match f.authoring {
    Authoring::Failure => {
        pgevolve_conformance::failure::run_failure_fixture(&f.fixture)?;
        return Ok(()); // skip L1-L9
    }
    // ... existing dispatch
}
```

- [ ] **Step 4: Author one fixture per failure stage**

`failure/parse/duplicate-schema/fixture.toml`:
```toml
[meta]
title = "Two CREATE SCHEMA statements for the same name"
authoring = "failure"

[expect.failure]
stage = "parse"
error_code = 1
stderr_contains = ["duplicate"]
```

`failure/parse/duplicate-schema/before.sql`: empty
`failure/parse/duplicate-schema/after.sql`:
```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
-- @pgevolve schema=app
CREATE SCHEMA app;
```

`failure/ast-resolution/fk-to-missing-table/fixture.toml`:
```toml
[meta]
title = "FK references a table not declared in source"
authoring = "failure"

[expect.failure]
stage = "ast_resolution"
error_code = 1
stderr_contains = ["AST resolution failed", "app.users"]
```

`after.sql`:
```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
    id bigint PRIMARY KEY,
    user_id bigint NOT NULL,
    CONSTRAINT fk_user FOREIGN KEY (user_id) REFERENCES app.users (id)
);
```

`failure/cycle/` lands empty for v0.1 — body-cycle errors only fire when v0.2 sub-specs introduce body-bearing objects. Add a `.gitkeep` and a comment in `AUTHORING.md` explaining the v0.2-pending status.

`failure/lint-at-plan/drop-column-without-intent/` already created in T3.

- [ ] **Step 5: Run the suite**

```bash
cargo test -p pgevolve-conformance 2>&1 | tail -20
```

Expected: failure fixtures fire and pass.

- [ ] **Step 6: Full suite check**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test(conformance): failure/ subtree runner (T4)

Failure fixtures assert pgevolve refuses with the documented stage,
exit code, and stderr substrings. Stages: parse, ast_resolution,
order, lint_at_plan. First fixtures: failure/parse/duplicate-schema,
failure/ast-resolution/fk-to-missing-table,
failure/lint-at-plan/drop-column-without-intent. failure/cycle/
stays empty until v0.2 body-bearing objects land.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T5: `TestPgBackend` pluggability

**Goal:** Mirror the architecture-readiness `ShadowBackend` trait on the test side. Three backends: testcontainers (default), compose, dsn. `dev/docker-compose.pg.yml` shipped.

**Files:**
- Create: `crates/pgevolve-testkit/src/test_pg_backend.rs`
- Modify: `crates/pgevolve-testkit/src/lib.rs`
- Create: `dev/docker-compose.pg.yml`
- Modify: `crates/pgevolve-conformance/src/lib.rs` (use the trait)

**Load-bearing spec section:** §9.

- [ ] **Step 1: Implement the trait**

```rust
//! TestPgBackend — pluggable real-Postgres backend for tests.

use std::env;
use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, Copy)]
pub enum BackendMode {
    Testcontainers,
    Compose,
    Dsn,
}

impl BackendMode {
    pub fn from_env() -> Self {
        match env::var("PGEVOLVE_TEST_PG_MODE").as_deref() {
            Ok("compose") => Self::Compose,
            Ok("dsn") => Self::Dsn,
            _ => Self::Testcontainers,
        }
    }
}

#[async_trait]
pub trait TestPgGuard: Send {
    fn url(&self) -> &str;
    async fn reset(&mut self) -> Result<()>;
}

#[async_trait]
pub trait TestPgBackend: Send + Sync {
    async fn checkout(&self, major: u32) -> Result<Box<dyn TestPgGuard>>;
}

pub fn resolve() -> Result<Box<dyn TestPgBackend>> {
    match BackendMode::from_env() {
        BackendMode::Testcontainers => Ok(Box::new(TestcontainersBackend::default())),
        BackendMode::Compose => Ok(Box::new(ComposeBackend::from_env()?)),
        BackendMode::Dsn => Ok(Box::new(DsnBackend::from_env()?)),
    }
}

/// Implementations follow.

#[derive(Default)]
pub struct TestcontainersBackend;

#[async_trait]
impl TestPgBackend for TestcontainersBackend {
    async fn checkout(&self, major: u32) -> Result<Box<dyn TestPgGuard>> {
        let pg = crate::ephemeral_pg::EphemeralPostgres::start(major).await?;
        Ok(Box::new(TestcontainersGuard { pg }))
    }
}

struct TestcontainersGuard { pg: crate::ephemeral_pg::EphemeralPostgres }

#[async_trait]
impl TestPgGuard for TestcontainersGuard {
    fn url(&self) -> &str { self.pg.url() }
    async fn reset(&mut self) -> Result<()> {
        // testcontainers backend recreates the container on each checkout;
        // reset is a no-op (the next checkout boots a fresh container).
        Ok(())
    }
}

pub struct ComposeBackend { urls: std::collections::BTreeMap<u32, String> }

impl ComposeBackend {
    fn from_env() -> Result<Self> {
        let mut urls = std::collections::BTreeMap::new();
        for major in [14, 15, 16, 17] {
            let key = format!("PGEVOLVE_TEST_PG_{major}_URL");
            if let Ok(url) = env::var(&key) {
                urls.insert(major, url);
            }
        }
        if urls.is_empty() {
            anyhow::bail!("compose mode requires PGEVOLVE_TEST_PG_<MAJOR>_URL env vars");
        }
        Ok(Self { urls })
    }
}

#[async_trait]
impl TestPgBackend for ComposeBackend {
    async fn checkout(&self, major: u32) -> Result<Box<dyn TestPgGuard>> {
        let url = self.urls.get(&major).ok_or_else(|| anyhow::anyhow!("no compose URL for PG {major}"))?;
        let guard = ComposeGuard { url: url.clone() };
        guard.reset_now().await?;
        Ok(Box::new(guard))
    }
}

struct ComposeGuard { url: String }
impl ComposeGuard {
    async fn reset_now(&self) -> Result<()> {
        let (client, conn) = tokio_postgres::connect(&self.url, tokio_postgres::NoTls).await?;
        tokio::spawn(conn);
        client.batch_execute(
            "DO $$
             DECLARE r record;
             BEGIN
               FOR r IN SELECT nspname FROM pg_namespace
                        WHERE nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast', 'public')
                          AND nspname NOT LIKE 'pg_temp_%' AND nspname NOT LIKE 'pg_toast_temp_%'
               LOOP EXECUTE format('DROP SCHEMA %I CASCADE', r.nspname); END LOOP;
               EXECUTE 'DROP SCHEMA public CASCADE';
               EXECUTE 'CREATE SCHEMA public';
             END $$;"
        ).await?;
        Ok(())
    }
}

#[async_trait]
impl TestPgGuard for ComposeGuard {
    fn url(&self) -> &str { &self.url }
    async fn reset(&mut self) -> Result<()> { self.reset_now().await }
}

pub struct DsnBackend { urls: std::collections::BTreeMap<u32, String> }
impl DsnBackend {
    fn from_env() -> Result<Self> { ComposeBackend::from_env().map(|c| Self { urls: c.urls }) }
}

#[async_trait]
impl TestPgBackend for DsnBackend {
    async fn checkout(&self, major: u32) -> Result<Box<dyn TestPgGuard>> {
        let url = self.urls.get(&major).ok_or_else(|| anyhow::anyhow!("no DSN for PG {major}"))?;
        let guard = ComposeGuard { url: url.clone() };
        guard.reset_now().await?;
        Ok(Box::new(guard))
    }
}
```

- [ ] **Step 2: Ship `dev/docker-compose.pg.yml`**

```yaml
# PG 14/15/16/17 on fixed ports for fast local test iteration.
# Usage:
#   docker compose -f dev/docker-compose.pg.yml up -d
#   PGEVOLVE_TEST_PG_MODE=compose \
#     PGEVOLVE_TEST_PG_14_URL=postgres://postgres:postgres@localhost:54214/postgres \
#     PGEVOLVE_TEST_PG_15_URL=postgres://postgres:postgres@localhost:54215/postgres \
#     PGEVOLVE_TEST_PG_16_URL=postgres://postgres:postgres@localhost:54216/postgres \
#     PGEVOLVE_TEST_PG_17_URL=postgres://postgres:postgres@localhost:54217/postgres \
#     cargo test
version: "3.8"
services:
  pg14:
    image: postgres:14
    environment: { POSTGRES_PASSWORD: postgres, POSTGRES_DB: postgres }
    ports: ["54214:5432"]
  pg15:
    image: postgres:15
    environment: { POSTGRES_PASSWORD: postgres, POSTGRES_DB: postgres }
    ports: ["54215:5432"]
  pg16:
    image: postgres:16
    environment: { POSTGRES_PASSWORD: postgres, POSTGRES_DB: postgres }
    ports: ["54216:5432"]
  pg17:
    image: postgres:17
    environment: { POSTGRES_PASSWORD: postgres, POSTGRES_DB: postgres }
    ports: ["54217:5432"]
```

- [ ] **Step 3: Replace direct `EphemeralPostgres::start` calls in conformance**

In `crates/pgevolve-conformance/src/`, find places that call `EphemeralPostgres::start` directly. Replace with `pgevolve_testkit::test_pg_backend::resolve()?.checkout(major).await?`.

- [ ] **Step 4: Smoke test**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -10
PGEVOLVE_TEST_PG_MODE=testcontainers cargo test -p pgevolve-conformance 2>&1 | tail -10
# Compose mode test (manual):
# docker compose -f dev/docker-compose.pg.yml up -d
# PGEVOLVE_TEST_PG_MODE=compose PGEVOLVE_TEST_PG_17_URL=postgres://... cargo test -p pgevolve-conformance 2>&1 | tail -10
```

Expected: both modes work. Compose-mode boot time should be measurably faster on second run.

- [ ] **Step 5: Commit**

```bash
git add -A crates/pgevolve-testkit/ crates/pgevolve-conformance/ dev/
git commit -m "$(cat <<'EOF'
test(testkit): TestPgBackend pluggability (T5)

Three backends: testcontainers (default, hermetic), compose (fast
local iteration via dev/docker-compose.pg.yml), dsn (managed PG /
restricted dev). Mode selected via PGEVOLVE_TEST_PG_MODE env var.
Conformance suite uses the trait via testkit::test_pg_backend::resolve.

dev/docker-compose.pg.yml ships PG 14/15/16/17 on stable ports.
Developers run `docker compose -f dev/docker-compose.pg.yml up -d`
once at session start and then iterate with compose mode.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T6: Coverage matrix to (capability × change-kind × major)

**Goal:** Extend `cargo xtask coverage --check` to gate on the full three-dimensional matrix. Per-version override fields (`[pg.expect]`, `[expect.plan.pg<N>]`) become legal in `fixture.toml`.

**Files:**
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (new fields)
- Create: `xtask/src/coverage.rs`
- Modify: `xtask/src/main.rs` (register subcommand)
- Modify: `docs/spec/*.md` (add `change_kinds:` per row)

**Load-bearing spec section:** §8.

- [ ] **Step 1: Add `[pg.expect]` and `[expect.plan.pg<N>]` to the schema**

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FixturePgExpect(pub std::collections::BTreeMap<String, String>);

// In FixturePg:
#[serde(default)]
pub expect: FixturePgExpect,
```

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectPlanPerPg(pub std::collections::BTreeMap<String, PerPgPlanOverride>);

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PerPgPlanOverride {
    pub steps: Option<usize>,
    #[serde(default)]
    pub rewrites_used: Vec<String>,
}

// In ExpectPlan:
#[serde(default, flatten)]
pub per_pg: ExpectPlanPerPg,
```

The `flatten` lets `[expect.plan.pg15]` deserialize naturally as a child of `expect.plan`.

> **Verification:** TOML keys like `pg15` under `[expect.plan]` may need to be expressed as `[expect.plan.pg15]` rather than as a flattened map. Adjust to whatever toml-rs accepts cleanly. If `flatten` doesn't work, use an explicit `BTreeMap<u32, PerPgPlanOverride>` field named `per_pg`.

- [ ] **Step 2: Have the runner apply per-version overrides**

When the runner constructs the fixture plan for a given PG major, it looks up `pg.expect.<major>` and `expect.plan.pg<major>` and overrides the defaults.

```rust
fn resolve_for_major(fixture: &Fixture, major: u32) -> FixturePlan {
    let pg_expect = fixture.pg.expect.0.get(&major.to_string()).map(String::as_str);
    if pg_expect == Some("skip") { return FixturePlan::Skip; }
    if pg_expect == Some("failure") { return FixturePlan::ExpectFailure; }

    let mut plan = fixture.expect.plan.clone();
    if let Some(override_) = fixture.expect.plan.per_pg.0.get(&format!("pg{major}")) {
        if let Some(s) = override_.steps { plan.steps = Some(s); }
        if !override_.rewrites_used.is_empty() { plan.rewrites_used = override_.rewrites_used.clone(); }
    }
    FixturePlan::Run(plan)
}
```

- [ ] **Step 3: Implement `xtask coverage`**

Create `xtask/src/coverage.rs`:

```rust
//! cargo xtask coverage [--check | --gaps]

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CapabilityRow {
    pub object: String,            // "Table", "Column", "Index", ...
    pub feature: String,           // free-text per docs/spec/*.md row
    pub change_kinds: Vec<String>, // populated by docs/spec parser
    pub status: String,            // "Implemented" | "Partial" | "Planned" | ...
}

pub fn run(mode: CoverageMode) -> Result<()> {
    let rows = parse_spec_rows()?;
    let fixtures = scan_fixtures()?;
    let matrix = build_matrix(&rows, &fixtures);
    match mode {
        CoverageMode::Check => check_matrix(&matrix),
        CoverageMode::Gaps => print_gaps(&matrix),
    }
}

pub enum CoverageMode { Check, Gaps }

fn parse_spec_rows() -> Result<Vec<CapabilityRow>> {
    use walkdir::WalkDir;
    let mut rows = Vec::new();
    for entry in WalkDir::new("docs/spec").into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") { continue; }
        let content = std::fs::read_to_string(path)?;
        for line in content.lines() {
            // Markdown table row format expected: `| <Object> | <status> | <description, possibly with change_kinds: [...]> |`
            if !line.starts_with('|') { continue; }
            let cells: Vec<_> = line.split('|').map(|c| c.trim()).collect();
            if cells.len() < 4 { continue; }
            let object_cell = cells[1];
            let status_cell = cells[2];
            let desc_cell = cells[3];
            // Skip header/separator rows: header begins with bare text like "Object";
            // separator is dashes.
            if object_cell.is_empty() || object_cell.contains("---") { continue; }
            if !is_known_status(status_cell) { continue; }
            let change_kinds = parse_change_kinds_annotation(desc_cell);
            rows.push(CapabilityRow {
                object: object_cell.trim_matches('`').to_string(),
                feature: object_cell.trim_matches('`').to_string(), // for v0.1 we treat object == feature
                change_kinds,
                status: status_cell.to_string(),
            });
        }
    }
    Ok(rows)
}

fn is_known_status(s: &str) -> bool {
    s.contains("Implemented") || s.contains("Partial")
        || s.contains("Planned") || s.contains("Future") || s.contains("Not planned")
}

fn parse_change_kinds_annotation(desc: &str) -> Vec<String> {
    // Look for `change_kinds: [a, b, c]` substring.
    let Some(start) = desc.find("change_kinds:") else { return Vec::new() };
    let after = &desc[start + "change_kinds:".len()..];
    let Some(open) = after.find('[') else { return Vec::new() };
    let Some(close) = after[open..].find(']') else { return Vec::new() };
    let inner = &after[open + 1 .. open + close];
    inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn scan_fixtures() -> Result<BTreeMap<(String, String, u32), String>> {
    use walkdir::WalkDir;
    use pgevolve_conformance::fixture::Fixture;
    let mut out = BTreeMap::new();
    let cases_root = Path::new("crates/pgevolve-conformance/tests/cases");
    for entry in WalkDir::new(cases_root).into_iter().filter_map(|e| e.ok()) {
        if entry.file_name() != "fixture.toml" { continue; }
        let dir = entry.path().parent().unwrap();
        let fixture = match Fixture::load(dir) { Ok(f) => f, Err(_) => continue };
        // The spec_refs field is "objects.column.add" style; first segment
        // is the object kind, last segment is the change kind. The middle
        // segments narrow the feature; we ignore them here.
        for sref in &fixture.meta.spec_refs {
            let segs: Vec<_> = sref.split('.').collect();
            if segs.len() < 2 { continue; }
            let object = segs[0].to_string();
            let change_kind = segs.last().unwrap().to_string();
            for major in fixture.pg.min..=fixture.pg.max {
                // Honor pg.expect = "skip" overrides — they don't count
                // as coverage for that major.
                let key = major.to_string();
                if fixture.pg.expect.0.get(&key).map(String::as_str) == Some("skip") {
                    continue;
                }
                out.insert(
                    (object.clone(), change_kind.clone(), major),
                    dir.display().to_string(),
                );
            }
        }
    }
    Ok(out)
}

fn build_matrix(
    rows: &[CapabilityRow],
    fixtures: &BTreeMap<(String, String, u32), String>,
) -> BTreeMap<(String, String, u32), Option<String>> {
    let mut matrix = BTreeMap::new();
    for row in rows {
        if !row.status.contains("Implemented") && !row.status.contains("Partial") {
            continue; // only Implemented / Partial entries gate coverage
        }
        for change in &row.change_kinds {
            for major in [14u32, 15, 16, 17] {
                let key = (row.object.clone(), change.clone(), major);
                let cell = fixtures.get(&key).cloned();
                matrix.insert(key, cell);
            }
        }
    }
    matrix
}

fn check_matrix(matrix: &BTreeMap<(String, String, u32), Option<String>>) -> Result<()> {
    let gaps: Vec<_> = matrix.iter().filter(|(_, v)| v.is_none()).collect();
    if gaps.is_empty() {
        println!("coverage: clean ({} cells covered)", matrix.len());
        return Ok(());
    }
    eprintln!("coverage: {} gap(s):", gaps.len());
    for ((obj, change, pg), _) in gaps {
        eprintln!("  - {obj} / {change} on PG {pg}");
    }
    anyhow::bail!("coverage gaps")
}

fn print_gaps(matrix: &BTreeMap<(String, String, u32), Option<String>>) -> Result<()> {
    for ((obj, change, pg), v) in matrix {
        if v.is_none() {
            println!("{obj}\t{change}\tpg{pg}");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Register the subcommand**

In `xtask/src/main.rs`:

```rust
// Add to the clap enum:
Coverage { #[arg(long)] check: bool, #[arg(long)] gaps: bool },

// In dispatch:
Subcmd::Coverage { check, gaps } => {
    let mode = if check { coverage::CoverageMode::Check }
               else if gaps { coverage::CoverageMode::Gaps }
               else { coverage::CoverageMode::Check };
    coverage::run(mode)?;
}
```

- [ ] **Step 5: Add `change_kinds:` to `docs/spec/*.md`**

Per the spec §8 change-kinds table, append `change_kinds:` to each Implemented / Partial row. E.g., `docs/spec/objects.md`'s `TABLE` row:

```markdown
| `TABLE` | ✅ Implemented | `CREATE / DROP / ALTER` for every v0.1 column / constraint operation. <br>change_kinds: [create, drop, alter, comment_on] |
```

Adapt the format to whatever fits naturally; the xtask parser reads the `change_kinds:` annotation.

- [ ] **Step 6: Run the coverage check**

```bash
cargo xtask coverage --check 2>&1 | tail -10
```

Expected: PASS for v0.1 surface (since all fixtures land under `objects/` and reference their spec rows). If GAP, either author the missing fixture or accept the gap as v0.2-pending (waiver mechanism not yet specified; track in an issue).

- [ ] **Step 7: Commit**

```bash
git add -A crates/pgevolve-conformance/ xtask/ docs/spec/
git commit -m "$(cat <<'EOF'
test(coverage): extend matrix to (capability × change-kind × major) (T6)

Per-version override fields land in fixture.toml: [pg.expect] declares
success/failure/skip per major, [expect.plan.pgN] overrides
structural fields. cargo xtask coverage --check gates PRs;
cargo xtask coverage --gaps prints the prioritized authoring queue.
docs/spec/*.md rows gain change_kinds: annotations the xtask
consumes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T7: Runtime budgets and `fixture-cost` xtask

**Goal:** Per-fixture budget (default 30s) and suite-total budget (5min no-Docker / 15min full) enforced as hard failures. `cargo xtask fixture-cost` reports per-fixture timings.

**Files:**
- Modify: `crates/pgevolve-conformance/src/fixture.rs` (add `[budget]`)
- Modify: `crates/pgevolve-conformance/tests/run.rs` (timing + budget enforcement)
- Create: `xtask/src/fixture_cost.rs`
- Modify: `xtask/src/main.rs`

**Load-bearing spec section:** §10.

- [ ] **Step 1: Add `[budget]` field**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct FixtureBudget {
    /// Per-fixture wall-clock cap in seconds. Default 30.
    #[serde(default = "default_budget_seconds")]
    pub seconds: u64,
}
fn default_budget_seconds() -> u64 { 30 }
impl Default for FixtureBudget { fn default() -> Self { Self { seconds: 30 } } }

// In Fixture:
#[serde(default)]
pub budget: FixtureBudget,
```

- [ ] **Step 2: Time and enforce**

In `tests/run.rs`, wrap each fixture in `tokio::time::timeout(Duration::from_secs(fixture.budget.seconds), ...)`. Capture wall-clock per fixture into a TSV at `target/conformance-timings.tsv`.

```rust
let start = std::time::Instant::now();
let result = tokio::time::timeout(
    std::time::Duration::from_secs(fixture.budget.seconds),
    run_fixture(&fixture),
).await;
let elapsed = start.elapsed();
append_timing(&fixture, elapsed);
match result {
    Ok(r) => r?,
    Err(_) => anyhow::bail!("fixture {} exceeded {}s budget", fixture.dir.display(), fixture.budget.seconds),
}
```

- [ ] **Step 3: Suite total budget**

In the same runner, accumulate elapsed time across fixtures. At end, assert `total < suite_budget`. The suite budget is 300s for the no-Docker path (set via env `PGEVOLVE_CONFORMANCE_SUITE_BUDGET_SECONDS`, default 300) and 900s for the full path.

- [ ] **Step 4: Implement `fixture-cost` xtask**

```rust
//! cargo xtask fixture-cost — per-fixture timing report.

use anyhow::Result;
use std::path::Path;

pub fn run() -> Result<()> {
    let path = Path::new("target/conformance-timings.tsv");
    if !path.exists() {
        anyhow::bail!("no timings file at {}; run `cargo test -p pgevolve-conformance` first", path.display());
    }
    let content = std::fs::read_to_string(path)?;
    let mut rows: Vec<(String, f64)> = content
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(2, '\t');
            let dir = parts.next()?;
            let secs: f64 = parts.next()?.parse().ok()?;
            Some((dir.to_string(), secs))
        })
        .collect();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    println!("top fixtures by wall-clock:");
    for (dir, secs) in rows.iter().take(20) {
        println!("  {:>6.2}s  {}", secs, dir);
    }
    let total: f64 = rows.iter().map(|r| r.1).sum();
    println!("---\n{:>6.2}s  total over {} fixtures", total, rows.len());
    Ok(())
}
```

Register subcommand in `xtask/src/main.rs`.

- [ ] **Step 5: Test**

```bash
cargo test -p pgevolve-conformance 2>&1 | tail -10
cargo xtask fixture-cost
```

Expected: green; `fixture-cost` prints a sorted list.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test(conformance): runtime budgets + fixture-cost xtask (T7)

Per-fixture budget (default 30s, configurable via [budget].seconds)
and suite-total budget (300s no-Docker, 900s full) enforced as
hard failures. Timings persisted to target/conformance-timings.tsv;
cargo xtask fixture-cost prints the top-20 slowest fixtures.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase T7.5: Capture-regression tooling

**Goal:** Four xtasks that close the property-test discovery → permanent-fixture loop. Nightly workflow auto-opens a draft PR scaffolding the regression fixture. PR CI fails on property-test issues older than 30 days.

**Files:**
- Create: `xtask/src/{capture_regression,verify_regression,property_status,diagnose_pg_version}.rs`
- Modify: `xtask/src/main.rs`
- Modify: `.github/workflows/property-tests.yml`
- Modify: `.github/workflows/ci.yml`

**Load-bearing spec section:** §11.

- [ ] **Step 1: Implement `capture-regression`**

```rust
//! cargo xtask capture-regression --seed <hex> --issue <n>

use anyhow::Result;
use std::path::Path;

pub fn run(seed_hex: &str, issue: u64) -> Result<()> {
    // 1. Find the proptest-regressions file holding the seed.
    let regr_dirs = vec![
        Path::new("crates/pgevolve-core/proptest-regressions"),
        Path::new("crates/pgevolve/proptest-regressions"),
    ];
    let mut hit = None;
    for d in &regr_dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            for entry in rd.flatten() {
                let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
                if content.contains(seed_hex) {
                    hit = Some((d.to_path_buf(), entry.path()));
                    break;
                }
            }
        }
    }
    let (_dir, _file) = hit.ok_or_else(|| anyhow::anyhow!("seed {seed_hex} not found in any proptest-regressions"))?;

    // 2. Re-run the proptest case to materialize the minimized (before, after)
    //    IR pair. The mechanism: a small helper in pgevolve-testkit that
    //    accepts a seed and returns the IR pair via the same arbitrary_catalog
    //    + arbitrary_mutation chain the property tests use.
    let (before_ir, after_ir) = pgevolve_testkit::replay_seed(seed_hex)?;

    // 3. Render each IR to SQL via the dump renderer.
    let before_sql = pgevolve_testkit::render_to_sql(&before_ir)?;
    let after_sql = pgevolve_testkit::render_to_sql(&after_ir)?;

    // 4. Scaffold the fixture.
    let slug = format!("issue-{issue}");
    let fixture_dir = Path::new("crates/pgevolve-conformance/tests/cases/regressions").join(&slug);
    std::fs::create_dir_all(&fixture_dir)?;
    std::fs::write(fixture_dir.join("before.sql"), before_sql)?;
    std::fs::write(fixture_dir.join("after.sql"), after_sql)?;
    std::fs::write(fixture_dir.join("fixture.toml"), format!(
        r#"[meta]
title = "regression: issue {issue}"
authoring = "regressions"
issue = "https://github.com/your-org/pgevolve/issues/{issue}"
spec_refs = []

[pg]
min = 14
max = 17

[expect.diff]
contains = []

[expect.plan]
steps = 0

# CAPTURED FROM PROPTEST SEED: {seed_hex}
# Edit this file to add specific assertions about the bug shape.
"#,
    ))?;

    println!("scaffolded {}", fixture_dir.display());
    println!("next: edit {0}/fixture.toml with specific assertions, then run `cargo xtask verify-regression {0}`",
        fixture_dir.display());
    Ok(())
}
```

> **Verification:** `pgevolve_testkit::replay_seed` and `render_to_sql` are new helpers. Implement them in `pgevolve-testkit` as part of this task; the property tests already use `arbitrary_catalog` and `arbitrary_mutation`, so `replay_seed` is a thin re-entry into those.

- [ ] **Step 2: Implement `verify-regression`**

```rust
pub fn run(fixture_dir: &Path) -> Result<()> {
    // Run the fixture through the same runner the test suite uses.
    // Assert it fails the expected layer; bail if it passes (would mean
    // the bug isn't actually captured).
    let fixture = pgevolve_conformance::fixture::Fixture::load(fixture_dir)?;
    let outcome = pgevolve_conformance::run_fixture_inline(&fixture);
    match outcome {
        Ok(()) => anyhow::bail!(
            "fixture {} passes; cannot capture as regression. \
             Either the bug is already fixed or the fixture doesn't exercise it.",
            fixture_dir.display(),
        ),
        Err(e) => {
            println!("verified: fixture fails as expected: {e}");
            Ok(())
        }
    }
}
```

- [ ] **Step 3: Implement `property-status`**

```rust
//! cargo xtask property-status [--max-age-days N]

pub fn run(max_age_days: u64) -> Result<()> {
    // Query GitHub for open issues with the `property-test-failure` label.
    // Fail if any is older than max_age_days.
    let issues = gh_list_issues("property-test-failure")?;
    let now = chrono::Utc::now();
    let mut stale = Vec::new();
    for i in &issues {
        let age = (now - i.created_at).num_days();
        let status = if age > max_age_days as i64 { "STALE" } else { "ok" };
        println!("{status:>5} #{:5} {} ({} days)", i.number, i.title, age);
        if status == "STALE" { stale.push(i.number); }
    }
    if !stale.is_empty() {
        anyhow::bail!("{} stale property-test issues exceed {}-day threshold", stale.len(), max_age_days);
    }
    Ok(())
}

fn gh_list_issues(_label: &str) -> Result<Vec<Issue>> {
    // Shell out to `gh issue list --label property-test-failure --state open --json number,title,createdAt`.
    let output = std::process::Command::new("gh")
        .args(["issue", "list", "--label", "property-test-failure", "--state", "open", "--json", "number,title,createdAt"])
        .output()?;
    let issues: Vec<Issue> = serde_json::from_slice(&output.stdout)?;
    Ok(issues)
}

#[derive(serde::Deserialize)]
struct Issue {
    number: u64,
    title: String,
    #[serde(rename = "createdAt")]
    created_at: chrono::DateTime<chrono::Utc>,
}
```

- [ ] **Step 4: Implement `diagnose-pg-version`**

```rust
//! cargo xtask diagnose-pg-version <fixture-dir> --pg-major N

pub fn run(fixture_dir: &Path, major: u32) -> Result<()> {
    // Run the fixture against the requested PG major and print per-layer
    // outcomes plus suggested fixture.toml edits.
    let fixture = pgevolve_conformance::fixture::Fixture::load(fixture_dir)?;
    let outcomes = pgevolve_conformance::run_fixture_inline_with_major(&fixture, major)?;
    println!("Fixture: {}", fixture_dir.display());
    for (layer, result) in outcomes.iter() {
        match result {
            Ok(()) => println!("  L{layer}: pass"),
            Err(e) => {
                println!("  L{layer}: FAIL — {e}");
                println!("    suggestion: {}", suggest_fix(*layer, e));
            }
        }
    }
    Ok(())
}

fn suggest_fix(layer: usize, err: &dyn std::fmt::Display) -> String {
    match layer {
        3 => "L3 plan-SQL golden mismatch — run `cargo xtask bless --conformance` if the change is intentional".to_string(),
        2 => "L2 structural mismatch — add `[expect.plan.pgN]` override or split fixture".to_string(),
        4 => format!("L4 apply failed ({err}) — add `[pg.expect].\"N\" = \"failure\"` if expected"),
        _ => "investigate manually".to_string(),
    }
}
```

- [ ] **Step 5: Register subcommands**

In `xtask/src/main.rs`, add the four subcommands.

- [ ] **Step 6: Wire the nightly workflow auto-PR**

In `.github/workflows/property-tests.yml`, append after the property-tests step:

```yaml
- name: Capture failures as regression PRs
  if: failure()
  run: |
    set -euo pipefail
    for regr in crates/*/proptest-regressions/*.txt; do
      seed=$(head -1 "$regr" | awk '{print $1}')
      issue=$(gh issue list --label property-test-failure --state open --json number --jq '.[0].number')
      [ -n "$issue" ] || continue
      cargo xtask capture-regression --seed "$seed" --issue "$issue"
      branch="capture/property-issue-$issue"
      git checkout -b "$branch"
      git add crates/pgevolve-conformance/tests/cases/regressions/
      git commit -m "test(regression): capture proptest failure for issue $issue"
      git push -u origin "$branch"
      gh pr create --draft --title "Capture property-test failure (issue $issue)" \
        --body "Auto-generated by property-tests workflow. Review fixture, fix underlying bug, then run \`cargo xtask verify-regression\` to confirm." \
        --base main
    done
```

- [ ] **Step 7: Wire the compliance gate in CI**

In `.github/workflows/ci.yml`, add a job:

```yaml
property-status:
  name: Property-test issue compliance
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: actions-rust-lang/setup-rust-toolchain@v1
    - run: cargo xtask property-status --max-age-days 30
      env:
        GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

Add `property-status` as a required check on `main` (repo settings — outside this plan; note in the commit message).

- [ ] **Step 8: Smoke test**

```bash
cargo xtask property-status --max-age-days 30
cargo xtask capture-regression --seed "deadbeef" --issue 0 2>&1 | tail -5
```

Expected: `property-status` runs (likely PASS — no property failures); `capture-regression` errors with "seed not found" (no proptest run has happened to produce the seed).

- [ ] **Step 9: Full suite check**

```bash
cargo test --workspace --lib --tests 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add xtask/ .github/workflows/
git commit -m "$(cat <<'EOF'
test(xtask): capture-regression tooling (T7.5)

Four new xtasks close the property-test → regression-fixture loop:
- capture-regression: proptest seed → scaffolded regression fixture
- verify-regression: assert fixture fails on the broken code path
- property-status: list open property-test issues + age check
- diagnose-pg-version: per-PG-major fixture diagnostic with suggested edits

Nightly property-tests.yml workflow auto-opens a draft PR with the
captured fixture. ci.yml gains a property-status compliance gate
that fails PRs when any property-test issue is older than 30 days.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final cleanup

- [ ] **Step 1: Spec coverage check**

Re-read `docs/superpowers/specs/2026-05-15-test-strategy-v2-design.md` and confirm each section has a phase:

| Spec section | Phase |
|---|---|
| §3 layer table | T1 (L5), T2 (L6), T2.5 (L8, L9), T3 (L7), T4 (failure) |
| §4 taxonomy | T0 |
| §4.1 dependency-chains | T2.5 (first fixture) |
| §5 layer details | T1, T2, T2.5, T3, T4 |
| §6 fixture.toml schema | T0, T2.5, T3, T6 |
| §7 runner changes | T0, all subsequent phases |
| §8 coverage matrix | T6 |
| §9 TestPgBackend | T5 |
| §10 budgets | T7 |
| §11 capture tooling | T7.5 |
| §12 authoring tooling summary | T6, T7, T7.5 |
| §13 CI integration | T6, T7.5 |
| §14 phasing | this plan's structure |

All sections covered.

- [ ] **Step 2: Run the full suite one last time**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests 2>&1 | tail -10
cargo xtask coverage --check
```

Expected: all green.

- [ ] **Step 3: Update README test tier table**

Update `README.md`'s test tier table to reflect:
- Tier C as the canonical gate.
- The five fixture subtrees.
- L1–L9 layer count.
- `PGEVOLVE_TEST_PG_MODE` env var.

Commit:

```bash
git add README.md
git commit -m "docs: update README test-tier table for v0.2 conformance expansions"
```

- [ ] **Step 4: Note the per-family fixture-authoring phase (T8)**

T8 (per-family fixture authoring waves) is the largest phase and follows the v0.2 sub-spec sequence. It's not part of *this* plan — each sub-spec brings its own fixture authoring. Open a tracking issue:

```bash
gh issue create --title "T8: Per-family conformance fixture authoring (v0.2 surface)" \
  --body "Per docs/superpowers/specs/2026-05-15-test-strategy-v2-design.md §14 T8, each v0.2 sub-spec (views, MVs, types, extensions, functions, triggers, partitioning, reloptions) brings its own conformance fixtures. T8 is the umbrella tracking issue; each sub-spec PR adds its own fixtures under objects/<family>/ and the matching scenarios/."
```
