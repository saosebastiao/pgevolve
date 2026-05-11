# Command reference

Per-command details with realistic invocations. The
[capability spec](../spec/cli.md) lists every flag with its
implementation status; this file shows them in context.

## Global flags

These apply to every subcommand.

| Flag | Effect |
|---|---|
| `--config <path>` | Read config from `<path>` instead of `./pgevolve.toml`. |
| `--format human|json|sql` | Output format. `sql` is only meaningful for `diff`. |
| `-v`, `-vv` | Increase log verbosity (info → debug → trace). Logs go to stderr. |
| `--quiet` | Errors only. |
| `-h`, `--help` | Per-command help. |
| `--version` | Print the binary version. |

## `pgevolve init`

Scaffolds a new project.

```
USAGE: pgevolve init [--dir <path>] [--force]
```

| Flag | Default | Effect |
|---|---|---|
| `--dir <path>` | `.` | Directory to initialize. |
| `--force` | off | Overwrite an existing `pgevolve.toml`. |

Creates: `pgevolve.toml`, `schema/`, `plans/`, and adds a `.gitignore`
section if one doesn't exist yet. Refuses to overwrite an existing
`pgevolve.toml` unless `--force`.

## `pgevolve lint`

```
USAGE: pgevolve lint
```

Parses the source tree, runs the universal rules and the configured
layout-profile rules, and prints any findings. Exit `0` on no errors;
exit `1` on any error-severity finding.

Layout violations look like:

```
error: [schema_mirror_path] table should be at `app/tables/users.sql`; found at `schema/oops/users.sql` (schema/oops/users.sql:1:1)
pgevolve lint: 1 finding(s), 1 error(s)
```

Universal rules (e.g., `closed_world_references`, `managed_schemas_match`)
are listed in the [spec](../spec/lint-and-layout.md).

## `pgevolve validate`

```
USAGE: pgevolve validate [--shadow]
```

Parses the source tree (subsumes `lint` parse).

| Flag | Effect |
|---|---|
| `--shadow` | Round-trip the IR through an ephemeral Postgres of the version named in `[shadow].postgres_version`. Requires Docker. |

Without `--shadow` the command reports parse success and 0 lint
findings. With `--shadow` it additionally:

1. Starts a `postgres:<major>-alpine` container.
2. Builds a plan from `(empty, source)` and applies it.
3. Introspects the shadow DB into a `Catalog`.
4. Diffs the source IR against the introspected IR.
5. Reports any divergences as `Finding`s on stderr.

Exit `0` on match; `1` on any divergence.

## `pgevolve diff`

```
USAGE: pgevolve diff --db <env> [--url <dsn>]
```

Prints the change set between the source IR and a live database.
Always exits `0`; this is informational.

| Flag | Effect |
|---|---|
| `--db <env>` | Environment name from `[environments.<env>]`. |
| `--url <dsn>` | Override the resolved DSN. |

Output formats (selected with the global `--format` flag):

- `human` (default) — one-line summary per change, indented details.
- `json` — the same `ChangeSet` serialized.
- `sql` — naive ALTER SQL with **no online rewrites**. For review only;
  run `plan` for the applyable form.

```sh
pgevolve diff --db dev
# 1 change(s):
#   - AlterTable
#       alter table app.users (1 op(s))
```

## `pgevolve plan`

```
USAGE: pgevolve plan --db <env> [--url <dsn>] [-o <dir>]
```

The full pipeline: parse → diff → order → rewrite → group →
`Plan::from_grouped` → `write_plan_dir`.

| Flag | Default | Effect |
|---|---|---|
| `--db <env>` | — (required) | Environment to plan against. |
| `--url <dsn>` | — | Override the resolved DSN. |
| `-o <dir>` | `<plan_dir>/<YYYY-MM-DD>-<short-id>` | Output directory. |

```sh
pgevolve plan --db dev
# Wrote plan abc1234567890123 to plans/2026-05-11-abc1234567890123 (1 group(s), 1 step(s), 0 intent(s))
```

If `diff` is empty, `plan` still writes a directory with zero groups —
useful for asserting "no changes" in CI.

## `pgevolve apply`

```
USAGE: pgevolve apply <plan-dir> --db <env> [--url <dsn>]
                                  [--allow-different-target] [--allow-drift]
```

Reads a plan directory and applies it.

| Argument / flag | Effect |
|---|---|
| `<plan-dir>` | Path to a directory previously written by `pgevolve plan`. |
| `--db <env>` | Environment to apply against. |
| `--url <dsn>` | Override the resolved DSN. |
| `--allow-different-target` | Skip the target-identity match check. Use only when you're intentionally re-targeting (e.g., applying a staging plan to dev for local testing). |
| `--allow-drift` | Skip the drift recheck. Use only when re-applying after intentional out-of-band changes. |

Exit codes (spec §13):

| Code | Cause |
|---|---|
| `0` | Success |
| `2` | Pre-flight mismatch (target-identity / drift / unapproved intent) |
| `3` | Apply error (lock held / step failed) |
| `1` | Anything else |

### Approval flow for destructive plans

When `plan` produces destructive intents, they're written to
`intent.toml` with `approved = false`. `apply` reads them and refuses
to run until they're flipped. See
[plan-format.md](./plan-format.md#approving-destructive-intents).

## `pgevolve status`

```
USAGE: pgevolve status --db <env> [--url <dsn>] [--apply-id <uuid>] [--limit <n>]
```

| Flag | Default | Effect |
|---|---|---|
| `--db <env>` | — (required) | |
| `--url <dsn>` | — | |
| `--apply-id <uuid>` | — | Print per-step detail for one specific apply. |
| `--limit <n>` | 10 | Cap on the recent-applies list. |

```sh
pgevolve status --db dev
# 3 recent apply/applies:
#   <uuid-1>  plan=abc1234567890123  status=succeeded  started=2026-05-11T18:00:00Z  finished=2026-05-11T18:00:03Z
#   …
```

With `--format json`, emits a serializable shape for automation.

## `pgevolve bootstrap`

```
USAGE: pgevolve bootstrap --db <env> [--url <dsn>]
```

Installs or upgrades the `pgevolve` metadata schema (the
`pgevolve.bootstrap_version`, `apply_log`, `plan_steps`, and `lock`
tables). Other commands auto-bootstrap, so this is mostly useful for
pre-bootstrapping a fresh DB before the first apply.

## `pgevolve dump` *(v0.1.x)*

```
USAGE: pgevolve dump --db <env> -o <dir>
```

Introspect a live database and write source-format SQL files in the
configured layout. **Not yet implemented in v0.1** — it needs an
IR→SQL emitter beyond the piecemeal helpers in
`pgevolve_core::plan::rewrite::sql`. Tracked for v0.1.x.

The intended use case is *adoption*: pointing `dump` at an existing
production database to produce a starting `schema/` tree.
