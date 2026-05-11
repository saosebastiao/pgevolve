# Conformance Test Suite — Design

**Status:** approved 2026-05-11
**Author:** Daniel Toone
**Supersedes:** none

## Motivation

pgevolve currently relies on a mix of deterministic tier-2 fixtures, tier-3
catalog goldens, hand-rolled tier-4 CLI tests, and **non-deterministic tier-5
property tests** as CI gates. Property-test flakes have repeatedly blocked
merges and produced failure reports that don't reproduce locally.

We want a single deterministic test surface that:

1. **Is the only test gate in CI.** Non-deterministic tests are useful for
   *discovery* but never block a merge.
2. **Proves conformance per Postgres major.** When pgevolve claims support
   for a Postgres version, every documented feature has at least one fixture
   asserting end-to-end correctness against that version.
3. **Captures every property-test failure** as a permanent regression
   fixture so the same class of bug cannot regress unnoticed.

## Non-goals

- Replacing tier-3 catalog round-trip goldens (different invariant: catalog
  → IR fidelity; unchanged by this work).
- Removing property tests entirely. They remain, but only run locally and
  in the weekly soak workflow.
- Building a generic SQL-fixture format for use outside pgevolve.

## Architecture

A new test surface — **Tier C ("conformance")** — lives alongside existing
tiers as a dedicated workspace crate:

- **`crates/pgevolve-conformance/`** — new crate. Depends on
  `pgevolve-core`, `pgevolve` (for the `apply` path), and `pgevolve-testkit`
  (for `EphemeralPostgres`). Contains one fixture-walking integration test
  per assertion layer.
- **`crates/pgevolve-conformance/tests/cases/**`** — fixture tree, one
  directory per fixture.

### CI gates after this change

| Tier | Where | CI gate? | Determinism |
|------|-------|----------|-------------|
| 1 | unit tests in `src/` | yes | full |
| 2 | parser fixture corpus | yes | full |
| 3 | catalog round-trip goldens | yes | full |
| C | **conformance suite (new)** | **yes** | **full** |
| 4 | executor smoke / hand-rolled CLI e2e | folded into Tier C |
| 5 | property tests (pure + PG-bound) | **no — `#[ignore]` by default** | seeded |
| 7 | weekly soak | no — cron only | seeded, high case count |

Tier 5 stays in the tree but is gated behind `#[ignore]`. A new
`property-tests.yml` workflow runs them nightly with a high `PROPTEST_CASES`
budget. Failures open issues but do not block PRs.

### Discovery → regression flow

When a property test fails (locally, nightly, or in soak):

1. Capture the minimized IR pair (before/after) from the proptest output.
2. Translate to `before.sql` / `after.sql` and place under
   `crates/pgevolve-conformance/tests/cases/regressions/<issue-id>/`.
3. Author `fixture.toml` pointing at the upstream issue.
4. Fix the bug; the new fixture becomes a permanent CI gate against
   regression.

This is the *only* sanctioned path for adding regression coverage from
property-test failures.

## Fixture layout

```
crates/pgevolve-conformance/tests/cases/
  tables/
    add-column-not-null-default/
      fixture.toml
      before.sql
      after.sql
      expected/
        diff.txt              # canonical diff lines
        plan.sql              # default golden (normalized)
      per-pg/                 # optional version-specific overrides
        pg15/plan.sql
        pg17/plan.sql
  indexes/
  constraints/
  partitions/
  types/
  functions/
  views/
  ...
  regressions/
    issue-123-cascade-drop/
      ...
```

Rules:

- One leaf directory = one fixture.
- Directory name is the fixture ID; surfaces verbatim in failure messages.
- The runner is a single `#[test]` per assertion layer that walks the tree
  and parameterizes; each fixture is its own sub-case so failures point at
  a specific directory.

### `fixture.toml`

```toml
[meta]
title     = "ADD COLUMN ... NOT NULL DEFAULT — online rewrite"
spec_refs = ["objects.column.add", "rewrites.not_null_via_check"]
issue     = "https://github.com/.../issues/123"   # if regression

[pg]
min = 14            # inclusive; defaults to project-wide minimum if omitted
max = 17            # inclusive; defaults to project-wide maximum if omitted

[intent]            # written verbatim into intent.toml before plan
allow_data_loss = false

[planner]
strategy = "online"

[expect.diff]
contains = [
  "app.users.email: absent -> present",
]

[expect.plan]
steps         = 3
rewrites_used = ["not_null_via_check"]
golden        = "expected/plan.sql"     # default; null = opt out

[expect.apply]
succeeds             = true
post_apply_equals_to = "after.sql"
```

Defaults if a key is omitted:

- `pg.min` / `pg.max` — project-wide supported PG range.
- `intent` — empty (no data-loss waivers).
- `planner.strategy` — `"online"` (matches default config).
- `expect.plan.golden` — `"expected/plan.sql"`. **Plan-SQL goldens are
  on by default**; opt out only when output is genuinely environment-
  dependent.
- `expect.apply.succeeds` — `true`.
- `expect.apply.post_apply_equals_to` — `"after.sql"`.

### Per-PG overrides

When planner output legitimately diverges by Postgres major (e.g., a
rewrite uses syntax only available in PG15+), the fixture provides
`per-pg/pg<N>/plan.sql` files. The runner picks `per-pg/pg<N>/plan.sql` if
present for the active PG major, else falls back to `expected/plan.sql`.

The `[expect.plan]` assertions in `fixture.toml` (`steps`, `rewrites_used`)
must hold for every supported version; if structural expectations also
diverge by version, the fixture must be split into two fixtures with
disjoint `pg.min/max` ranges.

## Assertion layers

Each fixture asserts **all four layers** unless explicitly opted out. The
runner emits each layer as a separate test result.

### 1. Diff invariants (always on)

- Parse `before.sql` and `after.sql` into IRs.
- Compute the diff between them.
- For each substring in `expect.diff.contains`, assert it appears in the
  rendered diff output (same substring-match pattern as `parser_corpus`).
- Failure shows the full rendered diff with missing substrings highlighted.

### 2. Plan invariants (always on)

- Run the planner over the diff with the fixture's `[intent]` and
  `[planner]` config.
- Assert step count equals `expect.plan.steps`.
- Assert each entry in `expect.plan.rewrites_used` appears in the plan
  manifest's rewrite list (substring; rewrites have stable string IDs).
- Failure shows the generated plan with annotations.

### 3. Plan SQL golden (default on, opt-out)

- Render the plan's `plan.sql`.
- Apply a normalization pass: strip plan ID, timestamp, embedded hash,
  any `-- generated at` headers. Normalization is centralized in a
  helper in `pgevolve-conformance/src/normalize.rs`.
- Byte-equal compare against the golden (`per-pg/pg<N>/plan.sql` if
  present, else `expected/plan.sql`).
- On mismatch: produce a unified diff and instruct the developer to run
  `cargo xtask bless --conformance` to regenerate goldens.
- Opt out by setting `expect.plan.golden = false`. Document why in
  `fixture.toml`'s `[meta].title` or a comment.

### 4. Apply roundtrip (default on, docker-gated)

- Start an `EphemeralPostgres` of the active PG major (from the CI
  matrix; see "Version matrix" below).
- Bootstrap pgevolve, seed the DB by executing `before.sql` via psql or
  `tokio_postgres`.
- Write `after.sql` into a temp `schema/` and run
  `pgevolve plan --db <ephemeral>` followed by `pgevolve apply <plan> --db <ephemeral>`.
- Introspect the post-apply DB into an IR via the catalog reader.
- Parse `after.sql` into an IR via the parser.
- Assert IR-equal (existing `Catalog::canonical_eq`).
- Skipped cleanly when `docker info` fails (consistent with existing
  testkit behavior). Skips count as test failures in CI but pass locally;
  the conformance crate gates Docker presence at workflow level.

If `expect.apply.succeeds = false`, the runner asserts that **either
plan or apply** returns a non-zero exit and that the error message
contains every substring in `expect.apply.error_contains`.

## Version coverage

Goal: every supported Postgres major has documented, exhaustive feature
coverage in the conformance suite.

### Deliverables

- **`docs/spec/pg-versions/pg<N>.md`** (one per supported major: 14, 15,
  16, 17). Each lists changes from the previous major *relevant to
  pgevolve* (new syntax, deprecated behavior, semantic shifts in
  catalogs). Sourced from official Postgres release notes. This is the
  input plan for fixture authoring.
- **`docs/spec/coverage.md`** — generated by `cargo xtask coverage`.
  Rows are (object kind × feature) from `docs/spec/`, columns are PG
  majors, cells are fixture paths or `N/A`. The xtask walks the fixture
  tree, reads `spec_refs` and `pg.min/max`, and emits the table.
- **`docs/spec/` linkage** — each capability entry gains a `fixtures:`
  field listing the conformance fixtures that prove it. Populated and
  verified by the same xtask.
- **CI gate** — `cargo xtask coverage --check` fails if any capability
  marked "Implemented" for version N has no fixture covering version N.

### Authoring process (one-time)

1. For each supported major, write the delta doc (`pg<N>.md`).
2. For each entry in `docs/spec/` marked "Implemented" or "Partial",
   author the minimum set of fixtures to cover it across the supported
   range. Atomic fixtures preferred (one feature per fixture); a small
   `scenarios/` sub-tree holds combined-feature fixtures.
3. Run `cargo xtask coverage --check`. Iterate until clean.

## Determinism prerequisite

Plan-SQL goldens are useless if planner output is non-deterministic.
Before fixtures can be authored, the planner must be audited for sources
of non-determinism:

- `HashMap` / `HashSet` iteration that escapes into ordered output.
  Replace with `BTreeMap` or `IndexMap`, or impose an explicit sort.
- Floating-point timestamps or wallclock reads in any planner code path.
  Plan IDs already hash canonical inputs; verify no other sources leak.
- Hash-seed randomization across runs (`RandomState`). Audit `pgevolve-core`
  for any usage that affects output ordering.

This audit is **C0** below and is a prerequisite for everything else.

## Phasing

| Phase | Deliverable | Gate |
|-------|-------------|------|
| C0 | Determinism audit of planner output; fix any sources found. | `cargo test --workspace` produces byte-identical `plan.sql` across 100 consecutive runs on a sample input. |
| C1 | `pgevolve-conformance` crate scaffold; fixture runner; normalization helper; bless command in xtask. | Runner walks an empty fixture tree without panicking; `cargo test -p pgevolve-conformance` passes. |
| C2 | Hand-port the ~20 most valuable Tier-4 CLI scenarios into fixtures. Delete the Rust-coded duplicates. | All ported scenarios pass under the new runner against the CI matrix. |
| C3 | Author `docs/spec/pg-versions/pg{14,15,16,17}.md` from official release notes. | Doc review approval. |
| C4 | Fixture authoring pass: every `docs/spec/` "Implemented" capability has ≥1 fixture, with appropriate `pg.min/max`. | `cargo xtask coverage --check` passes. |
| C5 | Move property tests behind `#[ignore]`; create `property-tests.yml` workflow; remove property tests from required CI gates. | PR-required CI runs only Tiers 1, 2, 3, C. Nightly runs property + soak. |

C1 is the unblocker — once the runner exists, fixture authoring in C2
and C4 parallelizes well and is a natural fit for subagent-driven
execution.

## Open questions resolved during brainstorming

- **Replace property tests in CI?** Yes — fully. They remain in the
  tree, gated behind `#[ignore]`, run only locally and in a dedicated
  workflow.
- **Anchor fixtures to capabilities or version semantics?** Both, with
  version semantics as the primary axis. Each fixture declares its
  `spec_refs`; coverage is enforced per (capability × PG major).
- **Plan-SQL goldens — opt-in or default-on?** Default-on. The user's
  explicit preference is maximum visibility into planner output drift;
  bless churn is acceptable.
- **One crate or test directory?** Dedicated crate
  (`crates/pgevolve-conformance`) so the test surface has clean
  dependencies on `pgevolve` (for apply) and its own CI matrix.

## Risks

- **Authoring cost.** C4 is large — every supported feature × every
  supported major. Mitigate by ordering: start with the highest-risk
  surfaces (constraints, partitions, indexes) and iterate. The coverage
  CI gate flips on at C5, not at C4 start, so C4 can land incrementally.
- **Golden churn during planner work.** Default-on goldens mean any
  planner refactor produces a large diff. Mitigate by making `bless`
  fast and the diff format clean; review discipline around bless
  commits is essential.
- **Docker dependency in CI.** Conformance requires Docker for the
  apply-roundtrip layer. CI already depends on Docker for Tier 3, so
  this is not new exposure, but conformance is a much wider docker
  surface — workflow must handle Docker outages gracefully (re-queue
  rather than fail).
