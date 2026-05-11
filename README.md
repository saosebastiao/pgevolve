# pgevolve

Postgres-specific declarative schema management.

`pgevolve` treats a directory of `CREATE`-style SQL files as the source of
truth for one or more Postgres schemas, introspects a live database to
derive its current state, and computes ordered, dependency-aware migration
plans that bring the database to the desired state. It refuses to lose
data unless explicitly authorized in a per-plan intent file.

> **Status:** under active development. v0.1 is not yet released.

## Usage at a glance

```sh
# 1. Scaffold a project (creates `pgevolve.toml`, `schema/`, `plans/`).
pgevolve init

# 2. Author your schema as CREATE-style SQL under `schema/`.
#    File layout is enforced by the `layout_profile` setting; the default
#    (`schema-mirror`) wants `schema/<schema>/<kind>/<name>.sql`.

# 3. Plan a migration against a configured environment.
pgevolve plan --db dev
#   wrote plans/2026-05-12-<short-id>/{plan.sql, intent.toml, manifest.toml}

# 4. Review the generated plan.sql + intent.toml in code review.

# 5. Apply it.
pgevolve apply plans/2026-05-12-<short-id> --db dev
```

### CLI surface

| Command | What it does | Touches DB | Writes files |
|---|---|---|---|
| `pgevolve init` | Scaffold project files | no | yes |
| `pgevolve lint` | Run universal + layout-profile rules | no | no |
| `pgevolve validate` | Parse + build source IR; with `--shadow`, round-trip the IR through an ephemeral Postgres of the configured version and report any divergences | shadow only | no |
| `pgevolve diff --db <env>` | Build source + catalog IR; print change set (`--format=human|json|sql`) | read-only | no |
| `pgevolve plan --db <env> [-o <dir>]` | Full pipeline; write plan directory | read-only | yes |
| `pgevolve apply <plan-dir> --db <env>` | Execute plan | read+write | no |
| `pgevolve status --db <env>` | Show recent applies and per-step state | read-only | no |
| `pgevolve dump --db <env> -o <dir>` | Introspect live DB and write source SQL | read-only | yes — *v0.1.x* |
| `pgevolve bootstrap --db <env>` | Install/upgrade the `pgevolve` metadata schema | read+write | no |

Exit codes follow spec §13: `0` success, `1` lint/validation error, `2`
drift or pre-flight mismatch, `3` apply error, `4` config or CLI input
error.

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

## Documentation

- [`docs/spec/`](./docs/spec/) — living capability catalogue. Every object
  kind, column type, constraint kind, index option, CLI command, etc.
  with its implementation status (Implemented / Partial / Planned /
  Future / Not planned). Start here when you want to know whether
  pgevolve does / will do a given thing.
- [`docs/superpowers/specs/2026-05-09-pgevolve-design.md`](./docs/superpowers/specs/2026-05-09-pgevolve-design.md) — v0.1 design.
- [`docs/superpowers/plans/`](./docs/superpowers/plans/) — phase-by-phase
  implementation plans.

### v0.1 phase progress

| Phase | Title                        | Status   |
|-------|------------------------------|----------|
| 0     | Workspace                    | done     |
| 1     | IR                           | done     |
| 2     | Source parser                | done     |
| 3     | Catalog reader               | done     |
| 4     | Differ                       | done     |
| 5     | Planner                      | done     |
| 6     | Rewrites                     | done     |
| 7     | Plan format                  | done     |
| 8     | Executor                     | done     |
| 9     | CLI                          | done     |
| 10    | Linter                       | done     |
| 11    | Testkit                      | done     |
| 12    | Shadow                       | done     |

## Workspace layout

- `crates/pgevolve-core` — IR, parser, diff, planner, plan I/O, lint
  engine. Pure library; no I/O at the type level (the parser owns the only
  filesystem walk).
- `crates/pgevolve` — CLI binary + executor. Holds the `tokio_postgres`
  adapter, advisory lock, audit writers, and the apply loop.
- `crates/pgevolve-testkit` — internal test infra: `EphemeralPostgres`
  (testcontainers wrapper), `PgCatalogQuerier`, IR generator and mutator,
  equivalence asserter, migration-fixture loader.
- `xtask` — `cargo xtask bless` regenerates tier-3 catalog goldens.

## Test tiers

| Tier | Where | Runs | Needs Docker |
|------|-------|------|--------------|
| 1 | unit tests in `src/` | `cargo test --workspace --lib` | no |
| 2 | parser/IR fixture corpora in `tests/parser_corpus.rs`, `tests/parse_directory.rs` | `cargo test --workspace --tests` | no |
| 3 | catalog round-trip goldens in `tests/catalog_round_trip.rs` | `cargo test --workspace --tests` | yes (PG 14/15/16/17) |
| 4 | executor + CLI integration in `crates/pgevolve/tests/{executor_smoke,cli_e2e,chaos_apply}.rs` | `cargo test --workspace --tests` | yes |
| 5 | property tests in `crates/pgevolve-core/tests/property_tests.rs` (pure) and `crates/pgevolve/tests/pg_property_tests.rs` (PG-bound) | `cargo test --workspace --tests` | partial |
| 7 | weekly soak via `.github/workflows/soak.yml` at `PROPTEST_CASES=5000` | manual / cron | yes |

Set `PGEVOLVE_DISABLE_DOCKER_TESTS=1` to skip every Docker-gated test;
the suite skips cleanly when `docker info` fails.

Regenerate tier-3 catalog goldens with `cargo xtask bless` (runs against
ephemeral containers per PG major).

PG-bound property tests default to 3 cases per test for fast feedback;
override with `PGEVOLVE_PROPERTY_CASES=<n>` to stress harder locally. CI
uses 50; the soak workflow uses 5000.

## Dependencies

The workspace deliberately avoids unmaintained / archived crates. Notable
choices:

- **Parsing**: [`pg_query`](https://crates.io/crates/pg_query) (official
  Postgres parser bindings).
- **Hashing**: [`blake3`](https://crates.io/crates/blake3) — plan id,
  target identity, and IR canonical hashing.
- **Encoding**: bincode for the plan-id input hash payload (binary &
  deterministic), serde_json for the embedded catalog snapshot in
  `manifest.toml`. **`serde_yaml` is deliberately not used** (upstream
  archived it in 2024).
- **CLI**: clap v4 derive.
- **Async runtime**: tokio multi-thread; tokio-postgres for the executor.
- **Property tests**: proptest v1, with `EphemeralPostgres` from
  testcontainers for tier-4/5.

## License

Dual-licensed under MIT or Apache-2.0, at your option.
