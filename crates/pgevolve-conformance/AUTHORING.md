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
the xtask docs in T7.5). The tool re-runs the proptest case to
materialize the minimized IR pair, renders both to SQL, and scaffolds
`regressions/issue-<n>/{fixture.toml, before.sql, after.sql}`.

## Adding a per-version override

Either use `[pg.expect].<N> = "failure"` (skip / fail per major) or
`[expect.plan.pg<N>]` (override individual structural fields). Cap at
one override per fixture; more than that is a fixture-split signal.
