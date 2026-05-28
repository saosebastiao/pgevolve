# Cookbook

Concrete migration patterns, with the plan shape pgevolve produces and
why. Every recipe assumes you've gone through
[Getting started](./getting-started.md) and have a working project.

## Add a nullable column

The simplest case. The planner emits one `ALTER TABLE ADD COLUMN` step
in a single transactional group.

```sql
-- Before
CREATE TABLE app.users (
    id    bigint NOT NULL,
    email text   NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);

-- After (add `display_name`)
CREATE TABLE app.users (
    id           bigint NOT NULL,
    email        text   NOT NULL,
    display_name text,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
```

`pgevolve plan --db dev` →

```sql
-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.users
ALTER TABLE app.users ADD COLUMN display_name text;
COMMIT;
```

No intent required.

## Add a NOT NULL column to a populated table

This is where pgevolve earns its keep. Adding a `NOT NULL` column with
a default to a large table is normally an `ACCESS EXCLUSIVE` scan;
pgevolve does it in two steps:

```sql
-- After
CREATE TABLE app.users (
    id           bigint NOT NULL,
    email        text   NOT NULL,
    display_name text   NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
```

If `display_name` already existed as nullable, the planner emits the
four-step CHECK pattern:

```sql
BEGIN;
-- @pgevolve step=1 kind=add_check_for_not_null
ALTER TABLE app.users
  ADD CONSTRAINT __pgevolve_chk_display_name CHECK (display_name IS NOT NULL) NOT VALID;
COMMIT;

BEGIN;
-- @pgevolve step=2 kind=validate_constraint
ALTER TABLE app.users VALIDATE CONSTRAINT __pgevolve_chk_display_name;
COMMIT;

BEGIN;
-- @pgevolve step=3 kind=set_column_nullable
ALTER TABLE app.users ALTER COLUMN display_name SET NOT NULL;
-- @pgevolve step=4 kind=drop_constraint
ALTER TABLE app.users DROP CONSTRAINT __pgevolve_chk_display_name;
COMMIT;
```

`SET NOT NULL` is cheap once the validated CHECK proves no NULL rows
exist; Postgres skips the table scan.

If you want the old-style single-step `SET NOT NULL` (e.g., on a
guaranteed-empty table), set
`[planner.online_rewrites].not_null_via_check_pattern = false` for that
environment.

## Add a foreign key without locking

Adding an FK to an existing table normally locks during validation.
pgevolve emits the `NOT VALID` + `VALIDATE` pattern across two
transaction groups:

```sql
-- After
CREATE TABLE app.invoices (
    id          bigint NOT NULL,
    customer_id bigint NOT NULL,
    CONSTRAINT invoices_pkey PRIMARY KEY (id),
    CONSTRAINT invoices_customer_fk FOREIGN KEY (customer_id) REFERENCES app.customers (id)
);
```

If `app.invoices` is new in this plan, the FK rides inline with the
`CREATE TABLE`. If `app.invoices` already exists:

```sql
-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_constraint_not_valid destructive=false targets=app.invoices
ALTER TABLE app.invoices ADD CONSTRAINT invoices_customer_fk
  FOREIGN KEY (customer_id) REFERENCES app.customers (id) NOT VALID;
COMMIT;

-- @pgevolve group id=2 transactional=true
BEGIN;
-- @pgevolve step=2 kind=validate_constraint destructive=false targets=app.invoices
ALTER TABLE app.invoices VALIDATE CONSTRAINT invoices_customer_fk;
COMMIT;
```

The two groups are committed independently. If step 2 fails, step 1
stays committed and you can `pgevolve plan` again to retry only the
validation.

## Add a non-unique index concurrently

```sql
CREATE INDEX users_email_idx ON app.users (email);
```

If `app.users` already exists in the live database, the planner
rewrites this to `CREATE INDEX CONCURRENTLY` in its own
non-transactional group:

```sql
-- @pgevolve group id=1 transactional=false
-- @pgevolve step=1 kind=create_index_concurrent destructive=false targets=app.users_email_idx,app.users
CREATE INDEX CONCURRENTLY users_email_idx ON app.users USING btree (email);
```

If the index is `UNIQUE`, pgevolve uses the locking variant
(`CREATE UNIQUE INDEX`) — see the
[`indexes.md` rationale](../spec/indexes.md#online-rewrite-rules-for-indexes).

## Drop a column

```diff
 CREATE TABLE app.users (
     id    bigint NOT NULL,
     email text   NOT NULL,
-    legacy_email text,
     CONSTRAINT users_pkey PRIMARY KEY (id)
 );
```

`pgevolve plan` produces a destructive step and writes an `intent.toml`
with `approved = false`:

```toml
plan_id = "..."

[[intent]]
id       = 1
step     = 1
kind     = "drop_column"
target   = "app.users.legacy_email"
reason   = "drops column legacy_email"
approved = false
```

`pgevolve apply` refuses to run while `approved = false`. Edit the
file, commit the change, and the apply succeeds.

## Drop a table

Same approval flow as drop-column, with extra `data_loss_warning`
flag set in the destructiveness record.

## Forward-reference FK cycle (chicken-and-egg)

Sometimes two tables FK each other:

```sql
CREATE TABLE app.posts (
    id     bigint NOT NULL,
    author bigint NOT NULL,
    CONSTRAINT posts_pkey PRIMARY KEY (id),
    CONSTRAINT posts_author_fk FOREIGN KEY (author) REFERENCES app.users (id)
);

CREATE TABLE app.users (
    id            bigint NOT NULL,
    latest_post   bigint,
    CONSTRAINT users_pkey PRIMARY KEY (id),
    CONSTRAINT users_latest_post_fk FOREIGN KEY (latest_post) REFERENCES app.posts (id)
);
```

The planner detects the cycle and extracts one of the FKs into a
post-pass `ALTER TABLE ADD CONSTRAINT` step. Both tables are created
without that FK first; the FK is added (with `NOT VALID` rewrite if the
target is large) afterward.

## Rename a column or table

pgevolve does not detect renames today — they diff as drop+add, which
is **destructive** for columns (data loss). If you need to rename:

1. Add the new column / table, and a backfill in a data migration (a
   step pgevolve does not handle).
2. Cut over reads / writes to the new name.
3. Drop the old in a separate, intent-approved plan.

A future version may detect renames via a developer-supplied hint
(e.g., a `-- @pgevolve rename` directive). For now the safety-first
posture stands.

## Re-apply after a partial failure

If step 4 of a 5-step plan fails:

- For a **transactional group**: the group rolls back. Earlier steps in
  the group are also rolled back. Step 4's `error_message` is in
  `pgevolve.plan_steps`. Re-plan from the *current* live state and the
  re-apply skips the steps that already committed in earlier groups.
- For an **autocommit group** (e.g., `CONCURRENTLY` step): earlier steps
  stay committed. Re-planning produces a smaller plan that picks up
  from where you stopped.

You don't manually fix anything in the plan directory. You re-run
`pgevolve plan --db <env>` and apply the new plan.

## Managing views

### Create a simple view

```sql
-- schema/app/views/active_users.sql
CREATE VIEW app.active_users AS
  SELECT id, email FROM app.users WHERE deleted_at IS NULL;
```

`pgevolve plan` emits one `create_view` step in a transactional group.

### Add a column to an existing view (compatible change)

If the new column is appended at the end of the SELECT list, the body
change is **compatible**: Postgres can apply it without dropping the view.
pgevolve emits `CREATE OR REPLACE VIEW`:

```sql
-- After: add `created_at`
CREATE VIEW app.active_users AS
  SELECT id, email, created_at FROM app.users WHERE deleted_at IS NULL;
```

```sql
-- @pgevolve step=1 kind=create_view destructive=false targets=app.active_users
CREATE OR REPLACE VIEW app.active_users AS
  SELECT id, email, created_at FROM app.users WHERE deleted_at IS NULL;
```

### Reorder columns (incompatible change → DROP + CREATE)

Reordering columns or changing a column type is **incompatible** with
`CREATE OR REPLACE VIEW`. pgevolve emits an explicit `drop_view` followed
by `create_view`:

```sql
-- After: move `email` before `id`
CREATE VIEW app.active_users AS
  SELECT email, id FROM app.users WHERE deleted_at IS NULL;
```

```sql
-- @pgevolve step=1 kind=drop_view destructive=true intent_id=1 targets=app.active_users
DROP VIEW app.active_users;
-- @pgevolve step=2 kind=create_view destructive=false targets=app.active_users
CREATE VIEW app.active_users AS SELECT email, id FROM app.users WHERE deleted_at IS NULL;
```

The `drop_view` step is destructive — you must flip `approved = true` in
`intent.toml` before applying.

### Dependent-view cascade

If view `B` selects from view `A`, modifying `A`'s body incompatibly
automatically cascades to `B`. pgevolve walks the `body_dependencies`
graph and emits explicit `DROP + CREATE` steps for every affected view.
The plan is fully auditable: no hidden `CASCADE` drops.

To opt out of automatic cascade and instead get an error listing the
affected views, set:

```toml
[planner.online_rewrites]
view_drop_create_dependents = false
```

## Managing user-defined types

### Define an enum

```sql
-- schema/app/types/order_status.sql
CREATE TYPE app.order_status AS ENUM ('pending', 'processing', 'shipped', 'delivered');
```

`pgevolve plan` emits one `create_type` step.

### Add a value to an existing enum

Append a new label at the end (or position it with `BEFORE`/`AFTER` in source):

```sql
-- After: add 'cancelled'
CREATE TYPE app.order_status AS ENUM ('pending', 'processing', 'shipped', 'delivered', 'cancelled');
```

```sql
-- @pgevolve step=1 kind=alter_type_add_value destructive=false targets=app.order_status
ALTER TYPE app.order_status ADD VALUE 'cancelled' AFTER 'delivered';
```

No intent required. The step is transactional (Postgres 12+).

### Rename an enum value

```sql
-- After: rename 'processing' to 'in_progress'
CREATE TYPE app.order_status AS ENUM ('pending', 'in_progress', 'shipped', 'delivered', 'cancelled');
```

```sql
-- @pgevolve step=1 kind=alter_type_rename_value destructive=false targets=app.order_status
ALTER TYPE app.order_status RENAME VALUE 'processing' TO 'in_progress';
```

### Drop an enum value (ReplaceWithCascade)

Postgres does not support `ALTER TYPE … DROP VALUE`. Removing a value
triggers a `ReplaceWithCascade`: `DROP TYPE CASCADE` + `CREATE TYPE`.
All columns and views referencing the type are recreated in the same
transactional group.

```sql
-- @pgevolve step=1 kind=drop_type destructive=true intent_id=1 targets=app.order_status
DROP TYPE app.order_status CASCADE;
-- @pgevolve step=2 kind=create_type destructive=false targets=app.order_status
CREATE TYPE app.order_status AS ENUM ('pending', 'shipped', 'delivered');
```

Flip `approved = true` in `intent.toml` before applying.

### Create a domain with a CHECK constraint

```sql
-- schema/app/types/positive_int.sql
CREATE DOMAIN app.positive_int AS integer
  NOT NULL
  CHECK (VALUE > 0);
```

`pgevolve plan` emits one `create_type` step (domain uses the same step kind as enum/composite).

### Add a CHECK constraint to an existing domain

```sql
-- After: also reject values above one million
CREATE DOMAIN app.positive_int AS integer
  NOT NULL
  CONSTRAINT positive_int_lower CHECK (VALUE > 0)
  CONSTRAINT positive_int_upper CHECK (VALUE <= 1000000);
```

```sql
-- @pgevolve step=1 kind=alter_domain_add_constraint destructive=false targets=app.positive_int
ALTER DOMAIN app.positive_int ADD CONSTRAINT positive_int_upper CHECK (VALUE <= 1000000);
```

### Drop an attribute from a composite type (ReplaceWithCascade)

Postgres supports `ALTER TYPE … DROP ATTRIBUTE` only when no column
or function depends on the composite. pgevolve always uses
`ReplaceWithCascade` for composite attribute drops to handle the
general case safely:

```sql
-- Before
CREATE TYPE app.address AS (street text, city text, zip text);

-- After: drop 'zip'
CREATE TYPE app.address AS (street text, city text);
```

```sql
-- @pgevolve step=1 kind=drop_type destructive=true intent_id=1 targets=app.address
DROP TYPE app.address CASCADE;
-- @pgevolve step=2 kind=create_type destructive=false targets=app.address
CREATE TYPE app.address AS (street text, city text);
```

## Managing functions and procedures

### Define a simple SQL function

```sql
-- schema/app/functions/add_one.sql
CREATE FUNCTION app.add_one(x integer)
  RETURNS integer
  LANGUAGE sql
  IMMUTABLE STRICT
AS $$
  SELECT x + 1
$$;
```

`pgevolve plan` emits one `create_or_replace_function` step.

### Define a PL/pgSQL function with a static body

```sql
-- schema/app/functions/get_active_user.sql
CREATE FUNCTION app.get_active_user(p_id bigint)
  RETURNS app.users
  LANGUAGE plpgsql
  STABLE
AS $$
DECLARE
  r app.users;
BEGIN
  SELECT * INTO r FROM app.users WHERE id = p_id AND deleted_at IS NULL;
  RETURN r;
END
$$;
```

pgevolve extracts the `app.users` dep edge from the static `SELECT` statement at parse time.

### Replace a function body (in-place)

Edit the function's SQL or PL/pgSQL body. If the language and return type are unchanged, `pgevolve plan` emits a single `create_or_replace_function` step — no DROP needed.

```sql
-- After: tighten to active users only
CREATE FUNCTION app.get_active_user(p_id bigint)
  RETURNS app.users
  LANGUAGE plpgsql
  STABLE
AS $$
DECLARE
  r app.users;
BEGIN
  SELECT * INTO r
  FROM app.users
  WHERE id = p_id AND deleted_at IS NULL AND suspended = false;
  RETURN r;
END
$$;
```

```sql
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.get_active_user
CREATE OR REPLACE FUNCTION app.get_active_user(p_id bigint) ...;
```

No intent required. If the return type or language changes, pgevolve falls back to `DROP FUNCTION CASCADE` + `CREATE OR REPLACE FUNCTION` (destructive — requires intent approval).

### Add an overload (same name, different arg types)

PL/pgSQL functions support overloading on arg types. Just add the second definition:

```sql
-- schema/app/functions/format_name.sql  (integer overload)
CREATE FUNCTION app.format_name(user_id integer)
  RETURNS text
  LANGUAGE sql STABLE
AS $$
  SELECT first_name || ' ' || last_name FROM app.users WHERE id = user_id
$$;

-- schema/app/functions/format_name_text.sql  (text overload)
CREATE FUNCTION app.format_name(raw_name text)
  RETURNS text
  LANGUAGE sql IMMUTABLE STRICT
AS $$
  SELECT initcap(raw_name)
$$;
```

pgevolve tracks each overload independently; the identity is `qname + arg_types_normalized`.

### Define a procedure with COMMIT in the body

```sql
-- schema/app/procedures/process_batch.sql
CREATE PROCEDURE app.process_batch(batch_size integer)
  LANGUAGE plpgsql
AS $$
DECLARE
  r record;
BEGIN
  FOR r IN SELECT id FROM app.jobs WHERE status = 'pending' LIMIT batch_size LOOP
    UPDATE app.jobs SET status = 'done' WHERE id = r.id;
    COMMIT;
  END LOOP;
END
$$;
```

pgevolve detects `COMMIT` in the body and emits the step with `transactional=false` (outside a transaction block). The `procedure-contains-commit` lint warning fires as a reminder that the procedure cannot participate in a larger transaction.

### Use `-- @pgevolve dep:` for dynamic SQL

If a function uses `EXECUTE` (dynamic SQL), pgevolve cannot extract deps statically. Declare them with a directive:

```sql
-- schema/app/functions/refresh_summary.sql
CREATE FUNCTION app.refresh_summary()
  RETURNS void
  LANGUAGE plpgsql
AS $$
BEGIN
  -- @pgevolve dep: app.summary
  EXECUTE 'REFRESH MATERIALIZED VIEW app.summary';
END
$$;
```

Without the directive, the `plpgsql-dynamic-sql` lint rule fires as an Error. The directive tells pgevolve that `app.summary` is a dependency, so the planner can order the refresh after any changes to that MV.

## Tune storage parameters (reloptions)

Storage parameters (`fillfactor`, `autovacuum_*`, `parallel_workers`,
GIN `fastupdate`, BRIN `pages_per_range`, …) are declared inline on
the object. Each typed key has a `None` default that means *"unmanaged"*
— pgevolve will neither set nor reset it.

### Declare on a new or existing table

```sql
-- schema/app/tables/orders.sql
CREATE TABLE app.orders (
    id          bigint PRIMARY KEY,
    customer_id bigint NOT NULL,
    placed_at   timestamptz NOT NULL
) WITH (
    fillfactor          = 80,
    autovacuum_enabled  = true,
    autovacuum_vacuum_scale_factor = 0.05,
    parallel_workers    = 4
);
```

If `app.orders` already exists in the catalog without these settings,
`pgevolve plan` emits a single batched `ALTER TABLE`:

```sql
ALTER TABLE app.orders SET (fillfactor = 80, autovacuum_enabled = true,
    autovacuum_vacuum_scale_factor = 0.05, parallel_workers = 4);
```

Both the inline `WITH (…)` form on `CREATE TABLE` and a separate
`ALTER TABLE app.orders SET (…);` are accepted in source.

### Per-AM index reloptions

Indexes accept access-method-specific options, validated at parse time
so PG-invalid combinations fail fast:

```sql
CREATE INDEX orders_customer_id_idx ON app.orders (customer_id)
    WITH (fillfactor = 80);                       -- B-tree: 50..=100

CREATE INDEX orders_tags_idx ON app.orders USING gin (tags)
    WITH (fastupdate = false, gin_pending_list_limit = 4096);

CREATE INDEX orders_placed_at_idx ON app.orders USING brin (placed_at)
    WITH (pages_per_range = 32, autosummarize = true);
```

`fillfactor` ranges differ per AM: B-tree 50..=100, GiST 10..=100,
SP-GiST 90..=100. BRIN and GIN reject `fillfactor` outright.

### Removing a managed reloption

**Removing a value from source does *not* issue a `RESET`.** This is
the same lenient pattern used by `owner`, `grants`, and RLS policies —
pgevolve never destructively undoes state on the catalog side just
because source went quiet.

To clear a reloption you previously managed:

1. Apply `ALTER TABLE app.orders RESET (fillfactor);` out-of-band
   (psql, your DBA tooling, a one-off migration).
2. Remove the `fillfactor = 80` declaration from source.

On the next `pgevolve plan` run, both source and catalog read `None`
and the diff is empty.

### The `unmanaged-reloption` lint

If the catalog has a reloption that source doesn't declare, the
`unmanaged-reloption` lint fires as a warning. This includes both
typed keys (e.g., the DBA set `fillfactor = 70` directly) and
extension keys (e.g., `pg_partman.retention_keep_table = 'true'`).
Waive via `[[lint_waiver]]` in `intent.toml` if the drift is
intentional.

> **First-apply caveat.** `CREATE TABLE … WITH (…)` against a
> brand-new (not-yet-in-catalog) object currently emits the `CREATE`
> step without the inline `WITH (…)` clause. The reloptions land on
> the *second* `plan` + `apply` cycle as an `ALTER … SET`. This is a
> known v0.3.x limitation (see
> [`docs/spec/reloptions.md`](../spec/reloptions.md)); convergent in
> two iterations.

## Grant a role read-only access to a table

`pgevolve` models per-object `owner` and `grants` as of v0.3.1. Both
follow the **lenient drift policy**: declaring `owner = None` (the
default in source) means "unmanaged" — the differ will neither set nor
reset the owner. The same applies to `grants`: declared grants are
added/kept; catalog grants you haven't declared surface as the
`unmanaged-grant` lint warning but are never silently revoked.

```sql
-- schema/app/tables/orders.sql
CREATE TABLE app.orders (
    id          bigint PRIMARY KEY,
    customer_id bigint NOT NULL,
    placed_at   timestamptz NOT NULL
);

-- @pgevolve owner: app_owner
GRANT SELECT ON app.orders TO reporting;
GRANT SELECT (id, placed_at) ON app.orders TO analytics_readonly;
```

`pgevolve plan` emits:

```sql
ALTER TABLE app.orders OWNER TO app_owner;
GRANT SELECT ON TABLE app.orders TO reporting;
GRANT SELECT (id, placed_at) ON TABLE app.orders TO analytics_readonly;
```

To revoke an existing grant, simply remove the `GRANT` line from source.
The differ emits an explicit `REVOKE` because the grant was previously
managed (it appeared in your `Vec<Grant>` and is now gone). This is
distinct from grants the catalog already had but source never claimed:
those are *unmanaged*, never auto-revoked, and surface via the
`unmanaged-grant` lint.

For cross-cutting defaults (e.g., "every new table in `app` is owned by
`app_owner` and grants `SELECT` to `reporting`"), use
`ALTER DEFAULT PRIVILEGES`:

```sql
ALTER DEFAULT PRIVILEGES IN SCHEMA app
    GRANT SELECT ON TABLES TO reporting;
```

`pgevolve` models these as first-class IR; see
[`docs/spec/grants.md`](../spec/grants.md) for the full surface.

## Enable row-level security on a table

`pgevolve` models per-table `rls_enabled`, `rls_forced`, and an embedded
`policies: Vec<Policy>` as of v0.3.2.

```sql
-- schema/app/tables/documents.sql
CREATE TABLE app.documents (
    id      bigint PRIMARY KEY,
    owner   text   NOT NULL,
    body    text   NOT NULL
);

ALTER TABLE app.documents ENABLE ROW LEVEL SECURITY;

CREATE POLICY owner_can_read ON app.documents
    FOR SELECT
    USING (owner = current_user);

CREATE POLICY owner_can_write ON app.documents
    FOR INSERT
    WITH CHECK (owner = current_user);
```

`pgevolve plan` emits one step per change (CREATE/ALTER/DROP POLICY,
ENABLE/DISABLE/FORCE/NOFORCE ROW LEVEL SECURITY). Any change to a
policy's `command` (e.g., `FOR SELECT` → `FOR UPDATE`) goes through
DROP + CREATE because Postgres has no `ALTER POLICY … CHANGE COMMAND`.

Two policy attributes use `NormalizedExpr` for diff (same canon as
CHECK constraints): `USING (…)` and `WITH CHECK (…)`. Whitespace and
keyword-case differences between source and `pg_policies` therefore
don't trigger spurious recreates.

The lenient-drift rule applies: a policy in the catalog that source
doesn't declare surfaces as `unmanaged-policy` (warning) instead of an
auto-DROP. Remove a managed policy from source to drop it explicitly.

See [`docs/spec/policies.md`](../spec/policies.md) for the full
attribute matrix.

## Manage cluster roles

The role surface is *cluster-level*, not per-database. pgevolve manages
it via a separate project type (`pgevolve-cluster.toml` + a `roles/`
tree) and a parallel command family: `pgevolve cluster init / diff /
plan / apply / status`.

```sh
mkdir myapp-cluster && cd myapp-cluster
pgevolve cluster init
```

Author roles as `CREATE ROLE` SQL:

```sql
-- roles/app_owner.sql
CREATE ROLE app_owner WITH NOLOGIN;

-- roles/reporting.sql
CREATE ROLE reporting WITH LOGIN NOINHERIT;
GRANT app_owner TO reporting;
```

The full role-attribute matrix is supported (`LOGIN`/`NOLOGIN`,
`SUPERUSER`/`NOSUPERUSER`, `CREATEDB`, `CREATEROLE`, `REPLICATION`,
`BYPASSRLS`, `CONNECTION LIMIT`, `VALID UNTIL`). Passwords are
**intentionally not modeled** — set them out-of-band so they never
appear in source-controlled SQL.

```sh
pgevolve cluster plan
pgevolve cluster apply plans/2026-05-23-<id>
```

The per-database commands (`pgevolve plan`, `pgevolve apply`, etc.)
treat the role names mentioned in `GRANT` / `OWNER TO` clauses as
*references* — they don't create the roles. Use `pgevolve cluster …`
to manage role lifecycle once at the cluster level, then reference
those role names across all the per-DB projects that share the cluster.

See [`docs/spec/cluster.md`](../spec/cluster.md) for the full project
layout and the role-attribute surface.

## Set up logical replication

Postgres logical replication is declared in two places: a **publication** on
the source database and a **subscription** on the target database. pgevolve
manages both as first-class objects since v0.3.4 (publications) and v0.3.5
(subscriptions).

### Step 1 — Declare the publication (source database project)

```sql
-- schema/publications/pub_orders.sql
CREATE PUBLICATION pub_orders
    FOR TABLE app.orders, app.order_items
    WITH (publish = 'insert, update, delete');
```

`pgevolve plan` against the source DB emits:

```sql
-- @pgevolve step=1 kind=create_publication destructive=false targets=pub_orders
CREATE PUBLICATION pub_orders
    FOR TABLE app.orders, app.order_items
    WITH (publish = 'insert, update, delete');
```

### Step 2 — Declare the subscription (target database project)

Keep credentials out of source SQL by using the `${VAR}` interpolation syntax:

```sql
-- schema/subscriptions/sub_orders.sql
CREATE SUBSCRIPTION sub_orders
    CONNECTION 'host=primary.example.com dbname=app user=repl_user password=${REPL_PWD}'
    PUBLICATION pub_orders
    WITH (binary = true, streaming = on);
```

The literal `${REPL_PWD}` is stored verbatim in plan.sql. It is never
resolved at plan time — only at apply time. This means:

- The plan file is safe to commit and code-review.
- The credential is never written to disk.

`pgevolve plan` against the target DB emits:

```sql
-- @pgevolve step=1 kind=create_subscription destructive=false targets=sub_orders
CREATE SUBSCRIPTION sub_orders
    CONNECTION 'host=primary.example.com dbname=app user=repl_user password=${REPL_PWD}'
    PUBLICATION pub_orders
    WITH (binary = true, streaming = on);
```

### Step 3 — Apply with the credential in the environment

```sh
# In CI or your deploy script — never in source
export REPL_PWD="$(vault kv get -field=password secret/repl_user)"
pgevolve apply plans/2026-05-26-<plan-id>
```

pgevolve scans the plan for `${...}` references before opening any connection.
If `REPL_PWD` is not set, it prints a clear error and exits before touching
the database:

```
error: unresolved env-var reference ${REPL_PWD} in step 1 (create_subscription)
```

### Changing the connection string

Edit the `CONNECTION` value in source and re-plan:

```sql
-- Updated host after a primary failover
CREATE SUBSCRIPTION sub_orders
    CONNECTION 'host=newprimary.example.com dbname=app user=repl_user password=${REPL_PWD}'
    PUBLICATION pub_orders
    WITH (binary = true, streaming = on);
```

`pgevolve plan` detects the connection-string change and emits:

```sql
-- @pgevolve step=1 kind=alter_subscription_connection destructive=false targets=sub_orders
ALTER SUBSCRIPTION sub_orders
    CONNECTION 'host=newprimary.example.com dbname=app user=repl_user password=${REPL_PWD}';
```

### Adding a publication to an existing subscription

```sql
-- Extend sub_orders to also consume pub_users
CREATE SUBSCRIPTION sub_orders
    CONNECTION 'host=primary.example.com dbname=app user=repl_user password=${REPL_PWD}'
    PUBLICATION pub_orders, pub_users
    WITH (binary = true, streaming = on);
```

```sql
-- @pgevolve step=1 kind=alter_subscription_add_publication destructive=false targets=sub_orders
ALTER SUBSCRIPTION sub_orders ADD PUBLICATION pub_users;
```

### Lint: plaintext password caught at plan time

pgevolve's `subscription-password-in-source` lint fires **at parse time** if
the source SQL contains a literal password:

```sql
-- This triggers a hard lint error — never commit plaintext credentials
CREATE SUBSCRIPTION bad_sub
    CONNECTION 'host=primary.example.com dbname=app user=repl password=hunter2'
    PUBLICATION pub_orders;
```

```
error[subscription-password-in-source]: CONNECTION string contains a literal
  password= value. Use ${VAR} env-var interpolation instead.
  --> schema/subscriptions/bad_sub.sql:2
```

The lint is severity Error and not waivable.

See [`docs/spec/subscriptions.md`](../spec/subscriptions.md) for the full
option matrix, lint rules, and operational verb rejection.

## Run the same plan against multiple environments

A plan is bound to a specific `target_identity`. If you generate
against `dev` but want to apply against `staging`:

```sh
pgevolve apply plans/2026-05-11-abc1234567890123 --db staging \
    --allow-different-target
```

This is **intentionally explicit**. The much safer pattern is to plan
twice (once per environment) and review both plans — drift between
environments will show up as a plan difference.

## Multi-column statistics for correlated columns

When two or more columns are strongly correlated (e.g., `status` and `region`
always appear together), Postgres tends to drastically underestimate the
selectivity of combined predicates. `CREATE STATISTICS` teaches the planner
about these correlations.

### Declare the statistic

```sql
-- schema/app/tables.sql
CREATE TABLE app.orders (
    id       bigint NOT NULL,
    status   text   NOT NULL,
    region   text   NOT NULL,
    amount   numeric NOT NULL,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);

-- schema/app/statistics.sql
CREATE STATISTICS app.orders_status_region
    ON (status, region)
    FROM app.orders;
```

`pgevolve plan --db dev` →

```sql
-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_statistic destructive=false targets=app.orders_status_region
CREATE STATISTICS app.orders_status_region ON (status, region) FROM app.orders;
-- @pgevolve step=2 kind=create_statistic destructive=false targets=app.orders_status_region
ANALYZE app.orders;
COMMIT;
```

No intent required.

### Limit to specific kinds

If you only want functional dependency tracking (the cheapest kind):

```sql
CREATE STATISTICS app.orders_dep
    (dependencies)
    ON (status, region)
    FROM app.orders;
```

The kinds clause accepts any non-empty subset of `ndistinct`, `dependencies`,
`mcv`. Omitting the clause enables all three (Postgres default).

### Raise the analyze target for fine-grained estimates

The `statistics_target` controls how many rows the analyzer samples when
building the statistic. The Postgres default is `-1` (inherit the column
setting, usually 100). Raising it to 500 gives much more accurate estimates
for skewed distributions:

```sql
-- Before (no target override)
CREATE STATISTICS app.orders_status_region
    ON (status, region)
    FROM app.orders;

-- After (raise target)
CREATE STATISTICS app.orders_status_region
    ON (status, region)
    FROM app.orders;
ALTER STATISTICS app.orders_status_region SET STATISTICS 500;
```

`pgevolve plan --db dev` →

```sql
-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_statistic_set_target destructive=false targets=app.orders_status_region
ALTER STATISTICS app.orders_status_region SET STATISTICS 500;
COMMIT;
```

This uses the cheap `AlterStatisticSetTarget` path — no DROP + CREATE needed.

### What triggers a destructive ReplaceStatistic

Changing the column list or the kinds requires a `DROP STATISTICS` + `CREATE
STATISTICS` because Postgres has no in-place `ALTER` for those fields:

```sql
-- Before
CREATE STATISTICS app.orders_status_region
    ON (status, region) FROM app.orders;

-- After — add amount column
CREATE STATISTICS app.orders_status_region
    ON (status, region, amount) FROM app.orders;
```

`pgevolve plan --db dev` →

```sql
-- @pgevolve group id=1 transactional=false
-- @pgevolve step=1 kind=replace_statistic destructive=true targets=app.orders_status_region intent=required
DROP STATISTICS app.orders_status_region;
CREATE STATISTICS app.orders_status_region ON (status, region, amount) FROM app.orders;
```

`intent=required` means you must acknowledge the destructive step in
`pgevolve.toml` or pass `--intent` on the CLI before the plan can be applied.

### Lint: unmanaged statistics

If a statistic exists in the live database but is not declared in source,
pgevolve emits an `unmanaged-statistic` warning (severity Warning, waivable):

```
WARN unmanaged-statistic: statistics app.orders_status_region exists in the
     catalog but is not declared in source. Add it to source or waive this
     lint in pgevolve.toml.
```

To waive it:

```toml
# pgevolve.toml
[[lint.waive]]
rule = "unmanaged-statistic"
target = "app.orders_status_region"
reason = "Legacy statistic, managed out-of-band."
```

See [`docs/spec/statistics.md`](../spec/statistics.md) for the full surface,
step-kind matrix, and catalog reader notes.
