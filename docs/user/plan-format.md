# Plan format

A plan directory is the unit of code review. This guide explains what
each file means and how the executor consumes it.

```
plans/2026-05-11-abc1234567890123/
├── plan.sql        ← canonical artifact (commit this)
├── intent.toml     ← destructive approvals (commit this; flip approvals when ready)
└── manifest.toml   ← plan id, version metadata, embedded pre-image (commit this)
```

All three files are plain text. **Commit them to the same repo as your
`schema/` tree** — the plan directory is part of the migration history.

## `plan.sql`

The applyable artifact. Reads cleanly with `psql -f plan.sql` for
people who want to bypass the executor — pgevolve only relies on the
structured `-- @pgevolve` directive comments to drive transactions and
audit logging.

### Header directives

```sql
-- @pgevolve plan id=abc1234567890123 version=0.1.0 ruleset=1 created=2026-05-11T18:42:11Z
-- @pgevolve source_rev=git:c0ffeeabc
-- @pgevolve target=tid-xyz
-- @pgevolve intents_required=2
```

| Directive | Meaning |
|---|---|
| `plan id=<16-hex>` | Short plan id. Must match `intent.toml` and `manifest.toml`. |
| `version=<x.y.z>` | The pgevolve version that produced the plan. |
| `ruleset=<n>` | Planner ruleset version. Bumps mean the rewrites changed. |
| `created=<rfc3339>` | UTC timestamp. |
| `source_rev=<rev>` | Optional source-tree revision (`git rev-parse HEAD` if you're in a git repo). |
| `target=<id>` | Stable identifier of the database (hash of host/port/dbname/cluster). |
| `intents_required=<n>` | How many destructive intents this plan declares. |

### Group and step directives

Each transactional group is wrapped in `BEGIN; ... COMMIT;`. Each step
has its own directive line:

```sql
-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.invoices
CREATE TABLE app.invoices ( ... );
-- @pgevolve step=2 kind=add_constraint_not_valid destructive=false targets=app.invoices.invoices_customer_fk
ALTER TABLE app.invoices ADD CONSTRAINT invoices_customer_fk
  FOREIGN KEY (customer_id) REFERENCES app.customers(id) NOT VALID;
COMMIT;

-- @pgevolve group id=2 transactional=false
-- @pgevolve step=3 kind=create_index_concurrent destructive=false targets=app.invoices.invoices_customer_idx
CREATE INDEX CONCURRENTLY invoices_customer_idx ON app.invoices (customer_id);

-- @pgevolve group id=3 transactional=true
BEGIN;
-- @pgevolve step=4 kind=validate_constraint destructive=false targets=app.invoices.invoices_customer_fk
ALTER TABLE app.invoices VALIDATE CONSTRAINT invoices_customer_fk;
-- @pgevolve step=5 kind=drop_column destructive=true intent_id=1 targets=app.users.legacy_email
ALTER TABLE app.users DROP COLUMN legacy_email;
COMMIT;
```

| Directive field | Meaning |
|---|---|
| `group id=<n>` | 1-indexed group number. |
| `transactional=true|false` | Whether the group runs in one `BEGIN/COMMIT`. Non-transactional groups host `CONCURRENTLY` operations. |
| `step=<n>` | 1-indexed step number, contiguous across all groups. |
| `kind=<step_kind>` | Same vocabulary as `plan::raw_step::StepKind`. |
| `destructive=true|false` | Whether the step requires an approved intent. |
| `intent_id=<n>` | Present only on destructive steps; references the same `id` in `intent.toml`. |
| `targets=<qname1>,<qname2>,...` | Comma-separated list of affected qualified names. |

### Step kinds — v0.2 additions (views and materialized views)

The following step kinds were added in v0.2 alongside view and materialized view support:

| Kind | SQL emitted | Transactional | Notes |
|---|---|---|---|
| `create_view` | `CREATE VIEW <qname> AS <body>` | yes | Used for new views and for incompatible body replacements (recreate). |
| `drop_view` | `DROP VIEW <qname>` | yes | Destructive — requires intent approval. |
| `create_materialized_view` | `CREATE MATERIALIZED VIEW <qname> AS <body>` | yes | |
| `drop_materialized_view` | `DROP MATERIALIZED VIEW <qname>` | yes | Destructive — requires intent approval. |
| `refresh_materialized_view` | `REFRESH MATERIALIZED VIEW [CONCURRENTLY] <qname>` | no (CONCURRENTLY); yes (without) | Upgraded to CONCURRENTLY under online strategy when a unique index is present. |
| `alter_view_set_reloption` | `ALTER VIEW <qname> SET (security_barrier = …)` | yes | Also handles `security_invoker`. |
| `comment_on_view` | `COMMENT ON VIEW <qname> IS '…'` | yes | Used for both regular views and materialized views. |

## `intent.toml`

```toml
plan_id = "abc1234567890123"

[[intent]]
id       = 1
step     = 5
kind     = "drop_column"
target   = "app.users.legacy_email"
reason   = "drops column legacy_email"
approved = false
```

Every destructive step gets one `[[intent]]` row. `intent.toml` also supports `[[step_override]]` rows (see below). The executor:

- Reads this file at apply time.
- **Refuses to run** while any row has `approved = false`.
- Records the approval state in the audit log.

### Approving destructive intents

Open `intent.toml`, change `approved = false` to `approved = true` for
each row you want to allow, and commit the change. The plan id field
must stay intact; pgevolve cross-checks it against `plan.sql` and
`manifest.toml` and rejects mismatches.

> **Approval is intentional friction.** A reviewer should be the one
> flipping `approved = true`, not the same person who authored the
> change. Treat the diff in `intent.toml` as the "are you really sure?"
> gate.

## `[[step_override]]` — suppress or skip steps

`intent.toml` also accepts `[[step_override]]` rows. Step overrides let you suppress specific planner steps (e.g., skip a `refresh_materialized_view` during a maintenance window without regenerating the plan):

```toml
[[step_override]]
kind = "refresh_materialized_view"
target = "app.daily_summary"
suppress = true
```

| Field | Required | Notes |
|---|---|---|
| `kind` | yes | Step kind to match (e.g., `refresh_materialized_view`, `create_view`). |
| `target` | yes | Qualified object name the override applies to. |
| `suppress` | no | Default `false`. When `true`, the matching step is silently omitted from execution. The plan is otherwise applied normally. |

> **When to use.** Step overrides are appropriate for one-off operational situations (e.g., deferring an expensive `REFRESH` to off-peak hours). They are **not** a substitute for intent approval — destructive steps still require `approved = true` even when a `[[step_override]]` is present.

## `manifest.toml`

```toml
plan_id                 = "abc1234567890123"
plan_hash               = "abc1234567890123…"  # full 64-char hex
pgevolve_version        = "0.1.0"
planner_ruleset_version = 1
source_rev              = "git:c0ffeeabc"
target_identity         = "tid-xyz"
created_at              = "2026-05-11T18:42:11Z"
target_snapshot_json    = "..."                # embedded pre-image catalog as pretty-printed JSON
```

| Field | Meaning |
|---|---|
| `plan_id` | Short plan id; matches `plan.sql` and `intent.toml`. |
| `plan_hash` | Full 64-char BLAKE3 hex. Recomputable from `(source, target, version, ruleset)`. |
| `pgevolve_version` | The pgevolve build that wrote the plan. |
| `planner_ruleset_version` | Planner ruleset version. |
| `source_rev` | Optional source-tree revision. |
| `target_identity` | Target-DB identity hash. |
| `created_at` | UTC timestamp. |
| `target_snapshot_json` | Embedded pre-image `Catalog` as JSON. Used at apply time for drift detection — pgevolve compares this against the live state to make sure no out-of-band changes happened since planning. |

## Round-trip property

`Plan::write_to_dir(p, dir); Plan::read_from_dir(dir) == p` — modulo
the grafted `destructive_reason` (which lives in `intent.toml`, not in
`plan.sql`). The round-trip is property-tested over random catalogs.

## What pgevolve does and does NOT touch

| File | pgevolve writes | pgevolve reads at apply |
|---|---|---|
| `plan.sql` | yes (planner) | yes (executor) |
| `intent.toml` | yes (planner) with `approved=false` | yes (executor reads the flipped values) |
| `manifest.toml` | yes (planner) | yes (executor cross-checks plan id + reads pre-image for drift) |

pgevolve never modifies a plan directory after it's written. Edits to
`intent.toml`'s approval flags are made by users (typically in a code
review). Anything else is suspicious.
