# Configuration reference

Everything in `pgevolve.toml`. The
[capability spec](../spec/cli.md#pgevolvetoml-schema) lists what's
implemented vs. planned; this file walks through *how* to use what's
implemented today.

`pgevolve init` creates a minimal config; this guide explains each
section.

## File location

By default pgevolve loads `./pgevolve.toml`. Override with `--config
<path>` on any command.

## `[project]`

```toml
[project]
name           = "myapp"            # informational
schema_dir     = "schema"           # relative to the config file
plan_dir       = "plans"            # relative to the config file
layout_profile = "schema-mirror"    # one of the built-ins, or a path
```

| Key | Default | Notes |
|---|---|---|
| `name` | — (required) | Shown in `--help` and logs. Purely informational. |
| `schema_dir` | `"schema"` | Where source `*.sql` lives. Resolved relative to the config file. |
| `plan_dir` | `"plans"` | Where `pgevolve plan` writes new plan directories. |
| `layout_profile` | `"schema-mirror"` | One of `schema-mirror`, `kind-grouped`, `feature-grouped`, `free-form`, or a path to a `*.toml` file declaring a custom profile. See the [spec](../spec/lint-and-layout.md). |

## `[managed]`

```toml
[managed]
schemas        = ["app", "billing"]
ignore_objects = ["app.legacy_etl_*", "billing.audit_*"]
```

| Key | Default | Notes |
|---|---|---|
| `schemas` | `[]` | List of schema names pgevolve is responsible for. **Anything outside this list is ignored** by `diff`, `plan`, and `apply`. An empty list means "no schemas are managed" (lint will not enforce schema match). |
| `ignore_objects` | `[]` | Qname or glob patterns to exclude even within managed schemas. Useful for legacy tables that aren't yet pgevolve-controlled. |
| `min_pg_version` | `14` | Minimum PG major version the project targets. Gates PG-version-specific source features (e.g., publication row filters need PG 15+). |

> **The `[managed]` filter is the safety net.** Even if your source tree
> declares only one table, an unfiltered apply would emit drops for
> every catalog object outside your control. The filter prevents that.

## `[planner]`

```toml
[planner]
strategy = "online"   # "atomic" | "online"
```

| Key | Default | Notes |
|---|---|---|
| `strategy` | `"online"` | `"atomic"` puts everything in one transaction and disables every online rewrite; `"online"` enables the configured rewrites (next section). |

## `[planner.online_rewrites]`

```toml
[planner.online_rewrites]
create_index_concurrent       = true
fk_not_valid_then_validate    = true
check_not_valid_then_validate = true
not_null_via_check_pattern    = true
refresh_mv_concurrently       = true
view_drop_create_dependents   = true
```

Each switch defaults to `true`. Setting any to `false` disables that
specific rewrite without dropping to atomic mode. Useful for
environments where you want online behavior in general but need to opt
out of one pattern (e.g., a managed-service Postgres that disallows
`CONCURRENTLY`).

| Switch | Rewrite | Spec |
|---|---|---|
| `create_index_concurrent` | Non-unique `CreateIndex` on an existing table → `CREATE INDEX CONCURRENTLY`. Same for `DropIndex`. | [`indexes.md`](../spec/indexes.md#online-rewrite-rules-for-indexes) |
| `fk_not_valid_then_validate` | `ADD FOREIGN KEY` on an existing table → `ADD ... NOT VALID` + `VALIDATE CONSTRAINT` (two transaction groups). | [`pipeline.md`](../spec/pipeline.md#rewrites) |
| `check_not_valid_then_validate` | Same shape, for CHECK constraints. | [`pipeline.md`](../spec/pipeline.md#rewrites) |
| `not_null_via_check_pattern` | `SET NOT NULL` on a populated column → four-step `ADD CHECK NOT VALID` / `VALIDATE` / `SET NOT NULL` / `DROP CHECK`. | [`pipeline.md`](../spec/pipeline.md#rewrites) |
| `refresh_mv_concurrently` | Upgrade `REFRESH MATERIALIZED VIEW` to `REFRESH … CONCURRENTLY` when the MV has at least one unique index. Has no effect under `strategy = "atomic"`. | [`cli.md`](../spec/cli.md) |
| `view_drop_create_dependents` | Walk the `body_dependencies` graph and emit explicit `DROP + CREATE` steps for every view transitively affected by an upstream change. When `false`, the planner errors instead of cascading dependent-view recreations. | [`cli.md`](../spec/cli.md) |

## `[environments.<name>]`

```toml
[environments.dev]
url      = "postgres://localhost/myapp_dev"

[environments.prod]
url_env  = "DATABASE_URL_PROD"        # read DSN from env var (recommended)
strategy = "online"                   # overrides [planner].strategy for --db=prod

[environments.test]
url      = "postgres://localhost/myapp_test"
strategy = "atomic"                   # opt-out for fast / hermetic test DB
```

| Key | Notes |
|---|---|
| `url` | Explicit DSN. Mutually exclusive with `url_env`. |
| `url_env` | Name of an environment variable holding the DSN. Read at command time. Mutually exclusive with `url`. |
| `strategy` | Optional per-environment override of `[planner].strategy`. |

Omit both `url` and `url_env` to fall through to `PGEVOLVE_DATABASE_URL`
and libpq env vars (`PGHOST`, `PGUSER`, etc.).

### Connection precedence

For `pgevolve <cmd> --db <env>`:

1. `--url <dsn>` (CLI argument)
2. `[environments.<env>].url`
3. `[environments.<env>].url_env`
4. `PGEVOLVE_DATABASE_URL`
5. libpq env vars
6. `~/.pgpass`

## `[shadow]`

```toml
[shadow]
backend          = "auto"         # auto | testcontainers | dsn
url              = "postgres://localhost/myapp_shadow"   # for backend = "dsn"
url_env          = "PGEVOLVE_SHADOW_URL"                 # alternative to url
reset            = "drop_schema_cascade"                 # drop_schema_cascade | none
extensions       = ["pgcrypto", "uuid-ossp"]
postgres_version = "17"           # major version; used to select container or validate DSN
```

| Key | Default | Notes |
|---|---|---|
| `backend` | `"auto"` | How to obtain a shadow Postgres. See below. |
| `url` | — | DSN for an existing Postgres to use as shadow. Requires `backend = "dsn"` or `backend = "auto"` with a URL set. Mutually exclusive with `url_env`. |
| `url_env` | — | Name of an environment variable holding the shadow DSN. Alternative to `url`. |
| `reset` | `"drop_schema_cascade"` | How to clean the shadow DB between runs. `"drop_schema_cascade"` drops all schemas under `[managed].schemas`; `"none"` leaves the DB as-is (useful for DSN backends where you manage teardown yourself). |
| `extensions` | `[]` | Extensions to install in the shadow DB before any apply. Names must match `[a-zA-Z_][a-zA-Z0-9_-]*`. |
| `postgres_version` | `"16"` | Major version: `"14"`, `"15"`, `"16"`, or `"17"`. Pick the version that matches production. Used to select the container image or to validate a provided DSN. |

### `backend` values

- **`"auto"`** (default): uses `url` / `url_env` if set; otherwise tries
  testcontainers if Docker is available; otherwise errors with a helpful
  message.
- **`"testcontainers"`**: always uses Docker. Hermetic; requires Docker
  to be running. Pulls `postgres:<major>-alpine`.
- **`"dsn"`**: connects to a user-supplied Postgres. No Docker required.
  Useful for developers without Docker or for projects with
  pre-installed extensions (TimescaleDB, PostGIS, etc.).

`pgevolve validate --shadow` and the `--shadow-validate` flag on
`plan` / `diff` / `validate` all read this block. Without it those
flags error out with a helpful message.

## `[[lint_waiver]]`

```toml
[[lint_waiver]]
rule   = "column-position-drift"
target = "app.users"
reason = "applied via separate rewrite-table operation; see PR #234"
```

`[[lint_waiver]]` rows acknowledge `LintAtPlan`-severity findings so
that `pgevolve plan` doesn't refuse with exit `2`. Live in the plan's
`intent.toml`, not in `pgevolve.toml`.

| Key | Notes |
|---|---|
| `rule` | Exact rule name of the finding to waive (e.g., `"column-position-drift"`). Must be non-empty. |
| `target` | Substring matched against the finding's message (typically the qualified object name). Must be non-empty. |
| `reason` | Free-form justification. Shown in `--format human` output. |

Match semantics: a waiver applies when `rule` equals the finding's rule
name **and** `target` is a substring of the finding's message. A waiver
that matches zero findings is reported as a warning ("unused waiver").

Preflight at apply time validates structural well-formedness: both
`rule` and `target` must be non-empty strings. A malformed waiver row
causes preflight to exit `2`.

Multiple `[[lint_waiver]]` rows are supported — use one per finding.

## `[cluster]`

```toml
[cluster]
project = "../my-cluster"
```

Optional. Links this per-DB project to a sibling cluster project (managed
via `pgevolve cluster …` against a `pgevolve-cluster.toml`). When set,
cluster-aware lints (e.g. `grant-references-unknown-role`) cross-check
grantee role names against the linked cluster project's declared roles.

| Key | Notes |
|---|---|
| `project` | Path to the cluster project directory (containing `pgevolve-cluster.toml`). Relative paths resolve against `pgevolve.toml`'s directory. |

See [`docs/spec/cluster.md`](../spec/cluster.md) for the cluster surface
and [`docs/spec/grants.md`](../spec/grants.md) for the cross-check rules.

## Worked example: production-grade config

```toml
[project]
name           = "ledger"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "schema-mirror"

[managed]
schemas        = ["app", "billing", "audit"]
ignore_objects = ["audit.legacy_*"]

[planner]
strategy = "online"

[planner.online_rewrites]
# Production DB doesn't allow CONCURRENTLY (RDS-style).
create_index_concurrent       = false
fk_not_valid_then_validate    = true
check_not_valid_then_validate = true
not_null_via_check_pattern    = true

[environments.dev]
url      = "postgres://localhost/ledger_dev"
strategy = "atomic"               # fast local iteration

[environments.staging]
url_env  = "DATABASE_URL_STAGING"

[environments.prod]
url_env  = "DATABASE_URL_PROD"

[shadow]
backend          = "testcontainers"
postgres_version = "16"
```

CI runs `pgevolve validate --shadow` on every PR. The dev developer
runs `pgevolve plan --db dev`, then `pgevolve plan --db staging` once
the diff stabilizes; the same plan directory is applied to staging and
production after intent-file approval.
