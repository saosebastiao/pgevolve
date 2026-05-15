# pgevolve — Test Strategy v2

- Status: draft, awaiting review
- Date: 2026-05-15
- Authors: Daniel Toone
- Scope: How pgevolve's tests prove that declarative changes produce
  minimal, repeatable plans and surface every data-loss scenario, across
  the v0.2 surface.
- Builds on: [`2026-05-11-conformance-test-suite-design.md`](./2026-05-11-conformance-test-suite-design.md)
  (existing Tier C design — this spec thickens it).
- Sibling spec:
  [`2026-05-15-v0.2-architecture-review-design.md`](./2026-05-15-v0.2-architecture-review-design.md)
  defines the failure-mode policy and component surface this spec asserts
  against.

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
- Speeding up shadow PG boot. The runtime-budget §10 caps growth, but
  the underlying boot cost is what it is. We compensate with pooling.
- Reaching 100% PG-feature coverage. Coverage is per-`docs/spec/` entry
  with status `Implemented` or `Partial`. Future / Not-planned entries
  do not gate.

## 3. The four goals as assertion layers

The existing four layers prove different things; the user's four goals
require two more.

| Goal | Layer | What it asserts |
|---|---|---|
| Before/after coverage | L1 Diff | The diff over `before → after` includes every expected change substring. |
| Before/after coverage | L4 Apply roundtrip | Applying the plan to a real PG of the right major produces a state IR-equal to `after.sql`. |
| Repeatability | L3 Plan SQL golden | `plan.sql` byte-equals the per-PG golden after normalization. |
| Repeatability | L2 Plan structural | Step count and rewrite list match `expect.plan.*`. |
| **Minimality** | **L5 Minimality (new)** | Re-planning over the *post-apply* state vs `after.sql` produces an empty diff and an empty plan. |
| **Minimality** | **L6 No-collateral-damage (new)** | Every step's target is in `expect.plan.touches_only`. |
| **Data-loss surfacing** | **L7 Intent shape (new)** | Every entry in `expect.intent` appears in the generated `intent.toml` with the documented kind, target, and reason template. |

Layers 5–7 are described in §5. Layers 1–4 are unchanged from the Tier C
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
│   ├── shadow-load/
│   └── lint-at-plan/
└── regressions/            # captured from property-test failures
    └── issue-<n>-<slug>/
```

Each tree has a distinct authoring contract documented in
`crates/pgevolve-conformance/AUTHORING.md`:

- **`objects/<kind>/<change>/`** — exactly one capability × one
  change-kind. Single feature in `before.sql` / `after.sql`. L1–L5 all
  fire. L7 fires when the change is destructive.
- **`scenarios/`** — multi-feature workflows. L1–L5 fire; L6
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
stage = "parse"             # "parse" | "shadow_load" | "lint_at_plan"
error_code = 1              # exit code from CLI
stderr_contains = ["duplicate qname"]
```

L1–L7 are all skipped. The runner invokes the appropriate pipeline
stage (parse / parse + shadow / parse + shadow + diff + plan) and
asserts the failure shape. This is the operational test of the
arch-spec failure-mode policy (Decision 13).

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

[[expect.intent]]           # L7; mandatory for destructive fixtures
kind            = "..."
target          = "..."
reason_contains = ["..."]

[expect.apply]
succeeds             = true
post_apply_equals_to = "after.sql"

[expect.failure]            # mutually exclusive with [expect.plan]/[expect.apply]
stage           = "..."     # parse | shadow_load | lint_at_plan
error_code      = 1
stderr_contains = ["..."]
```

`authoring` is the routing key: the runner uses it to decide which
layer set applies. Mismatch between `authoring` and presence /
absence of incompatible keys is a fixture-loading error.

## 7. Runner changes

`crates/pgevolve-conformance/src/walk.rs` (the existing walker):

1. Recognize the five new top-level subtrees.
2. Construct a `FixturePlan` per fixture: which layers to fire, in
   which order, with which inputs.
3. Run each layer as a separate sub-test; failure in layer N does
   *not* skip layer N+1 unless N+1 depends on N's output (e.g., L5
   depends on L4's apply having happened).

`crates/pgevolve-conformance/src/assertions/` gains three new
modules: `minimality.rs`, `touches_only.rs`, `intent_shape.rs`.

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

## 9. Shadow PG pooling

Per the arch spec, shadow PG is the canonical body normalizer for
every body-bearing object. Without pooling, every fixture that
touches a view / MV / function pays a container boot. Pooling is
required for the suite to stay under §10's budget.

`pgevolve-testkit::ShadowPool`:

```rust
pub struct ShadowPool {
    inner: HashMap<PgMajor, Vec<EphemeralPostgres>>,
    max_per_major: usize,        // default 1; bumped to 4 for parallel test runs
}

impl ShadowPool {
    pub fn checkout(&mut self, pg: PgMajor) -> ShadowGuard;
    // Drop on ShadowGuard returns to pool and resets state via DROP SCHEMA CASCADE.
}
```

Resets are best-effort; on reset failure, the container is destroyed
and a fresh one boots in its place. The conformance crate spawns one
`ShadowPool` per `cargo test` process; pool guards are
`Send` for parallel-test compatibility.

`PGEVOLVE_SHADOW_POOL_MAX` env var controls the per-major cap.

## 10. Runtime budget

Two budgets, both CI-gated:

- **Per-fixture wall-clock: 30s** by default. Configurable in
  `fixture.toml` via `[budget].seconds = 60` for inherently slow
  fixtures (e.g., the rewrite-table fixture, which copies rows). Hard
  failure on overrun.
- **Suite total: 5 min** for the no-Docker path, **15 min** for the
  full path including L4 apply + ShadowPool boots. Hard failure on
  overrun.

`cargo xtask fixture-cost` produces a per-fixture timing report from
a recent CI run; surfaces the top-N slowest fixtures for review.

The budget is the back-pressure mechanism that prevents fixture-bloat
from silently slowing CI.

## 11. Property tests

Unchanged from the existing C-suite design — restated for
completeness:

- Property tests stay in the tree, `#[ignore]` by default.
- A dedicated `property-tests.yml` workflow runs them nightly with a
  high `PROPTEST_CASES` budget.
- Failures open issues but do not block PRs.
- Every confirmed property-test failure is captured as a permanent
  regression fixture under `crates/pgevolve-conformance/tests/cases/regressions/`.

This spec adds one new property to the suite:

- **`plan_minimality_under_no_op_mutations`** — for a random catalog
  `C`, planning `C → C'` where `C' = C` produces an empty plan. Pure;
  no Docker.

It's the property version of L5 from §5.1. The corresponding fixtures
are the per-capability regression captures.

## 12. Authoring tooling

`cargo xtask` gains:

- `bless --conformance` (exists) — regenerate plan-SQL goldens for
  failing L3.
- `new-fixture --object <kind> --change <change-kind> [--pg-min N] [--pg-max N]`
  — scaffold a fixture directory with placeholder `before.sql`,
  `after.sql`, and `fixture.toml`. The placeholders include
  reminders to fill in `expect.intent` if destructive.
- `coverage --check` (exists) — fail-on-gap CI gate.
- `coverage --gaps` (new) — print the prioritized gap list.
- `fixture-cost` (new) — per-fixture timing report.

`AUTHORING.md` in `crates/pgevolve-conformance/` (new) documents:

- Each authoring tree's contract.
- The minimum fields per `authoring` kind.
- When to bless vs investigate.
- How to capture a property-test failure as a regression fixture.

## 13. CI integration

`ci.yml` after this spec:

| Job | Tier | Trigger |
|---|---|---|
| `fmt-clippy` | n/a | push, PR |
| `tier-1-2` | 1, 2 | push, PR |
| `tier-3-conformance` | 3, C, L1–L3 only (no Docker for L4) | push, PR |
| `tier-c-apply` | C L4 + L5 (post-apply re-plan) | PR, blocking |
| `coverage-check` | `cargo xtask coverage --check` | PR, blocking |
| `fixture-cost` | report-only on PR | PR, advisory |

`property-tests.yml` (nightly, unchanged) — Tier 5; failures open
issues.

`soak.yml` (weekly, unchanged) — high-case property runs across all
PG majors.

## 14. Phasing

| Phase | Deliverable | Gate |
|---|---|---|
| T0 | Authoring tree split (`objects/`, `scenarios/`, `intent/`, `failure/`, `regressions/`); the existing fixture moves to `objects/tables/add-column-nullable/`. | Runner walks all five trees; suite green. |
| T1 | L5 minimality layer; opt-out semantics; new property test. | The existing fixture passes L5; an artificial "broken planner" stub fails it. |
| T2 | L6 no-collateral-damage layer. | One new `objects/` fixture covering a multi-target change confirms the assertion fires correctly when violated. |
| T3 | L7 intent-shape layer; mandatory-on-destructive enforcement. | First `intent/` fixture (`drop-column-requires-intent`) lands and is green; a sibling negative fixture confirms missing-intent is caught. |
| T4 | `failure/` subtree + failure-fixture layer. | One fixture per failure stage (`parse`, `shadow_load`, `lint_at_plan`) lands green. |
| T5 | ShadowPool in `pgevolve-testkit`; conformance crate adopts it. | Suite runtime drops measurably on a representative views fixture set. |
| T6 | Coverage matrix extended to (capability × change-kind × major); `coverage --gaps` and `coverage --check` updated. | `--check` clean for v0.1 capabilities. |
| T7 | Runtime budgets enforced; `fixture-cost` xtask. | Budget overruns hard-fail in CI. |
| T8 | Per-family fixture authoring waves, one per v0.2 sub-spec. | `coverage --check` clean for each new family as its sub-spec lands. |

T0–T3 are sequential prerequisites; T4–T7 can land in parallel; T8
follows the v0.2 sub-spec sequence from the arch spec §16.

T8 is the largest phase and is the natural fit for parallel
subagent-driven authoring (mirrors the C-suite design's C4 phase).

## 15. Risks

- **Authoring cost.** Same risk the existing C-suite design called out;
  this spec doubles down with more layers. Mitigated by `new-fixture`
  scaffolding, by `coverage --gaps` prioritization, and by reusing
  shadow boots via the pool.
- **L5 false positives from non-determinism elsewhere.** If a planner
  emits a `COMMENT ON` with a wallclock timestamp, L5 trips on every
  run. The C-suite's C0 determinism audit covers this for v0.1; this
  spec re-runs the audit per v0.2 family before its fixtures author.
- **L7 reason-string brittleness.** Asserting on `reason_contains`
  ties fixtures to planner reason templates. Mitigated by keeping
  templates stable (treated as part of the public contract) and by
  matching on substrings, not exact strings.
- **Pool reset hides state leaks.** `DROP SCHEMA CASCADE` doesn't
  always reset cluster-wide state (e.g., extensions, custom
  collations). Mitigation: pool guard records cluster-wide changes
  and forces container replacement on any. Documented in
  `AUTHORING.md`.
- **Failure-fixture flakes if PG version changes error strings.**
  `stderr_contains` substrings should target stable text we control
  (pgevolve's own error messages), not Postgres's. Authoring guide
  enforces this.

## 16. Open questions

- **Should L5 also run after each property-test pass?** It would
  catch minimality regressions in the property layer too. Lean: yes,
  add to the property test that already round-trips. Decision
  deferred to T1 implementation.
- **Do `intent/` fixtures need L4 apply at all?** They prove the
  intent shape; the apply roundtrip mostly proves the SQL is valid.
  Lean: yes, keep L4 — proves "approved intent leads to successful
  apply." Decision finalized in T3.
- **Should the conformance crate publish to crates.io?** Currently
  internal. As pgevolve gains third-party integrators, a published
  test harness would help. Out of scope for this spec; revisit at
  v1.0.
