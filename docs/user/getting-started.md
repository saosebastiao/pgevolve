# Getting started

A walkthrough of pgevolve's full loop on a new project: scaffold →
author SQL → plan → review → apply → status. Expect 5-10 minutes.

## 1. Initialize a project

```sh
mkdir myapp && cd myapp
pgevolve init
```

This creates:

```
myapp/
├── .gitignore
├── pgevolve.toml      ← project configuration
├── plans/             ← future plan directories live here
└── schema/            ← your SQL goes here
```

Open `pgevolve.toml` and edit at least `[environments.dev].url` to a
DSN you can connect to. For the rest of the walkthrough we'll assume:

```toml
[project]
name           = "myapp"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "schema-mirror"

[managed]
schemas        = ["app"]

[planner]
strategy = "online"

[environments.dev]
url = "postgres://postgres@localhost:5432/myapp_dev"
```

> **Heads-up.** `pgevolve apply` writes to the database. Use a throwaway
> database for this walkthrough — or run a local one via Docker:
> `docker run --rm -d -p 5432:5432 -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=myapp_dev postgres:16`.

## 2. Author the first version of the schema

The `schema-mirror` layout profile wants
`schema/<schema>/<kind>/<name>.sql`. For a `users` table in schema `app`:

```sh
mkdir -p schema/app/tables
mkdir -p schema/app/_schema  # the `_schema.sql` lives at schema/app/
```

Create `schema/app/_schema.sql`:

```sql
-- @pgevolve schema=app
CREATE SCHEMA app;
```

Create `schema/app/tables/users.sql`:

```sql
-- @pgevolve schema=app
CREATE TABLE app.users (
    id         bigint      NOT NULL,
    email      text        NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
```

The `-- @pgevolve schema=app` directive lets pgevolve associate any
unqualified objects in the file with the `app` schema (the `CREATE
TABLE` here is already qualified, so the directive is mainly future-
proofing).

## 3. Lint and (optionally) shadow-validate

Quick check that your source parses and obeys the layout profile:

```sh
pgevolve lint
# pgevolve lint: 0 findings
```

If you have Docker available, you can round-trip the IR through an
ephemeral Postgres to catch normalization surprises before they hit
your real database:

```sh
# Add a [shadow] block to pgevolve.toml first:
echo '
[shadow]
backend          = "testcontainers"
postgres_version = "16"' >> pgevolve.toml

pgevolve validate --shadow
# pgevolve validate --shadow: round-trip matched (1 object(s))
```

## 4. Plan the first migration

```sh
pgevolve plan --db dev
# Wrote plan abc1234567890123 to plans/2026-05-11-abc1234567890123 (1 group(s), 3 step(s), 0 intent(s))
```

Inspect what got written:

```sh
ls plans/2026-05-11-abc1234567890123/
# intent.toml  manifest.toml  plan.sql
cat plans/2026-05-11-abc1234567890123/plan.sql
```

You'll see the same DDL you authored, wrapped in `-- @pgevolve` directive
comments that pgevolve's executor reads. For details on the directive
format and the three-file layout, see [plan-format.md](./plan-format.md).

## 5. Apply

```sh
pgevolve apply plans/2026-05-11-abc1234567890123 --db dev
# applied (apply_id=<uuid>)
```

The `app.users` table now exists in `myapp_dev`:

```sh
psql myapp_dev -c '\d app.users'
```

## 6. Make a change

Add a `display_name` column to `schema/app/tables/users.sql`:

```sql
CREATE TABLE app.users (
    id           bigint      NOT NULL,
    email        text        NOT NULL,
    display_name text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
```

Plan and apply the change:

```sh
pgevolve diff --db dev
# 1 change(s):
#   - AlterTable
#       alter table app.users (1 op(s))

pgevolve plan --db dev
# Wrote plan xyz9876543210xyz to plans/2026-05-11-xyz9876543210xyz (1 group(s), 1 step(s), 0 intent(s))

cat plans/2026-05-11-xyz9876543210xyz/plan.sql
# … contains ALTER TABLE app.users ADD COLUMN display_name text;

pgevolve apply plans/2026-05-11-xyz9876543210xyz --db dev
# applied (apply_id=<uuid>)
```

## 7. See history

```sh
pgevolve status --db dev
# 2 recent apply/applies:
#   <uuid>  plan=abc1234567890123  status=succeeded  started=…  finished=…
#   <uuid>  plan=xyz9876543210xyz  status=succeeded  started=…  finished=…

pgevolve status --db dev --apply-id <uuid>
# apply <uuid>  plan=xyz9876543210xyz  status=succeeded
#   started_at=…  finished_at=…
#   pgevolve=0.1.0  source_rev=-  target=<hash>
#   steps (1):
#     [  1] g1 add_column   status=succeeded
```

## What's next

- **A destructive change.** Drop the `display_name` column you just
  added — pgevolve will write an `intent.toml` with `approved = false`
  and refuse to apply until you flip it. See
  [troubleshooting.md](./troubleshooting.md#unapproved-intent).
- **The cookbook** ([cookbook.md](./cookbook.md)) covers patterns for
  adding FKs / setting `NOT NULL` / dropping columns safely, declaring
  GRANTs and row-level-security policies, tuning storage parameters,
  and how the online-rewrite policies change the plan shape.
- **Configuration** ([configuration.md](./configuration.md)) has the
  full `pgevolve.toml` reference, including per-environment strategy
  overrides for production-grade deployments.
- **Cluster surface.** Roles live above the per-database layer. If your
  deployment owns its own Postgres cluster, scaffold a parallel cluster
  project with `pgevolve cluster init` (separate from the per-DB
  project you just created) to manage `CREATE ROLE` declaratively. See
  the [cluster spec](../spec/cluster.md).
