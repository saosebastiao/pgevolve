# pgevolve — Test Strategy v2

- Status: draft, awaiting review (revision 2)
- Date: 2026-05-15
- Authors: Daniel Toone
- Scope: How pgevolve's tests prove that declarative changes produce
  minimal, repeatable plans and surface every data-loss scenario, across
  the v0.2 surface.
- Builds on: [`2026-05-11-conformance-test-suite-design.md`](./2026-05-11-conformance-test-suite-design.md)
  (existing Tier C design — this spec thickens it).
- Sibling spec:
  [`2026-05-15-v0.2-architecture-review-design.md`](./2026-05-15-v0.2-architecture-review-design.md)
  defines the failure-mode policy, AST-derived dep graph, and component
  surface this spec asserts against.

## 1. Motivation

The Tier C conformance suite was scaffolded as a single proof-of-life
fixture (`tables/add-column-nullable`) plus a four-layer runner. The
runner works. What's missing is everything that turns "the suite runs"
into "the suite is a test of pgevolve's contract."

Four user-stated goals:

1. **Comprehensive before/after coverage** — every documented capability
   has at least one fixture per supported PG major.
2. **Minimal-plan proofs** — declarative changes produce the smallest
   correct plan. No collateral churn.
3. **Data-loss surfacing** — every destructive change produces an
   `intent.toml` row the user must approve; the absence of approval
   blocks apply.
4. **Repeatable, testable results** — every plan / diff /
   apply-roundtrip output is byte-stable across runs and across
   machines.

This spec makes each goal a fixture-level assertion that the runner
enforces. The result: when conformance is green for capability X on PG
major Y, X's contract holds for Y.

## 2. Non-goals

- Replacing the existing Tier C design. This spec extends it; the
  scaffolding, the bless command, the fixture format, the assertion
  layers 1–4 all remain.
- Replacing property tests. They keep their nightly discovery role.
- Reaching 100% PG-feature coverage. Coverage is per-`docs/spec/` entry
  with status `Implemented` or `Partial`. Future / Not-planned entries
  do not gate.
- Forcing Docker on every developer. The arch spec's AST-first pivot
  removes Docker from the plan hot path; the test backend is pluggable
  (§9).

## 3. The four goals as assertion layers

The existing four layers prove different things; the user's four goals
plus the AST-derived dep graph require five more.

| Goal | Layer | What it asserts |
|---|---|---|
| Before/after coverage | L1 Diff | The diff over `before → after` includes every expected change substring. |
| Before/after coverage | L4 Apply roundtrip | Applying the plan to a real PG of the right major produces a state IR-equal to `after.sql`. |
| Repeatability | L3 Plan SQL golden | `plan.sql` byte-equals the per-PG golden after normalization. |
| Repeatability | L2 Plan structural | Step count and rewrite list match `expect.plan.*`. |
| **Minimality** | **L5 Minimality (new)** | Re-planning over the *post-apply* state vs `after.sql` produces an empty diff and an empty plan. |
| **Minimality** | **L6 No-collateral-damage (new)** | Every step's target is in `expect.plan.touches_only`. |
| **Data-loss surfacing** | **L7 Intent shape (new)** | Every entry in `expect.intent` appears in the generated `intent.toml` with the documented kind, target, and reason template. |
| **Dep-graph correctness** | **L8 Dep-graph golden (new)** | The AST-derived dep graph for the fixture state byte-equals `expected/dep-graph.dot`. |
| **Order correctness** | **L9 Topological-order assertion (new)** | Every declared partial order in `expect.plan.order` is respected by the emitted step sequence. |

Layers 5–9 are described in §5. Layers 1–4 are unchanged from the Tier C
spec.

## 4. Fixture taxonomy

The existing single `tables/add-column-nullable` directory tree
generalizes to five sibling trees with distinct authoring contracts:

```
crates/pgevolve-conformance/tests/cases/
├── objects/                # atomic: one feature, one change-kind, one fixture
│   ├── schemas/
│   ├── tables/
│   ├── columns/
│   ├── indexes/
│   ├── sequences/
│   ├── constraints/
│   ├── views/              # v0.2
│   ├── materialized_views/ # v0.2
│   ├── functions/          # v0.2
│   ├── procedures/         # v0.2
│   ├── triggers/           # v0.2
│   ├── types/              # v0.2 (enum / domain / composite)
│   ├── extensions/         # v0.2
│   └── partitions/         # v0.2
├── scenarios/              # combined-feature workflows
│   ├── dependency-chains/  # function → MV → view-style stacks (§4.1)
│   ├── rename-column-with-dependent-views/
│   ├── add-fk-cycle-across-three-tables/
│   ├── partition-attach-with-fk-bearing-children/
│   └── extension-upgrade-with-managed-objects/
├── intent/                 # data-loss / lint-waiver surface
│   ├── drop-column-requires-intent/
│   ├── drop-table-requires-intent/
│   ├── column-position-drift-requires-waiver/
│   └── extension-cascade-requires-intent/
├── failure/                # failure-mode policy enforcement
│   ├── parse/
│   ├── ast-resolution/
│   ├── cycle/              # body-derived dep cycle rejection
│   └── lint-at-plan/
└── regressions/            # captured from property-test failures
    └── issue-<n>-<slug>/
```

Each tree has a distinct authoring contract documented in
`crates/pgevolve-conformance/AUTHORING.md`:

- **`objects/<kind>/<change>/`** — exactly one capability × one
  change-kind. Single feature in `before.sql` / `after.sql`. L1–L5 all
  fire. L7 fires when the change is destructive.
- **`scenarios/`** — multi-feature workflows. L1–L5, L8, L9 fire; L6
  `touches_only` is *not* asserted (these are *meant* to touch
  multiple objects). L7 fires as applicable.
- **`intent/`** — primary contract is the intent or lint-waiver
  surface. L1, L7 fire in full; L4 applies the plan only after
  `[[intent]].approved = true` (and any `[[lint_waiver]]` rows the
  fixture declares) are set programmatically by the runner. Fixtures
  that exercise the lint-waiver success path live here, paired with a
  `failure/lint-at-plan/` sibling that exercises the no-waiver
  rejection.
- **`failure/`** — primary contract is *that pgevolve refuses*. L1
  asserts no diff is produced; L2/L3/L4 skipped; a new sub-assertion
  on stderr / exit code fires. See §5.4.
- **`regressions/`** — one-off captures. Same shape as `objects/`;
  references the originating issue in `fixture.toml`.

The runner already walks the tree and parameterizes per fixture. The
walker is extended (§7) to (a) recognize the new top-level subtrees,
(b) dispatch to the correct assertion layer set per subtree, and (c)
produce a stable per-fixture test name.

### 4.1 `scenarios/dependency-chains/` — enumerated concerns

Complex dep structures (view → MV → function chains) need targeted
fixtures because each tests a distinct concern the atomic suite can't
cover. The subtree contains at minimum these fixtures, one per
concern:

| Fixture | Concern | Layers |
|---|---|---|
| `linear-3-layer-create/` | Forward order: function → MV → view create in dependency order. | L1–L5, L8, L9 |
| `linear-3-layer-drop/` | Reverse order: dropping a stack drops view before MV before function. | L1–L5, L8, L9 |
| `function-signature-change-cascade/` | Function signature change forces MV recreation, which forces view recreation. All three appear in plan.sql by name; no CASCADE. | L1–L5, L8, L9 |
| `column-rename-cascade/` | Renaming a base-table column cascades through MV column projection and view column reference. | L1–L5, L7, L8, L9 |
| `wide-fan-out-no-collateral/` | Modifying one function in a fanout doesn't recreate sibling chains. L6 `touches_only` opt-in. | L1–L6, L8, L9 |
| `partial-apply-resume/` | Apply crashes between MV creation and view creation; re-plan from partial state finishes the chain. Requires `[expect.apply.abort_after_step]` machinery (new in T3.5; until then, the tier-5 `drift_recovery_property` property test covers this generically). | L1–L5, L8, L9 |

Cycle rejection lives in `failure/cycle/` (see §5.4), not here.

## 5. New assertion layers in detail

### 5.1 L5 — Minimality (default on, opt-out per fixture)

After L4 applies the plan and asserts post-apply IR equivalence:

1. Treat the just-applied DB as a new "before" — read its catalog via
   `pgevolve-core::catalog::read_catalog`.
2. Use the same `after.sql` source IR as the new "after."
3. Run `diff` + `plan` against the pair.
4. Assert `diff.is_empty()` AND `plan.groups.is_empty()`.

This catches a class of bug L4 doesn't: L4 proves the *post-image*
matches; L5 proves the planner thinks so too. A planner that produces
a noisy follow-up plan (e.g., emits a redundant `COMMENT ON COLUMN`
on every run) fails L5 even when L4 passes.

Opt-out: `expect.plan.minimality = false`. Document why in the fixture
title. The only legitimate opt-out is fixtures whose change is *itself*
"plan a no-op."

Run-cost: one extra in-process pipeline pass per fixture; no extra PG.

### 5.2 L6 — No-collateral-damage (opt-in per fixture)

```toml
[expect.plan]
touches_only = ["app.users", "app.users.email"]
```

The runner asserts that every step's primary target qname is in the
declared set. Targets are pulled from each `RawStep::target`. Targets
not in the set fail the layer.

Most `objects/` fixtures opt in. `scenarios/` fixtures generally do
not (their point is multi-target). `intent/` fixtures opt in.

Implementation: targets are already populated on `RawStep`; the layer
is one set-comparison per step.

### 5.3 L7 — Intent shape (always on for destructive fixtures)

```toml
[[expect.intent]]
kind   = "drop_column"
target = "app.users.legacy_id"
reason_contains = "removes column"
```

The layer reads the runner-generated `intent.toml` and asserts:

- `expect.intent[i].kind` ∈ generated intents.
- `expect.intent[i].target` matches some intent row's target.
- The generated intent's `reason` (auto-rendered by the planner)
  contains every substring in `reason_contains`.

Layer also asserts the *count* matches:
`generated_intents.len() == expect.intent.len()`. Generating more or
fewer intents than expected is a failure.

Layer is **mandatory** if `before → after` is destructive. The runner
detects destructiveness by inspecting the change set's
`Destructiveness::RequiresApproval[AndDataLossWarning]` tags. If
destructive and `expect.intent` is empty, that's a fixture-authoring
error and fails immediately with a clear message.

For *non*-destructive fixtures, `expect.intent` must be absent or
empty; non-empty `expect.intent` on a non-destructive change is also
a fixture-authoring error.

### 5.4 Failure-fixture assertions

`failure/` fixtures invert the contract. Each fixture has:

```toml
[expect.failure]
stage = "parse"             # "parse" | "ast_resolution" | "order" | "lint_at_plan"
error_code = 1              # exit code from CLI
stderr_contains = ["duplicate qname"]
```

L1–L9 are all skipped. The runner invokes the appropriate pipeline
stage (parse / parse + ast_resolution / parse + ast_resolution + order /
the full pipeline through plan) and asserts the failure shape. This is
the operational test of the arch-spec failure-mode policy
(Decision 13).

The `"order"` stage is the body-derived dep cycle case (arch Decision
8). Fixtures under `failure/cycle/` use this stage and assert
`PlanError::BodyCycle`-shaped messages with the affected node names.

### 5.5 L8 — Dep-graph golden (always on; opt-out for trivial fixtures)

The arch spec's AST-derived dep graph is a first-class artifact. L8
makes it a fixture artifact too.

For each fixture, the runner:

1. Builds the source IR (parse + AST resolution).
2. Calls the same `pgevolve graph --format=dot` rendering path the CLI
   exposes (arch Decision 21).
3. Byte-compares the output against `expected/dep-graph.dot` after
   normalization (sort edges; strip timestamps).
4. On mismatch, produces a unified diff and instructs the developer
   to run `cargo xtask bless --conformance` to regenerate.

This catches a class of regression L1–L7 don't: planner output on
*this* fixture can be correct while the dep-graph builder silently
drops or duplicates an edge that breaks related fixtures. The
graph-as-artifact gives us per-fixture coverage of the dep model.

Opt-out: `expect.dep_graph = false` for fixtures where the graph is
trivially obvious (single-table changes); avoids golden churn for no
testing gain.

### 5.6 L9 — Topological-order assertion (opt-in per fixture)

```toml
[expect.plan]
order = [
  "app.fns.make_summary < app.mvs.summary",
  "app.mvs.summary < app.views.summary_dashboard",
]
```

Each entry declares a partial-order edge over step targets: `A < B`
means "if both A and B appear as step targets, A must appear before
B in step order." The runner verifies every declared edge.

Distinct from L3 (byte-equal plan.sql golden) because:

- L3 fails on harmless re-orderings within the same tie group.
- L9 asserts only the deps that actually matter — robust under
  internal reorderings.
- L9 catches a *missing* dep edge that L3 wouldn't detect on this
  fixture but would on related fixtures.

L9 is opt-in because it's redundant on fixtures where L3 already
covers the order (single-group plans). It's most valuable on
multi-object scenarios.

## 6. `fixture.toml` schema additions

Extending the existing schema with the new fields. Defaults preserve
backward compatibility with the existing `add-column-nullable`
fixture.

```toml
[meta]
title       = "..."
spec_refs   = ["..."]
authoring   = "objects"     # objects | scenarios | intent | failure | regressions
                            # — drives which assertion layers apply
issue       = "..."         # if regression

[pg]
min = 14
max = 17

[pg.expect]                 # per-version expectation override
"14" = "failure"            # success | failure | skip; default = "success"
"15" = "success"
"16" = "success"
"17" = "success"

[intent]                    # what gets written into intent.toml
allow_data_loss = false

[planner]
strategy = "online"         # online | atomic

[expect.diff]
contains = [...]

[expect.plan]
steps         = 3
rewrites_used = ["..."]
golden        = "expected/plan.sql"
minimality    = true        # L5; default true; opt out for no-op fixtures
touches_only  = ["..."]     # L6; absent = layer skipped
order         = ["..."]     # L9; absent = layer skipped

# Per-version structural overrides for L2 — when a fixture supports
# multiple majors but produces different step counts per version.
[expect.plan.pg15]
steps         = 2
rewrites_used = ["merge_via_native"]

[expect.dep_graph]          # L8
enabled = true              # default true; opt out for trivial graphs
golden  = "expected/dep-graph.dot"

[[expect.intent]]           # L7; mandatory for destructive fixtures
kind            = "..."
target          = "..."
reason_contains = ["..."]

[expect.apply]
succeeds             = true
post_apply_equals_to = "after.sql"

[expect.failure]            # mutually exclusive with [expect.plan]/[expect.apply]
                            # for fixtures whose top-level contract is failure.
                            # For per-version failure (via [pg.expect]), the same
                            # block applies when the active major resolves to "failure".
stage           = "..."     # parse | ast_resolution | order | lint_at_plan
error_code      = 1
stderr_contains = ["..."]

[budget]
seconds = 30                # default 30; per-fixture wall-clock cap
```

`authoring` is the routing key: the runner uses it to decide which
layer set applies. Mismatch between `authoring` and presence /
absence of incompatible keys is a fixture-loading error.

`[pg.expect]` enables a single fixture to assert both halves of a
version-conditional feature: "PG 14 rejects this with error X; PG 15+
produces the documented plan and applies cleanly." Without it,
version-conditional features go untested for their rejection path.

`[expect.plan.pg<N>]` overrides specific structural assertion fields
on a per-version basis, saving a fixture split when only one value
differs.

## 7. Runner changes

`crates/pgevolve-conformance/src/walk.rs` (the existing walker):

1. Recognize the five new top-level subtrees.
2. Construct a `FixturePlan` per fixture: which layers to fire, in
   which order, with which inputs. Per-version overrides from
   `[pg.expect]` and `[expect.plan.pg<N>]` are applied at plan
   construction time.
3. Run each layer as a separate sub-test; failure in layer N does
   *not* skip layer N+1 unless N+1 depends on N's output (e.g., L5
   depends on L4's apply having happened).

`crates/pgevolve-conformance/src/assertions/` gains five new
modules: `minimality.rs`, `touches_only.rs`, `intent_shape.rs`,
`dep_graph.rs`, `topological_order.rs`.

`crates/pgevolve-conformance/src/failure.rs` (new) handles the
`failure/` subtree's invert-the-contract layer.

## 8. Coverage matrix

The existing C-suite design proposes `cargo xtask coverage --check`
gating on (capability × PG-major). Extend to (capability × change-kind
× PG-major).

Change-kinds per object family:

| Object family | Change-kinds tracked |
|---|---|
| Schema, sequence, extension | create, alter, drop, comment_on |
| Table | create, drop, alter (per column-level change-kind below), comment_on |
| Column | add, drop, set_not_null, drop_not_null, set_default, drop_default, change_type, change_collation, rename, set_comment |
| Constraint | add (per kind), drop, validate, set_deferrable, set_comment |
| Index | create, drop, recreate, set_comment |
| View | create, drop, replace_compatible, replace_incompatible, set_reloption, set_comment |
| MV | create, drop, replace, refresh_concurrent, refresh_plain, set_comment |
| Function / procedure | create, drop, replace_body, change_signature, set_comment |
| Trigger | create, drop, replace, enable, disable, set_comment |
| Type (enum) | create, drop, add_value, rename_value, set_comment |
| Type (domain / composite) | create, drop, alter, set_comment |
| Partition | attach, detach, drop, replace_key |

`docs/spec/<file>.md` gains a `change_kinds:` field per row listing
the supported change kinds; `cargo xtask coverage --check` matches the
fixture tree against the (capability × change-kind × major) cells and
fails if any cell is empty.

`cargo xtask coverage --gaps` prints the unfilled cells as a
prioritized authoring queue.

## 9. Test-PG backend pluggability

The arch spec's AST-first pivot removes Docker from the `plan` hot
path. Real Postgres is still needed for Tier 3 catalog round-trip
goldens, Tier C apply-roundtrip (L4), and the opt-in shadow-validate
cross-check. The test side mirrors the arch spec's `ShadowBackend`
trait with `TestPgBackend`:

```rust
pub trait TestPgBackend {
    fn checkout(&self, major: PgMajor) -> TestPgGuard;
    // Drop on TestPgGuard returns to pool or destroys, per backend.
}
```

Three implementations:

| Backend | Who uses it | Lifecycle | Isolation | Boot cost |
|---|---|---|---|---|
| `testcontainers` (default) | CI, dev with Docker | per-process pool | per-container | Per-test container boot, amortized via pool |
| `compose` | Dev fast-iteration | shared across runs via repo-shipped `dev/docker-compose.pg.yml` | per-schema reset | ~zero (containers already up) |
| `dsn` | CI on managed PG, restricted dev | external | per-database reset | ~zero (DB exists) |

Mode selection:

```bash
PGEVOLVE_TEST_PG_MODE=testcontainers    # default
PGEVOLVE_TEST_PG_MODE=compose           # uses dev/docker-compose.pg.yml hosts
PGEVOLVE_TEST_PG_MODE=dsn               # uses PGEVOLVE_TEST_PG_<MAJOR>_URL
PGEVOLVE_TEST_PG_POOL_MAX=4             # cap per major; same env var across backends
```

A `dev/docker-compose.pg.yml` is shipped at the repo root declaring
PG 14/15/16/17 on stable ports. Developers run `docker compose -f
dev/docker-compose.pg.yml up -d` once at session start and then
iterate with `PGEVOLVE_TEST_PG_MODE=compose cargo test`. Suite
runtime in `compose` mode is dominated by SQL execution, not
container boot — meaningfully faster than testcontainers.

Reset semantics:

- `testcontainers`: destroy and recreate on guard drop (slow but
  hermetic) OR `drop_schema_cascade` for fast pool reuse (config
  toggle).
- `compose`: `drop_schema_cascade` on guard drop; if it fails, the
  container is marked dirty and the pool boots a replacement.
- `dsn`: `drop_database_create` (separate management database) OR
  `drop_schema_cascade`, configurable.

CI keeps `testcontainers` as the default for isolation. Developers
choose the mode that matches their environment.

## 10. Runtime budget

Two budgets, both CI-gated:

- **Per-fixture wall-clock: 30s** by default. Configurable in
  `fixture.toml` via `[budget].seconds = 60` for inherently slow
  fixtures (e.g., the rewrite-table fixture, which copies rows). Hard
  failure on overrun.
- **Suite total: 5 min** for the no-Docker path, **15 min** for the
  full path including L4 apply. Hard failure on overrun.

`cargo xtask fixture-cost` produces a per-fixture timing report from
a recent CI run; surfaces the top-N slowest fixtures for review.

The budget is the back-pressure mechanism that prevents fixture-bloat
from silently slowing CI.

## 11. Property tests and discovery → regression flow

Unchanged from the existing C-suite design in policy: property tests
stay `#[ignore]`, run nightly, captured failures become permanent
regression fixtures. **The tooling that makes this loop reliable is
new in this spec.**

### 11.1 New property

- **`plan_minimality_under_no_op_mutations`** — for a random catalog
  `C`, planning `C → C'` where `C' = C` produces an empty plan. Pure;
  no Docker. The property version of L5.

### 11.2 Capture tooling

Four new xtasks close the loop from "property test fails overnight"
to "permanent regression fixture in conformance":

| Command | Purpose |
|---|---|
| `cargo xtask capture-regression --seed <hex> --issue <n>` | Reads proptest's persistence file, re-runs the case to materialize the minimized IR pair, renders both to SQL via the same path `pgevolve dump` uses, scaffolds `regressions/issue-<n>/{fixture.toml, before.sql, after.sql}`. Defaults `[meta].issue` to the GitHub issue URL. |
| `cargo xtask verify-regression <fixture>` | Runs the fixture against current `main`; asserts the expected layer *fails*. Refuses to mark a fixture as "captured" if it passes on broken code — prevents capturing noise. |
| `cargo xtask property-status` | Lists open property-test issues and their associated regression fixtures; emits warning for any property test failing >7 days without a capture. |
| `cargo xtask diagnose-pg-version <fixture-dir> --pg-major N` | Runs the fixture against PG major N and reports per-layer outcomes plus suggested `fixture.toml` edits ("L3 mismatch — bless?", "L2 step count differs — add `[expect.plan.pgN]`?", "L4 apply failed — add `[pg.expect].\"N\" = \"failure\"`?"). The runtime counterpart to the per-major delta docs the arch spec commits to. |

### 11.3 Nightly workflow hook

The `property-tests.yml` workflow gains a post-failure step that
invokes `capture-regression` automatically and opens a draft PR with
the scaffolded fixture + the proptest seed. Maintainer reviews,
fixes the underlying bug, the verify step turns the fixture green,
the PR merges. **Capture-to-regression is one human-decision step,
not five.**

### 11.4 Compliance gate

`cargo xtask property-status --max-age-days 30` runs in PR CI and
fails the PR if any property-test issue has been open longer than
the threshold. Prevents permanent open-property-failure backlog.

## 12. Authoring tooling summary

`cargo xtask` after this spec:

- `bless --conformance` (exists) — regenerate plan-SQL and dep-graph
  goldens for failing L3 / L8.
- `new-fixture --object <kind> --change <change-kind> [--pg-min N] [--pg-max N]`
  — scaffold a fixture directory with placeholder `before.sql`,
  `after.sql`, and `fixture.toml`. The placeholders include
  reminders to fill in `expect.intent` if destructive.
- `coverage --check` (exists) — fail-on-gap CI gate.
- `coverage --gaps` (new) — print the prioritized gap list.
- `fixture-cost` (new) — per-fixture timing report.
- `capture-regression`, `verify-regression`, `property-status`,
  `diagnose-pg-version` (§11.2).

`AUTHORING.md` in `crates/pgevolve-conformance/` (new) documents:

- Each authoring tree's contract.
- The minimum fields per `authoring` kind.
- When to bless vs investigate.
- How to capture a property-test failure as a regression fixture.
- How to add a per-version override.

## 13. CI integration

`ci.yml` after this spec:

| Job | Tier | Trigger |
|---|---|---|
| `fmt-clippy` | n/a | push, PR |
| `tier-1-2` | 1, 2 | push, PR |
| `tier-3-conformance` | 3, C, L1–L3, L5, L7–L9 (no Docker for these) | push, PR |
| `tier-c-apply` | C L4 (apply roundtrip), gated on Docker availability | PR, blocking |
| `coverage-check` | `cargo xtask coverage --check` | PR, blocking |
| `property-status` | `cargo xtask property-status --max-age-days 30` | PR, blocking |
| `fixture-cost` | report-only on PR | PR, advisory |

L1–L3, L5, L7–L9 run without Docker because the AST-first pivot makes
plan generation reproducible without a container. L4 still needs real
PG; the `tier-c-apply` job uses `TestPgBackend = testcontainers` and
gates on Docker availability (skips cleanly on Docker outage).

`property-tests.yml` (nightly) — Tier 5 + capture-regression post-step.

`soak.yml` (weekly) — high-case property runs across all PG majors.

## 14. Phasing

| Phase | Deliverable | Gate |
|---|---|---|
| T0 | Authoring tree split (`objects/`, `scenarios/`, `intent/`, `failure/`, `regressions/`); the existing fixture moves to `objects/tables/add-column-nullable/`. | Runner walks all five trees; suite green. |
| T1 | L5 minimality layer; opt-out semantics; new property test. | The existing fixture passes L5; an artificial "broken planner" stub fails it. |
| T2 | L6 no-collateral-damage layer. | One new `objects/` fixture covering a multi-target change confirms the assertion fires correctly when violated. |
| T2.5 | L8 dep-graph golden + L9 topological-order layers; bless support for dep-graph. | First `scenarios/dependency-chains/` fixture (`linear-3-layer-create`) passes both layers; an injected dep-graph regression fails L8. |
| T3 | L7 intent-shape layer; mandatory-on-destructive enforcement. | First `intent/` fixture (`drop-column-requires-intent`) lands and is green; a sibling negative fixture confirms missing-intent is caught. |
| T4 | `failure/` subtree + failure-fixture layer (including `ast_resolution`, `order`, `lint_at_plan` stages). | One fixture per failure stage lands green. |
| T5 | `TestPgBackend` pluggability (testcontainers + compose + dsn); `dev/docker-compose.pg.yml` shipped; pool implementation. | Suite runs cleanly in all three modes; `compose` mode runtime is measurably faster than testcontainers on a representative fixture set. |
| T6 | Coverage matrix extended to (capability × change-kind × major); `coverage --gaps` and `coverage --check` updated. Per-version override fields (`[pg.expect]`, `[expect.plan.pg<N>]`) implemented. | `--check` clean for v0.1 capabilities. |
| T7 | Runtime budgets enforced; `fixture-cost` xtask. | Budget overruns hard-fail in CI. |
| T7.5 | Capture tooling: `capture-regression`, `verify-regression`, `property-status`, `diagnose-pg-version`. Nightly workflow hook for automatic capture-PR opening. Property-status compliance gate in PR CI. | One captured regression (real or synthetic) flows through the full automation loop. |
| T8 | Per-family fixture authoring waves, one per v0.2 sub-spec. | `coverage --check` clean for each new family as its sub-spec lands. |

T0–T3 are sequential prerequisites; T2.5 lands after T2 because L8 builds on the same runner spine. T4–T7.5 can land in parallel; T8 follows the v0.2 sub-spec sequence from the arch spec §16.

T8 is the largest phase and is the natural fit for parallel
subagent-driven authoring (mirrors the C-suite design's C4 phase).

## 15. Risks

- **Authoring cost.** Same risk the existing C-suite design called out;
  this spec doubles down with more layers. Mitigated by `new-fixture`
  scaffolding, by `coverage --gaps` prioritization, and by the
  pluggable test backend keeping suite runtime fast in dev.
- **L5 false positives from non-determinism elsewhere.** If a planner
  emits a `COMMENT ON` with a wallclock timestamp, L5 trips on every
  run. The C-suite's C0 determinism audit covers this for v0.1; this
  spec re-runs the audit per v0.2 family before its fixtures author.
- **L7 reason-string brittleness.** Asserting on `reason_contains`
  ties fixtures to planner reason templates. Mitigated by keeping
  templates stable (treated as part of the public contract) and by
  matching on substrings, not exact strings.
- **L8 churn under canonicalizer changes.** Any AST canonicalizer
  change (per arch Decision 10) re-blesses every dep-graph golden.
  Mitigated by treating canonicalizer version as part of the suite's
  bless boundary — a canonicalizer change is a single bless run, not
  per-fixture detective work.
- **`compose` mode state leaks.** `DROP SCHEMA CASCADE` doesn't reset
  cluster-wide state (extensions installed in `public`, custom GUCs).
  Mitigation: backend records cluster-wide changes and forces
  container replacement on detection. Documented in `AUTHORING.md`.
- **Failure-fixture flakes if PG version changes error strings.**
  `stderr_contains` substrings should target stable text we control
  (pgevolve's own error messages), not Postgres's. Authoring guide
  enforces this.
- **Per-version override sprawl.** A fixture with `[expect.plan.pg14]`,
  `[expect.plan.pg15]`, `[expect.plan.pg16]`, `[expect.plan.pg17]`
  has slipped past its purpose and should be four fixtures. The
  authoring guide caps overrides at one per fixture; more than that
  is a fixture-split signal.

## 16. Open questions

- **Should L5 also run after each property-test pass?** It would
  catch minimality regressions in the property layer too. Lean: yes,
  add to the property test that already round-trips. Decision
  deferred to T1 implementation.
- **Do `intent/` fixtures need L4 apply at all?** They prove the
  intent shape; the apply roundtrip mostly proves the SQL is valid.
  Lean: yes, keep L4 — proves "approved intent leads to successful
  apply." Decision finalized in T3.
- **Canonicalizer-version goldens.** When the AST canonicalizer
  (arch Decision 10) changes, do we re-bless all fixtures, or
  bake the canonicalizer version into golden paths? Lean: re-bless,
  with the bless commit tagged so reviewers can verify it's a
  canonicalizer change rather than a planner regression. Decision
  deferred to T2.5.
- **Should the conformance crate publish to crates.io?** Currently
  internal. As pgevolve gains third-party integrators, a published
  test harness would help. Out of scope for this spec; revisit at
  v1.0.
