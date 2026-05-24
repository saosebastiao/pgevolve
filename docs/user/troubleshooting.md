# Troubleshooting

Common errors and what to do about them. Each entry shows the actual
error string pgevolve emits, the cause, and the fix.

## Pre-flight failures (exit code 2)

### Target identity mismatch

```
target identity mismatch: plan=abc12345abcd1234 live=ff00112233445566
```

**Cause.** The plan was built against a different database than the one
you're applying to. `target_identity` is a hash of
`(current_database, host, port, cluster_name, system_identifier)`;
applying a `dev` plan to `prod` (or vice versa) hits this.

**Fix.**

- If the difference is intentional: re-run `pgevolve apply` with
  `--allow-different-target`.
- Otherwise: re-plan against the correct environment with
  `pgevolve plan --db <env>`.

### Unapproved intent

```
unapproved destructive intents: 1
```

**Cause.** The plan declares one or more destructive steps; the
corresponding `[[intent]]` rows in `intent.toml` still have
`approved = false`.

**Fix.** Open `intent.toml`, review each `[[intent]]` row, and change
`approved = false` to `approved = true` for the ones you authorize.
Commit the change in your code-review tool of choice before applying.

### Drift detected

```
drift detected since planning: 3 change(s)
```

**Cause.** The live database changed between when you ran `pgevolve
plan` and when you ran `pgevolve apply` — typically because someone
(or another tool) ran DDL out of band.

**Fix.**

- Inspect what changed: `pgevolve diff --db <env>`. Compare to
  `manifest.toml`'s `target_snapshot_json`.
- If the drift is intentional: re-plan with `pgevolve plan --db <env>`
  and apply the new plan.
- If the drift is **un**intentional: investigate the source. Don't
  paper over it with `--allow-drift`.

`--allow-drift` exists as a documented escape hatch for "I know the
drift is harmless"; it should be a thinking step, not a reflex.

## Apply failures (exit code 3)

### Advisory lock held

```
pgevolve advisory lock is held by another session
```

**Cause.** Another `pgevolve apply` is running, or one crashed without
releasing the lock cleanly.

**Fix.**

- Wait for the other apply to finish.
- If you're sure no one else is applying:

  ```sql
  SELECT held_by, held_since, pgevolve_version FROM pgevolve.lock;
  ```

  shows who claims to hold the lock. The session-scoped advisory lock
  releases automatically when its session disconnects, so a stale
  `pgevolve.lock` row often clears itself the moment the next acquirer
  takes the lock. Stuck rows from a crash are clearable by:

  ```sql
  SELECT pg_advisory_unlock_all();
  ```

  in the session that holds it, or by terminating that session via
  `pg_terminate_backend`.

### Step failed

```
step 4 (group 2) failed: [42P07] relation "app.users" already exists
```

**Cause.** Postgres rejected the SQL. The bracketed code (`42P07` here)
is the [SQLSTATE](https://www.postgresql.org/docs/current/errcodes-appendix.html);
the rest is the server message.

**Fix.** Inspect the audit log to see the exact step and SQL:

```sh
pgevolve status --db <env>
pgevolve status --db <env> --apply-id <uuid>
```

Or directly:

```sql
SELECT step_no, kind, status, error_message, sql_text
FROM pgevolve.plan_steps
WHERE apply_id = '<uuid>'
ORDER BY step_no;
```

**Common subtypes:**

- `42P07 relation already exists` → you're trying to create something
  that's already there. Usually means the live state drifted; re-plan.
- `23505 duplicate key value violates unique constraint` → an FK
  validation or unique-index build failed because the existing data
  doesn't satisfy the constraint. Fix the data first (out of band),
  then re-plan.
- `42501 permission denied` → the connection's role lacks the
  privilege. Connect as a sufficiently-privileged role (typically
  the schema owner) or grant the missing privileges out of band.
- `25006 cannot run inside a transaction block` (`CREATE INDEX
  CONCURRENTLY`) → almost certainly indicates a plan-format bug; file
  an issue.

## Lint / validation failures (exit code 1)

### Parse error

```
error: parse error: SyntaxError(...): ERROR:  syntax error at or near "..." at /path/to/file.sql:42:1
```

**Cause.** `pg_query` couldn't parse one of your SQL statements. The
file path and line are in the error message.

**Fix.** Run the offending SQL against a real Postgres to see the same
error. Most often it's a typo or an unsupported feature.

### Unsupported object kind

```
error: unsupported object kind: <kind> at /path/to/file.sql:1:1
```

**Cause.** You wrote a statement type that's not in pgevolve's
whitelist for the current release (e.g., a Postgres feature pgevolve
doesn't yet model).

**Fix.** See [`docs/spec/objects.md`](../spec/objects.md) for the
current coverage and roadmap. Views, materialized views, user-defined
types (enum/domain/composite), functions, procedures, triggers, and
extensions are supported as of v0.2.

### Layout-profile violation

```
error: [schema_mirror_path] table should be at `app/tables/users.sql`; found at `schema/oops/users.sql` (schema/oops/users.sql:1:1)
```

**Cause.** Your file is in a path that the configured layout profile
doesn't permit.

**Fix.** Either move the file, or switch `[project].layout_profile`
to one whose rules match your existing layout. `free-form` enforces no
path rules.

### `managed_schemas_match`

```
error: [managed_schemas_match] schema `audit` is declared in source but not listed in `[managed].schemas`
```

**Cause.** Your source declares a schema that's not in your
`[managed].schemas` list — meaning pgevolve would ignore everything in
it.

**Fix.** Add the schema name to `[managed].schemas`, or remove it from
the source.

## Config errors (exit code 4)

### Missing config

```
config error: i/o reading pgevolve.toml: No such file or directory (os error 2)
```

**Cause.** `pgevolve.toml` doesn't exist at the path pgevolve is looking
at (default `./pgevolve.toml`).

**Fix.** Run `pgevolve init` if this is a new project, or pass
`--config <path>` if your config lives elsewhere.

### Unknown environment

```
unknown environment: `prod`
```

**Cause.** `--db prod` referenced an environment that's not in
`pgevolve.toml`.

**Fix.** Add an `[environments.prod]` block, or use the correct env
name.

### Invalid strategy

```
parse error: ... unknown variant `bogus`, expected `atomic` or `online`
```

**Cause.** `[planner].strategy` (or `[environments.<env>].strategy`)
is not `"atomic"` or `"online"`.

**Fix.** Use one of the two valid values.

## Shadow validation

### Docker not available

```
--shadow requires Docker. Install Docker or run without --shadow.
```

**Cause.** `pgevolve validate --shadow` couldn't run `docker info`.

**Fix.** Either install Docker (and ensure your user can run it without
sudo), or drop the `--shadow` flag — non-shadow `validate` doesn't
require Docker.

### Shadow round-trip mismatch

```
pgevolve validate --shadow: 1 mismatch(es):
  - tables.app.users.columns.email.collation: `None` vs `Some(...)`
```

**Cause.** Your source IR doesn't match what pgevolve gets back after
applying it to a fresh Postgres of the configured version. Usually
indicates a Postgres normalization that the IR doesn't account for, or
a bug in pgevolve's introspection.

**Fix.** Report this as an issue with the source file + the error
output. Until it's fixed, you can pin a different `[shadow]
postgres_version` to see if the mismatch is version-specific.

## When all else fails

Open an issue at <https://github.com/saosebastiao/pgevolve/issues> with:

1. The exact command you ran.
2. The full output (stderr + stdout).
3. The relevant slice of `pgevolve.toml`.
4. The Postgres version (`SELECT version();`).
5. Your `pgevolve --version`.

Sensitive output? Redact the DSN — pgevolve never prints passwords, but
your environment variables might.
