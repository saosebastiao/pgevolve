# CLI

The `pgevolve` binary's command surface, flags, output formats, exit
codes, and configuration schema.

See [`../README.md`](./README.md) for the status legend.

## Commands

| Command | Status | Notes |
|---|---|---|
| `pgevolve init [--dir <path>] [--force]` | ✅ Implemented | Scaffolds `pgevolve.toml`, `schema/`, `plans/`, and a `.gitignore` block.<br>**Tests:** tier-4: `crates/pgevolve/tests/cli_e2e.rs::end_to_end_init_plan_apply_status` |
| `pgevolve lint` | ✅ Implemented | Runs universal lint rules + the configured layout-profile rules. Exits 1 on any error-severity finding.<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs` |
| `pgevolve validate` | ✅ Implemented | Parses source IR and runs lint. Exits 1 on any error finding.<br>**Tests:** tier-4: `crates/pgevolve/tests/cli_e2e.rs` |
| `pgevolve validate --shadow` | ✅ Implemented | As above + round-trip through an ephemeral Postgres of the `[shadow].postgres_version`.<br>**Tests:** tier-4: `crates/pgevolve/tests/shadow_validate.rs`, `shadow_validate_flag.rs`, `shadow_validate_views.rs` |
| `pgevolve diff --db <env> [--url <dsn>] [--format human|json|sql]` | ✅ Implemented | Prints the change set. Always exits 0 (informational).<br>**Tests:** tier-4: `crates/pgevolve/tests/cli_e2e.rs` |
| `pgevolve plan --db <env> [--url <dsn>] [-o <dir>]` | ✅ Implemented | Full pipeline; writes the plan directory. Output path defaults to `<plan_dir>/<YYYY-MM-DD>-<short-id>`.<br>**Tests:** tier-4: `crates/pgevolve/tests/api_build_plan.rs`, `cli_e2e.rs::end_to_end_init_plan_apply_status` |
| `pgevolve apply <plan-dir> --db <env> [--url <dsn>] [--allow-different-target] [--allow-drift]` | ✅ Implemented | Executes a plan directory. See "Exit codes" below.<br>**Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::apply_succeeds_end_to_end_and_persists_audit_rows`, `chaos_apply.rs` |
| `pgevolve status --db <env> [--url <dsn>] [--apply-id <uuid>] [--limit <n>] [--format human|json]` | ✅ Implemented | Recent applies + per-step detail.<br>**Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::status_queries_return_recent_apply_with_steps` |
| `pgevolve bootstrap --db <env> [--url <dsn>]` | ✅ Implemented | Explicit install/upgrade of the `pgevolve` metadata schema. (Other commands auto-bootstrap.)<br>**Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::bootstrap_is_idempotent` |
| `pgevolve dump --db <env> -o <dir>` | ✅ Implemented | Introspect a live DB and write `<dir>/schema.sql` containing `CREATE` statements for all managed schemas, tables, constraints, indexes, and sequences. Multi-file layout following `layout_profile` is deferred to v0.1.2+. Output does not include pgevolve source directives; add them manually before running `pgevolve lint`.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/dump_round_trip.rs` |
| `pgevolve graph [--graph-format dot\|mermaid] [-o <path>] [--plan <dir>]` | ✅ Implemented | Render the source dep graph. Read-only. `--graph-format` (not `--format`; collides with global flag) defaults to `dot`. `--plan <dir>` is deferred.<br>**Tests:** tier-4: `crates/pgevolve/tests/graph_command.rs` |
| `pgevolve doctor --db <env> [--url <dsn>]` | ✅ Implemented | Project health check: bootstrap status, NOT VALID constraints, INVALID indexes, source/catalog object counts, recent failed applies.<br>**Tests:** tier-4: `crates/pgevolve/tests/doctor_command.rs::doctor_help_includes_command` |
| `pgevolve rewrite-table <qname> --db <env> --confirm-rewrite` | 🟡 Partial | CLI surface stable; implementation lands with v0.2 partitioning / column-type-change sub-spec.<br>**Tests:** tier-4: `crates/pgevolve/tests/doctor_command.rs::rewrite_table_refuses_without_confirm_flag`, `rewrite_table_with_confirm_reports_not_yet_implemented` |
| `pgevolve fmt` | 🔮 Future | Rewrite source files into the configured layout. Lint identifies the violations; `fmt` would mechanically fix them. |
| `pgevolve check` | 🔮 Future | Alias for `lint && validate && plan --dry-run`. Pure convenience. |

**Tests (whole section):** tier-1: `crates/pgevolve/src/cli.rs::tests`; tier-4: `crates/pgevolve/tests/cli_e2e.rs::help_lists_all_nine_commands`.

## Global flags

| Flag | Status | Notes |
|---|---|---|
| `--config <path>` | ✅ Implemented | Defaults to `./pgevolve.toml`. <!-- TODO: no test located 2026-05-24 --> |
| `--format human|json|sql` | ✅ Implemented | `sql` is only meaningful for `diff`. Default is `human`.<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs::lint_default_format_is_human`, `lint_sql_format_is_rejected` |
| `-v` / `-vv` (verbosity) | ✅ Implemented | Bumps `tracing` filter to `debug` / `trace`. <!-- TODO: no test located 2026-05-24 --> |
| `--quiet` | ✅ Implemented | Filter set to `error`. <!-- TODO: no test located 2026-05-24 --> |
| `-h` / `--help` / `--version` (clap built-ins) | ✅ Implemented | **Tests:** tier-4: `crates/pgevolve/tests/cli_e2e.rs::help_lists_all_nine_commands` |

## Shadow-validation flags (`plan`, `diff`, `validate`)

| Flag | Status | Notes |
|---|---|---|
| `--shadow-validate` | ✅ Implemented (scaffold) | Opt-in cross-check against the shadow Postgres. v0.1 is a no-op for body-bearing objects (none exist yet); v0.2 sub-specs deepen coverage.<br>**Tests:** tier-4: `crates/pgevolve/tests/shadow_validate_flag.rs`, `shadow_validate_views.rs` |
| `--shadow-strict` | ✅ Implemented (scaffold) | Requires `--shadow-validate`. Treats shadow mismatches as errors rather than warnings.<br>**Tests:** tier-4: `crates/pgevolve/tests/shadow_validate.rs` |

## Output formats

| Format | Default for | Status | Notes |
|---|---|---|---|
| `human` | every command except `dump` | ✅ Implemented | Hierarchical text, color-on-tty when stdout is a TTY (color polish 🔮 Future).<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs::lint_default_format_is_human` |
| `json` | optional everywhere | ✅ Implemented | Stable schema; every top-level object carries a `schema_version` field (📋 to be added uniformly in v0.1.1).<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs::lint_json_format_emits_structured_output` |
| `sql` | `diff` only | ✅ Implemented | Naive ALTER SQL with no online rewrites — for code review only; users run `plan` for the applyable form.<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs::lint_sql_format_is_rejected` |

## Exit codes

Spec §13. Implemented in `commands::apply::run`; other commands follow
the same convention.

| Code | Meaning | Status | Tests |
|---|---|---|---|
| `0` | Success | ✅ Implemented | tier-4: `crates/pgevolve/tests/cli_e2e.rs::end_to_end_init_plan_apply_status` |
| `1` | Lint or validation error (or any unmapped error) | ✅ Implemented | tier-4: `crates/pgevolve/tests/lint_waiver_e2e.rs::plan_refuses_unwaived_column_position_drift` |
| `2` | Drift or pre-flight mismatch (target identity, drift, unapproved intents) | ✅ Implemented | tier-4: `crates/pgevolve/tests/executor_smoke.rs::apply_rejects_target_identity_mismatch` |
| `3` | Apply error (lock held, step failed) | ✅ Implemented | tier-4: `crates/pgevolve/tests/executor_smoke.rs::apply_rolls_back_transactional_group_on_failure` |
| `4` | Config or CLI input error | ✅ Implemented | tier-4: `crates/pgevolve/tests/doctor_command.rs::rewrite_table_refuses_without_confirm_flag` |

## Connection precedence

Mirrors `psql`. First non-empty source wins.

**Tests (whole section):** tier-1: `crates/pgevolve/src/connection.rs::tests`.

| Order | Source | Status | Notes |
|---|---|---|---|
| 1 | `--url <dsn>` CLI argument | ✅ Implemented | |
| 2 | `[environments.<env>].url` | ✅ Implemented | |
| 3 | `[environments.<env>].url_env` (env var name) | ✅ Implemented | |
| 4 | `PGEVOLVE_DATABASE_URL` env var | ✅ Implemented | |
| 5 | libpq env vars (`PGHOST`, `PGUSER`, `PGPASSWORD`, etc.) | ✅ Implemented | Implicit via `tokio_postgres::connect("")`. |
| 6 | `~/.pgpass` | ✅ Implemented (via libpq) | Same path libpq uses. |

## `pgevolve.toml` schema

**Tests (whole section):** tier-1: `crates/pgevolve/src/config.rs::tests`.

| Section | Status | Notes |
|---|---|---|
| `[project]` (name, schema_dir, plan_dir, layout_profile) | ✅ Implemented | Required. |
| `[managed]` (schemas, ignore_objects) | ✅ Implemented | Empty `schemas` list means "lint doesn't enforce schema match"; the filter still applies. |
| `[planner]` (strategy) | ✅ Implemented | `atomic` or `online`. |
| `[planner.online_rewrites]` (per-rewrite switches) | ✅ Implemented | Six switches: `create_index_concurrent`, `fk_not_valid_then_validate`, `check_not_valid_then_validate`, `not_null_via_check_pattern`, `refresh_mv_concurrently`, `view_drop_create_dependents`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/policy.rs::tests` |
| `[environments.<name>]` (url, url_env, strategy) | ✅ Implemented | Per-env strategy override. |
| `[shadow]` (backend, url, url_env, reset, extensions, postgres_version) | ✅ Implemented | Full schema: `backend = "auto"` (auto-select testcontainers or DSN); `url` / `url_env` for DSN override; `reset = "drop_schema_cascade"`; `extensions = ["pgcrypto"]`; `postgres_version = "17"`.<br>**Tests:** tier-4: `crates/pgevolve/tests/shadow_backend.rs` |
| `[extensions]` (declared extensions and versions) | 📋 Planned, v0.2 | Lands with extension support. |
| `[grants]` (high-level grant tables) | 📋 Planned, v0.3 | Lands with roles + grants. |

### `[planner.online_rewrites]` — v0.2 view / MV keys

| Key | Default | Notes |
|---|---|---|
| `refresh_mv_concurrently` | `true` | Upgrade `REFRESH MATERIALIZED VIEW` to `REFRESH MATERIALIZED VIEW CONCURRENTLY` when the MV has at least one unique index. Has no effect under `strategy = "atomic"`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/refresh_mv_concurrently.rs::tests`; tier-C: `objects/materialized_views/refresh-concurrently` |
| `view_drop_create_dependents` | `true` | When `true`, the planner walks the `body_dependencies` graph and emits explicit `DROP + CREATE` steps for every view transitively affected by an upstream change. When `false`, the planner errors instead of cascading dependent-view recreations — useful if you want to review every affected view manually.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/recreate_views.rs::tests`; tier-C: `scenarios/dependency-chains/view-on-view-column-drop` |

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

Beyond the `[[intent]]` rows written by the planner, `intent.toml` supports two user-authored table kinds: `[[lint_waiver]]` and `[[step_override]]`.

**Tests (whole section):** tier-4: `crates/pgevolve/tests/lint_waiver_e2e.rs::lint_waiver_survives_intent_toml_round_trip`, `plan_proceeds_with_matching_lint_waiver`; tier-1: `crates/pgevolve-core/src/plan/serialize.rs::tests`, `deserialize.rs::tests`.

### `[[lint_waiver]]`

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

### `[[step_override]]`

Step overrides allow suppressing or modifying individual planner steps. Useful when you want to skip, for example, a `refresh_materialized_view` step during a maintenance window.

```toml
[[step_override]]
kind = "refresh_materialized_view"
target = "app.daily_summary"
suppress = true
```

| Field | Required | Notes |
|---|---|---|
| `kind` | yes | Step kind to match (e.g., `refresh_materialized_view`). Must be a valid `StepKind` name. |
| `target` | yes | Qualified object name the override applies to. Non-empty. |
| `suppress` | no | Default `false`. When `true`, the matching step is omitted from the plan entirely. |

## Logging

| Aspect | Status | Notes |
|---|---|---|
| `tracing` + `tracing-subscriber` | ✅ Implemented | <!-- TODO: no test located 2026-05-24 --> |
| `RUST_LOG` env var override | ✅ Implemented | <!-- TODO: no test located 2026-05-24 --> |
| Structured fields on every span (`apply_id`, `step_no`, `qname`) | 🟡 Partial | The executor sets `apply_id`; richer per-step fields land alongside structured CLI JSON. <!-- TODO: no test located 2026-05-24 --> |
| stderr-only log output, stdout reserved for data | ✅ Implemented | <!-- TODO: no test located 2026-05-24 --> |
