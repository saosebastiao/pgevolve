# pgevolve

Postgres-specific declarative schema management.

[![crates.io](https://img.shields.io/crates/v/pgevolve.svg)](https://crates.io/crates/pgevolve)
[![docs.rs](https://img.shields.io/docsrs/pgevolve-core)](https://docs.rs/pgevolve-core)
[![CI](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml/badge.svg)](https://github.com/saosebastiao/pgevolve/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`pgevolve` treats a directory of `CREATE`-style SQL files as the source of
truth for one or more Postgres schemas, introspects a live database to
derive its current state, and computes ordered, dependency-aware migration
plans that bring the database to the desired state. It refuses to lose
data unless explicitly authorized in a per-plan intent file.

Current release: **v0.3.8** (Postgres 14–18). See
[`CHANGELOG.md`](./CHANGELOG.md) for per-release detail.

## Install

From [crates.io](https://crates.io/crates/pgevolve) (recommended):

```sh
cargo install pgevolve
```

From source:

```sh
git clone https://github.com/saosebastiao/pgevolve
cd pgevolve
cargo install --path crates/pgevolve
```

`pgevolve` requires Rust 1.95+.

## Getting started

```sh
# 1. Scaffold a project (creates `pgevolve.toml`, `schema/`, `plans/`).
pgevolve init

# 2. Author your schema as CREATE-style SQL under `schema/`.
#    File layout is enforced by the `layout_profile` setting; the default
#    (`schema-mirror`) wants `schema/<schema>/<kind>/<name>.sql`.

# 3. Plan a migration against a configured environment.
pgevolve plan --db dev
#   wrote plans/2026-05-23-<short-id>/{plan.sql, intent.toml, manifest.toml}

# 4. Review the generated plan.sql + intent.toml in code review.

# 5. Apply it.
pgevolve apply plans/2026-05-23-<short-id> --db dev
```

Full walkthrough: [`docs/user/getting-started.md`](./docs/user/getting-started.md).

## CLI surface

| Command | What it does | Touches DB | Writes files |
|---|---|---|---|
| `pgevolve init` | Scaffold project files | no | yes |
| `pgevolve lint` | Run universal + layout-profile rules | no | no |
| `pgevolve validate` | Parse + build source IR; with `--shadow`, round-trip the IR through ephemeral Postgres; `--shadow-validate` cross-checks dep graph against `pg_depend` | shadow only | no |
| `pgevolve diff --db <env>` | Build source + catalog IR; print change set (`--format=human\|json\|sql`); `--shadow-validate` opt-in | read-only | no |
| `pgevolve plan --db <env> [-o <dir>]` | Full pipeline; write plan directory; gates on unwaived `LintAtPlan` findings; `--shadow-validate` opt-in | read-only | yes |
| `pgevolve apply <plan-dir> --db <env>` | Execute plan | read+write | no |
| `pgevolve status --db <env>` | Show recent applies and per-step state | read-only | no |
| `pgevolve dump --db <env> -o <dir>` | Introspect live DB and write source SQL | read-only | yes |
| `pgevolve bootstrap --db <env>` | Install/upgrade the `pgevolve` metadata schema | read+write | no |
| `pgevolve graph` | Render source dep graph (`--graph-format dot\|mermaid`); no DB required | no | optional (`-o`) |
| `pgevolve doctor --db <env>` | Health check: bootstrap status, NOT VALID constraints, INVALID indexes, object counts, recent failures | read-only | no |
| `pgevolve cluster …` | Cluster-level surface: plan/apply role + role-attribute changes (`CREATE ROLE`, `ALTER ROLE`, `GRANT role TO role`) | varies | varies |
| `pgevolve rewrite-table <qname> --db <env> --confirm-rewrite` | Destructive table rewrite — *skeleton; not yet implemented* | — | — |

Exit codes: `0` success, `1` lint/validation error, `2` drift or
pre-flight mismatch, `3` apply error, `4` config or CLI input error.
Full reference: [`docs/user/commands.md`](./docs/user/commands.md).

## Configuration

`pgevolve.toml`:

```toml
[project]
name           = "myapp"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "schema-mirror"          # also: kind-grouped | feature-grouped | free-form | <path-to-custom.toml>

[managed]
schemas        = ["app", "billing"]       # schemas under pgevolve's control
ignore_objects = ["app.legacy_etl_*"]     # qname or glob

[planner]
strategy = "online"                       # "atomic" | "online"

[planner.online_rewrites]
create_index_concurrent       = true
fk_not_valid_then_validate    = true
check_not_valid_then_validate = true
not_null_via_check_pattern    = true

[environments.dev]
url = "postgres://localhost/myapp_dev"

[environments.prod]
url_env  = "DATABASE_URL_PROD"            # read DSN from env var
strategy = "online"
```

Connection precedence (mirrors `psql`):
`--url` CLI flag → `[environments.<env>].url` →
`[environments.<env>].url_env` → `PGEVOLVE_DATABASE_URL` →
libpq env (`PGHOST`, `PGUSER`, ...).

Full reference: [`docs/user/configuration.md`](./docs/user/configuration.md).

## Highlights

- **Declarative source of truth.** `CREATE`-style SQL under `schema/`
  is the only source; the plan is always derived, never authored.
- **Lenient drift.** Cross-cutting state (owner, grants, RLS policies,
  reloptions) marked `None` in source is treated as *unmanaged* — the
  differ never reverts catalog values you haven't claimed.
  Per-feature `unmanaged-*` lints surface the drift instead.
- **Audited apply.** Every step lands in `pgevolve.apply_log` /
  `pgevolve.plan_steps` with success/failure/abort transitions; the
  executor refuses to apply a tampered plan (BLAKE3 plan-id +
  manifest-embedded pre-image catalog).
- **Online rewrites by default.** Concurrent index create/drop,
  `NOT VALID` + `VALIDATE` for FK and CHECK, the 4-step
  `SET NOT NULL` via CHECK pattern, `REFRESH MATERIALIZED VIEW
  CONCURRENTLY` upgrade. Opt out per-environment with
  `strategy = "atomic"`.
- **All actively-maintained PG majors.** PG 14, 15, 16, 17, 18 covered in
  CI on every push; per-version SQL paths in the catalog reader.
- **Conformance-driven.** ~210 fixture-based end-to-end tests gate
  CI. Every claimed capability has a fixture; see
  [`docs/spec/testing.md`](./docs/spec/testing.md).

## Documentation

- [`docs/user/`](./docs/user/) — **user guide**: installation, getting
  started, configuration reference, command reference, cookbook,
  troubleshooting, plan-format walkthrough.
- [`docs/system/`](./docs/system/) — **architecture and internals**:
  [architecture](./docs/system/architecture.md), IR, planner, executor.
- [`docs/spec/`](./docs/spec/) — **living capability catalogue**: every
  object kind, column type, constraint, index option, lint rule, CLI
  command, with implementation status. The authoritative answer to
  "does pgevolve do X?".
- [`docs/CONSTITUTION.md`](./docs/CONSTITUTION.md) — binding principles
  for the project (license, dependency policy, type-system rigor,
  release discipline). Worth reading once if you intend to contribute.
- [`docs/RELEASING.md`](./docs/RELEASING.md) — release runbook.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE),
at your option. All dependencies are permissive-licensed
(MIT/Apache-2.0/BSD-family/ISC/Unicode/CC0/Zlib); no copyleft. Policy
enforced by `cargo deny check` (see [`deny.toml`](./deny.toml)) in CI.
