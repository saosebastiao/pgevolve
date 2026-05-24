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
USAGE: pgevolve lint [--format human|json]
```

Parses the source tree, runs the universal rules and the configured
layout-profile rules, and prints any findings. Exit `0` on no errors;
exit `1` on any error-severity finding.

The `--format` flag is the top-level `pgevolve --format ...` flag and
must precede the subcommand. `--format sql` is rejected for `lint`
(only meaningful for `diff`).

### Human format (default)

Layout violations look like:

```
error: [schema_mirror_path] table should be at `app/tables/users.sql`; found at `schema/oops/users.sql` (schema/oops/users.sql:1:1)
pgevolve lint: 1 finding(s), 1 error(s)
```

### JSON format

`pgevolve --format json lint` emits a stable structured document:

```json
{
  "findings": [
    {
      "severity": "error",
      "rule": "schema_mirror_path",
      "message": "table should be at `app/tables/users.sql`; found at `schema/oops/users.sql`",
      "location": { "file": "schema/oops/users.sql", "line": 1, "column": 1 }
    }
  ],
  "total": 1,
  "errors": 1
}
```

Severity values are stringified (`"error"`, `"warning"`, `"lint-at-plan"`).
Findings without a known source location omit the `location` field.

Universal rules (e.g., `closed_world_references`, `managed_schemas_match`)
are listed in the [spec](../spec/lint-and-layout.md).

## `pgevolve validate`

```
USAGE: pgevolve validate [--shadow] [--shadow-validate] [--shadow-strict]
```

Parses the source tree (subsumes `lint` parse).

| Flag | Effect |
|---|---|
| `--shadow` | Round-trip the IR through an ephemeral Postgres of the version named in `[shadow].postgres_version`. Requires Docker. |
| `--shadow-validate` | Cross-check the source dep graph against `pg_depend` in a shadow Postgres. See [Shadow validation](#shadow-validation). |
| `--shadow-strict` | Promote shadow-validation warnings to errors. Requires `--shadow-validate`. |

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
USAGE: pgevolve diff --db <env> [--url <dsn>] [--shadow-validate] [--shadow-strict]
```

Prints the change set between the source IR and a live database.
Always exits `0`; this is informational.

| Flag | Effect |
|---|---|
| `--db <env>` | Environment name from `[environments.<env>]`. |
| `--url <dsn>` | Override the resolved DSN. |
| `--shadow-validate` | Cross-check the source dep graph against `pg_depend` in a shadow Postgres. See [Shadow validation](#shadow-validation). |
| `--shadow-strict` | Promote shadow-validation warnings to errors. Requires `--shadow-validate`. |

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
USAGE: pgevolve plan --db <env> [--url <dsn>] [-o <dir>] [--shadow-validate] [--shadow-strict]
```

The full pipeline: parse → diff → order → rewrite → group →
`Plan::from_grouped` → `write_plan_dir`.

| Flag | Default | Effect |
|---|---|---|
| `--db <env>` | — (required) | Environment to plan against. |
| `--url <dsn>` | — | Override the resolved DSN. |
| `-o <dir>` | `<plan_dir>/<YYYY-MM-DD>-<short-id>` | Output directory. |
| `--shadow-validate` | off | Cross-check the source dep graph against `pg_depend` in a shadow Postgres. See [Shadow validation](#shadow-validation). |
| `--shadow-strict` | off | Promote shadow-validation warnings to errors. Requires `--shadow-validate`. |

`plan` also enforces `LintAtPlan` findings: if any unwaived
`LintAtPlan`-severity finding is present (e.g., column-position drift),
the command exits `2` and writes no plan directory. Acknowledge findings
with `[[lint_waiver]]` rows in `intent.toml` — see
[configuration](./configuration.md#lint_waiver).

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

## `pgevolve dump`

```
USAGE: pgevolve dump --db <env> -o <dir>
```

Introspect a live database and write source-format SQL to `<dir>/schema.sql`.

| Flag | Default | Effect |
|---|---|---|
| `--db <env>` | — (required) | Environment name from `[environments.<env>]`. |
| `--url <dsn>` | — | Override the resolved DSN. |
| `-o, --output <dir>` | — (required) | Output directory. Created if it doesn't exist. |

The command:

1. Connects to the database using the resolved DSN.
2. Reads the catalog for all managed schemas (from `[managed].schemas`).
3. Renders every object as a `CREATE` statement in dependency order:
   schemas → tables (inline PK/UK/CHECK) → FK `ALTER TABLE ADD CONSTRAINT` →
   standalone indexes → sequences.
4. Writes the result to `<dir>/schema.sql`.

```sh
pgevolve dump --db dev -o /tmp/schema-snapshot
# wrote 4096 bytes to /tmp/schema-snapshot/schema.sql
# note: output does not include pgevolve directives; add them before running `pgevolve lint`
```

**Scope notes (v0.3.x):**

- The entire catalog is written to a single `schema.sql` file. Multi-file
  layout following `layout_profile` is not yet implemented.
- The output does **not** include pgevolve source directives
  (`-- pgevolve: intent = ...` etc.), so it cannot be fed directly to
  `pgevolve lint` or used with `parse_directory` without first adding those
  directives. After `dump`, add directives manually or use a future
  `pgevolve annotate` helper.
- Coverage: schemas, tables (with inline PK / UK / CHECK constraints and
  FK ALTERs), standalone indexes, sequences, views, and materialized
  views are emitted. Functions, procedures, triggers, user-defined
  types, extensions, and role-level state (owners, grants, RLS policies,
  reloptions) are **not** yet emitted by `dump`.

The primary use case is *adoption*: pointing `dump` at an existing production
database to produce a starting `schema/` tree for a new pgevolve project.

## `pgevolve graph`

```
USAGE: pgevolve graph [--graph-format dot|mermaid] [-o <path>] [--plan <dir>]
```

Render the source dependency graph. Read-only; no database connection
required.

| Flag | Default | Effect |
|---|---|---|
| `--graph-format dot\|mermaid` | `dot` | Output format. `dot` is Graphviz DOT; `mermaid` is Mermaid flowchart syntax. Note: named `--graph-format`, not `--format`, to avoid a clap collision with the global `--format` flag. |
| `-o, --out <path>` | stdout | Write output to a file instead of stdout. |
| `--plan <dir>` | — | Render the dep graph captured inside an existing plan directory. **Not yet implemented** — errors with "not yet implemented"; reserved for a future sub-spec. |

```sh
# DOT output to stdout (pipe to `dot -Tpng -o deps.png` for a diagram)
pgevolve graph

# Mermaid output to a file
pgevolve graph --graph-format mermaid -o schema/deps.md
```

Used by the conformance suite's L8 dep-graph golden layer: fixtures
assert byte-stable DOT output for a given source tree.

## `pgevolve doctor`

```
USAGE: pgevolve doctor --db <env> [--url <dsn>]
```

Project health check. Read-only; does not modify the database or write
any files.

| Flag | Effect |
|---|---|
| `--db <env>` | Environment name from `[environments.<env>]` (required). |
| `--url <dsn>` | Override the resolved DSN. |

Reports:

- Bootstrap status (whether the `pgevolve` schema is installed). If
  not installed, the report tells you to run `pgevolve bootstrap`.
- NOT VALID constraints in managed schemas (candidates for a follow-up
  `VALIDATE CONSTRAINT`).
- INVALID indexes in managed schemas (candidates for a follow-up
  `REINDEX CONCURRENTLY`).
- Source object count vs. catalog object count (quick sanity check
  for unexpected drift) — schemas, tables, indexes, sequences.
- Recent failed applies from `pgevolve.apply_log` (only when
  bootstrapped).

```sh
pgevolve doctor --db dev
# pgevolve doctor — env dev
#   bootstrap: ok
#   drift: none
#   source:  1 schemas, 4 tables, 3 indexes, 1 sequences
#   catalog: 1 schemas, 4 tables, 3 indexes, 1 sequences
#   recent applies: no failures
```

Exit codes:

- `0` — every check passes (bootstrap installed, no drift, no recent
  apply failures).
- `1` — any of: bootstrap missing, NOT VALID constraint, INVALID
  index, or a failed apply in the recent log. The specific issue is
  printed in the report; exit code lets the command be scripted into
  deploy pre-flights.

A `pgevolve.apply_log` query error is not counted as an issue (the
table may not exist on very old bootstrap versions); the message is
printed but the command still returns `0` for that signal alone.

## `pgevolve rewrite-table` *(CLI skeleton)*

```
USAGE: pgevolve rewrite-table <qname> --db <env> --confirm-rewrite
```

Destructive table rewrite. **Not yet implemented** — the CLI surface is
stable but the command currently errors with a not-yet-implemented
message. The implementation lands with the column-reorder sub-spec.

| Argument / flag | Effect |
|---|---|
| `<qname>` | Qualified table name to rewrite (e.g., `app.users`). |
| `--db <env>` | Environment to operate against (required). |
| `--confirm-rewrite` | Explicit confirmation flag — required to guard against accidental invocation (required). |

The intended use case is column-position reorder: when `pgevolve plan`
detects column-position drift and you have an approved
`[[lint_waiver]]` for the relevant `column-position-drift` finding,
this command performs the shadow-copy table rewrite to materialise the
new column order.

## `pgevolve cluster` *(v0.3.0+)*

Cluster-level commands manage state shared across an entire Postgres
cluster (today: roles; future: tablespaces, GUCs, foreign servers).
They use a parallel project type — a directory containing
`pgevolve-cluster.toml` and a `roles/` tree — that is separate from a
per-DB pgevolve project. See [`docs/spec/cluster.md`](../spec/cluster.md)
for the surface and the project shape.

```
USAGE: pgevolve cluster [--config <path>] <subcommand>

Subcommands:
  init [<path>]   Scaffold a new cluster project.
  diff            Show the diff between source roles and the live cluster.
  plan            Write a cluster plan directory under `cluster-plans/<id>/`.
  apply [<id>]    Apply a cluster plan. Defaults to the most recent.
  status          List cluster plans under `cluster-plans/`.
```

| Flag | Effect |
|---|---|
| `--config <path>` | Path to `pgevolve-cluster.toml`. Defaults to `./pgevolve-cluster.toml`. |

Notes:

- Cluster commands read `pgevolve-cluster.toml`, not `pgevolve.toml`.
- The connection DSN comes from `[connection].dsn` in
  `pgevolve-cluster.toml`. The role used must be able to read
  `pg_authid` (typically superuser).
- The `[bootstrap].roles` list names roles pgevolve treats as PG-owned
  and never diffs in or out (default `["postgres"]`; cloud Postgres
  typically needs additional entries, e.g.
  `["postgres", "cloudsqlsuperuser"]`).
- Passwords are **not** stored in source; set them out-of-band.
- v0.3.0 limitations: cluster apply does not yet read `intent.toml`
  to gate destructive role drops, does not take an advisory lock,
  and does not write to a per-DB-style apply log. Review
  `cluster-plans/<id>/plan.sql` before applying. See
  [`docs/spec/cluster.md`](../spec/cluster.md#known-limitations-v030).

## Shadow validation

`--shadow-validate` is an optional opt-in cross-check available on
`validate`, `diff`, and `plan`. When set, pgevolve boots a shadow
Postgres (using the `[shadow]` block in `pgevolve.toml`) and verifies
the source dependency graph against the `pg_depend` catalog view.
Discrepancies are reported as warnings; `--shadow-strict` promotes
them to errors (and requires `--shadow-validate`).

```sh
# Check dep graph consistency; warn on discrepancies
pgevolve validate --shadow-validate

# Same but fail on any discrepancy
pgevolve validate --shadow-validate --shadow-strict

# Shadow-validate during plan; fail if dep graph diverges
pgevolve plan --db dev --shadow-validate --shadow-strict
```

Shadow backend selection follows `[shadow].backend` in `pgevolve.toml`
(`auto` | `testcontainers` | `dsn`). See
[configuration](./configuration.md#shadow).
