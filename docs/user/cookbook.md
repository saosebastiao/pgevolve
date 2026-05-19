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

pgevolve does not detect renames in v0.1 — they diff as drop+add, which
is **destructive** for columns (data loss). If you need to rename:

1. Add the new column / table, and a backfill in a data migration (a
   step pgevolve does not handle).
2. Cut over reads / writes to the new name.
3. Drop the old in a separate, intent-approved plan.

A future version may detect renames via a developer-supplied hint
(e.g., a `-- @pgevolve rename` directive). For v0.1 the safety-first
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
