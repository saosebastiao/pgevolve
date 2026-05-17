# CLI

The `pgevolve` binary's command surface, flags, output formats, exit
codes, and configuration schema.

See [`../README.md`](./README.md) for the status legend.

## Commands

| Command | Status | Notes |
|---|---|---|
| `pgevolve init [--dir <path>] [--force]` | ✅ Implemented | Scaffolds `pgevolve.toml`, `schema/`, `plans/`, and a `.gitignore` block. |
| `pgevolve lint` | ✅ Implemented | Runs universal lint rules + the configured layout-profile rules. Exits 1 on any error-severity finding. |
| `pgevolve validate` | ✅ Implemented | Parses source IR and runs lint. Exits 1 on any error finding. |
| `pgevolve validate --shadow` | ✅ Implemented | As above + round-trip through an ephemeral Postgres of the `[shadow].postgres_version`. |
| `pgevolve diff --db <env> [--url <dsn>] [--format human|json|sql]` | ✅ Implemented | Prints the change set. Always exits 0 (informational). |
| `pgevolve plan --db <env> [--url <dsn>] [-o <dir>]` | ✅ Implemented | Full pipeline; writes the plan directory. Output path defaults to `<plan_dir>/<YYYY-MM-DD>-<short-id>`. |
| `pgevolve apply <plan-dir> --db <env> [--url <dsn>] [--allow-different-target] [--allow-drift]` | ✅ Implemented | Executes a plan directory. See "Exit codes" below. |
| `pgevolve status --db <env> [--url <dsn>] [--apply-id <uuid>] [--limit <n>] [--format human|json]` | ✅ Implemented | Recent applies + per-step detail. |
| `pgevolve bootstrap --db <env> [--url <dsn>]` | ✅ Implemented | Explicit install/upgrade of the `pgevolve` metadata schema. (Other commands auto-bootstrap.) |
| `pgevolve dump --db <env> -o <dir>` | ✅ Implemented | Introspect a live DB and write `<dir>/schema.sql` containing `CREATE` statements for all managed schemas, tables, constraints, indexes, and sequences. Multi-file layout following `layout_profile` is deferred to v0.1.2+. Output does not include pgevolve source directives; add them manually before running `pgevolve lint`. |
| `pgevolve graph [--graph-format dot\|mermaid] [-o <path>] [--plan <dir>]` | ✅ Implemented | Render the source dep graph. Read-only. `--graph-format` (not `--format`; collides with global flag) defaults to `dot`. `--plan <dir>` is deferred. |
| `pgevolve doctor --db <env> [--url <dsn>]` | ✅ Implemented | Project health check: bootstrap status, NOT VALID constraints, INVALID indexes, source/catalog object counts, recent failed applies. |
| `pgevolve rewrite-table <qname> --db <env> --confirm-rewrite` | 🟡 Partial | CLI surface stable; implementation lands with v0.2 partitioning / column-type-change sub-spec. |
| `pgevolve fmt` | 🔮 Future | Rewrite source files into the configured layout. Lint identifies the violations; `fmt` would mechanically fix them. |
| `pgevolve check` | 🔮 Future | Alias for `lint && validate && plan --dry-run`. Pure convenience. |

## Global flags

| Flag | Status | Notes |
|---|---|---|
| `--config <path>` | ✅ Implemented | Defaults to `./pgevolve.toml`. |
| `--format human|json|sql` | ✅ Implemented | `sql` is only meaningful for `diff`. Default is `human`. |
| `-v` / `-vv` (verbosity) | ✅ Implemented | Bumps `tracing` filter to `debug` / `trace`. |
| `--quiet` | ✅ Implemented | Filter set to `error`. |
| `-h` / `--help` / `--version` (clap built-ins) | ✅ Implemented | |

## Shadow-validation flags (`plan`, `diff`, `validate`)

| Flag | Status | Notes |
|---|---|---|
| `--shadow-validate` | ✅ Implemented (scaffold) | Opt-in cross-check against the shadow Postgres. v0.1 is a no-op for body-bearing objects (none exist yet); v0.2 sub-specs deepen coverage. |
| `--shadow-strict` | ✅ Implemented (scaffold) | Requires `--shadow-validate`. Treats shadow mismatches as errors rather than warnings. |

## Output formats

| Format | Default for | Status | Notes |
|---|---|---|---|
| `human` | every command except `dump` | ✅ Implemented | Hierarchical text, color-on-tty when stdout is a TTY (color polish 🔮 Future). |
| `json` | optional everywhere | ✅ Implemented | Stable schema; every top-level object carries a `schema_version` field (📋 to be added uniformly in v0.1.1). |
| `sql` | `diff` only | ✅ Implemented | Naive ALTER SQL with no online rewrites — for code review only; users run `plan` for the applyable form. |

## Exit codes

Spec §13. Implemented in `commands::apply::run`; other commands follow
the same convention.

| Code | Meaning | Status |
|---|---|---|
| `0` | Success | ✅ Implemented |
| `1` | Lint or validation error (or any unmapped error) | ✅ Implemented |
| `2` | Drift or pre-flight mismatch (target identity, drift, unapproved intents) | ✅ Implemented |
| `3` | Apply error (lock held, step failed) | ✅ Implemented |
| `4` | Config or CLI input error | ✅ Implemented |

## Connection precedence

Mirrors `psql`. First non-empty source wins.

| Order | Source | Status | Notes |
|---|---|---|---|
| 1 | `--url <dsn>` CLI argument | ✅ Implemented | |
| 2 | `[environments.<env>].url` | ✅ Implemented | |
| 3 | `[environments.<env>].url_env` (env var name) | ✅ Implemented | |
| 4 | `PGEVOLVE_DATABASE_URL` env var | ✅ Implemented | |
| 5 | libpq env vars (`PGHOST`, `PGUSER`, `PGPASSWORD`, etc.) | ✅ Implemented | Implicit via `tokio_postgres::connect("")`. |
| 6 | `~/.pgpass` | ✅ Implemented (via libpq) | Same path libpq uses. |

## `pgevolve.toml` schema

| Section | Status | Notes |
|---|---|---|
| `[project]` (name, schema_dir, plan_dir, layout_profile) | ✅ Implemented | Required. |
| `[managed]` (schemas, ignore_objects) | ✅ Implemented | Empty `schemas` list means "lint doesn't enforce schema match"; the filter still applies. |
| `[planner]` (strategy) | ✅ Implemented | `atomic` or `online`. |
| `[planner.online_rewrites]` (per-rewrite switches) | ✅ Implemented | Four switches: `create_index_concurrent`, `fk_not_valid_then_validate`, `check_not_valid_then_validate`, `not_null_via_check_pattern`. |
| `[environments.<name>]` (url, url_env, strategy) | ✅ Implemented | Per-env strategy override. |
| `[shadow]` (backend, url, url_env, reset, extensions, postgres_version) | ✅ Implemented | Full schema: `backend = "auto"` (auto-select testcontainers or DSN); `url` / `url_env` for DSN override; `reset = "drop_schema_cascade"`; `extensions = ["pgcrypto"]`; `postgres_version = "17"`. |
| `[extensions]` (declared extensions and versions) | 📋 Planned, v0.2 | Lands with extension support. |
| `[grants]` (high-level grant tables) | 📋 Planned, v0.3 | Lands with roles + grants. |

### `[shadow]` block (full example)

```toml
[shadow]
backend = "auto"
url = "..."
url_env = "..."
reset = "drop_schema_cascade"
extensions = ["pgcrypto"]
postgres_version = "17"
```

## `intent.toml` schema

Beyond the `[[intent]]` rows written by the planner, `intent.toml` supports user-authored `[[lint_waiver]]` rows:

```toml
[[lint_waiver]]
rule = "column-position-drift"
target = "app.users"
reason = "..."
```

| Field | Required | Notes |
|---|---|---|
| `rule` | yes | Stable rule identifier (e.g., `column-position-drift`). Non-empty. |
| `target` | yes | Qualified object name the waiver applies to. Non-empty. |
| `reason` | no | Free-text explanation; encouraged for auditing. |

## Logging

| Aspect | Status | Notes |
|---|---|---|
| `tracing` + `tracing-subscriber` | ✅ Implemented | |
| `RUST_LOG` env var override | ✅ Implemented | |
| Structured fields on every span (`apply_id`, `step_no`, `qname`) | 🟡 Partial | The executor sets `apply_id`; richer per-step fields land alongside structured CLI JSON. |
| stderr-only log output, stdout reserved for data | ✅ Implemented | |
