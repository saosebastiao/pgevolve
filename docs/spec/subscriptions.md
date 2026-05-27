# Subscriptions

pgevolve models `CREATE SUBSCRIPTION` as a first-class declarative IR object.
A subscription is a per-database global namespace object (not schema-qualified)
that controls which publications a database subscribes to via Postgres logical
replication.

## Source surface

Three representative CREATE forms are supported:

```sql
-- Form 1: minimal — subscribe to a single publication
CREATE SUBSCRIPTION sub_main
    CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}'
    PUBLICATION pub_all;

-- Form 2: subscribe to multiple publications with WITH options
CREATE SUBSCRIPTION sub_filtered
    CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}'
    PUBLICATION pub_orders, pub_users
    WITH (binary = true, streaming = on, two_phase = false);

-- Form 3: all currently-supported options
CREATE SUBSCRIPTION sub_full
    CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}'
    PUBLICATION pub_all
    WITH (
        enabled           = true,
        slot_name         = 'myslot',
        binary            = false,
        streaming         = parallel,       -- PG 16+
        two_phase         = false,
        disable_on_error  = true,           -- PG 15+
        origin            = any             -- PG 16+
    );
```

Operational verb forms are **rejected** at parse time (see below).

## `${VAR}` env-var interpolation in CONNECTION strings

The `CONNECTION` string almost always contains credentials that must not be
stored in source control. pgevolve supports `${VAR}` placeholder syntax
anywhere inside the connection string. The literal `${VAR}` tokens are:

- **Stored verbatim** in the source IR and in `plan.sql`.
- **Never logged, persisted, or echoed** during plan generation.
- **Resolved at apply-time preflight**: before pgevolve opens any database
  connection, it scans every step's SQL for `${...}` references, resolves
  each against the process environment (`std::env::var`), and fails with a
  clear error if any reference is unset.

Example workflow:

```sh
# Set the credential in the shell before applying — never commit it
export REPL_PWD="$(vault kv get -field=password secret/repl)"
pgevolve apply plans/2026-05-26-abc1234567890123
```

If `REPL_PWD` is not set, pgevolve refuses to start the apply and prints:

```
error: unresolved env-var reference ${REPL_PWD} in step 1 (create_subscription)
```

**Do not use `$$`-quoting or single-quote escapes** inside the password value —
the substitution is literal string replacement before the SQL is sent to
`tokio-postgres`. The connection string itself is a libpq DSN, not SQL.

## Per-field lenient WITH options

Every field is `Option<T>`. `None` means "unmanaged" — pgevolve neither sets
nor resets the option. `Some(value)` means "managed" — the differ emits an
ALTER to converge the live subscription.

| Option | IR field | Postgres default | PG version | Notes |
|---|---|---|---|---|
| `enabled` | `enabled: Option<bool>` | `true` | 14+ | Whether the subscription is running |
| `slot_name` | `slot_name: Option<Identifier>` | subscription name | 14+ | Publisher-side slot name |
| `binary` | `binary: Option<bool>` | `false` | 14+ | Binary copy / binary replication protocol |
| `streaming` | `streaming: Option<StreamingMode>` | `off` | 14+ | `off` / `on` / `parallel` (parallel is PG 16+) |
| `two_phase` | `two_phase: Option<bool>` | `false` | 14+ | Two-phase commit handling |
| `disable_on_error` | `disable_on_error: Option<bool>` | `false` | **15+** | Disable subscription on apply error |
| `password_required` | `password_required: Option<bool>` | `true` | **16+** | Subscription owner must supply a password |
| `run_as_owner` | `run_as_owner: Option<bool>` | `false` | **16+** | Run apply worker as subscription owner |
| `origin` | `origin: Option<OriginMode>` | `any` | **16+** | `any` / `none` — replicate only non-replicated sources |
| `failover` | `failover: Option<bool>` | `false` | **17+** | Subscription survives failover |

**CREATE-only fields** (`create_slot`, `copy_data`): these are accepted in
source CREATE statements (so users can declare them) but the differ **never**
includes them in `AlterSubscriptionSetOptions` deltas — `pg_subscription`
does not store the CREATE-time decision, so there is nothing to diff against.

## Diff-modulo-password behavior

The `connection` field on a `Subscription` stores the raw connection string
verbatim, including unresolved `${VAR}` placeholders. The differ compares
connection strings as opaque strings: if the source string differs from the
catalog-read string, an `alter_connection` step is emitted.

Because `pg_subscription.subconninfo` stores the live connection string
(with the resolved password at subscription-create time), the catalog reader
replaces any `password=<value>` segment with `password=${__PGEVOLVE_REDACTED}`
before diff comparison. This prevents a spurious `alter_connection` step
every plan cycle due to the round-trip asymmetry between `${VAR}` in source
and a real password in `pg_subscription`.

## Supported lint rules

| Rule | Severity | Condition | Waivable? |
|---|---|---|---|
| `unmanaged-subscription` | Warning | A subscription is in the catalog but not in source | Yes |
| `subscription-references-undeclared-publication` | Warning | A subscription lists a publication not declared in source (may still work, but pgevolve cannot track it) | Yes |
| `subscription-feature-requires-pg-version` | Error | A PG-version-gated option (`disable_on_error` PG15+, `password_required`/`run_as_owner`/`origin` PG16+, `failover` PG17+, `streaming = parallel` PG16+) is used but `[managed].min_pg_version` is below the minimum | No |
| `subscription-password-in-source` | Error | The `CONNECTION` string contains a literal `password=` value (not a `${VAR}` reference) | No |

The last two rules are **not waivable** — they catch security and
compatibility problems that would surface at apply time or in a security audit.

**Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/unmanaged_subscription.rs::tests`,
`subscription_references_undeclared_publication.rs::tests`,
`subscription_feature_requires_pg_version.rs::tests`,
`subscription_password_in_source.rs::tests`; tier-C: `objects/subscriptions/lint-*`.

## Operational verb rejection

The following operational SQL forms are **rejected at parse time** with a
clear error message:

```sql
ALTER SUBSCRIPTION s REFRESH PUBLICATION;         -- rejected
ALTER SUBSCRIPTION s SKIP (lsn = '0/12345678');  -- rejected
ALTER SUBSCRIPTION s ENABLE;                      -- rejected (use WITH (enabled = true))
ALTER SUBSCRIPTION s DISABLE;                     -- rejected (use WITH (enabled = false))
```

These are point-in-time operations, not declarative state. pgevolve only
accepts the source-state-expressing forms that can be diffed and re-applied
safely:

```sql
ALTER SUBSCRIPTION s CONNECTION '...';
ALTER SUBSCRIPTION s ADD PUBLICATION pub_b;
ALTER SUBSCRIPTION s DROP PUBLICATION pub_a;
ALTER SUBSCRIPTION s SET PUBLICATION pub_b;
ALTER SUBSCRIPTION s SET (binary = true);
ALTER SUBSCRIPTION s OWNER TO new_owner;
```

## `pg_subscription` superuser restriction

Postgres restricts `pg_subscription` catalog access to superusers.
pgevolve's catalog reader therefore requires superuser (or `pg_monitor`)
privileges to read existing subscriptions. If the plan user lacks these
privileges, subscriptions are treated as absent from the catalog (the differ
emits a `create_subscription` step). Apply with a superuser role or grant
`pg_monitor` to the plan user.

## Conformance fixtures

12 fixtures under `crates/pgevolve-conformance/tests/cases/objects/subscriptions/`.
All fixtures carry `[fixture] apply = false` because subscriptions require
a publisher cluster at apply time and the conformance harness targets a
single ephemeral Postgres instance. The fixtures validate parse, diff, plan,
and lint without attempting to apply.

**Tests:** tier-C: `objects/subscriptions/create-minimal`,
`create-with-options`, `create-multi-publication`, `drop`, `alter-connection`,
`alter-add-publication`, `alter-drop-publication`, `alter-set-options`,
`alter-enabled-disable`, `comment-on`, `lint-password-in-source`,
`lint-pg-version-gating`.

## 8 StepKind variants

| Step kind | SQL emitted |
|---|---|
| `CreateSubscription` | `CREATE SUBSCRIPTION …` |
| `DropSubscription` | `DROP SUBSCRIPTION name` (destructive; intent required) |
| `AlterSubscriptionConnection` | `ALTER SUBSCRIPTION name CONNECTION '…'` |
| `AlterSubscriptionAddPublication` | `ALTER SUBSCRIPTION name ADD PUBLICATION …` |
| `AlterSubscriptionDropPublication` | `ALTER SUBSCRIPTION name DROP PUBLICATION …` |
| `AlterSubscriptionSetPublication` | `ALTER SUBSCRIPTION name SET PUBLICATION …` (full replacement) |
| `AlterSubscriptionSetOptions` | `ALTER SUBSCRIPTION name SET (key = value, …)` |
| `CommentOnSubscription` | `COMMENT ON SUBSCRIPTION name IS '…'` |

## Out of scope

- **`ALTER SUBSCRIPTION … RENAME TO`** — not supported. Rename is treated
  as Drop + Create (old name disappears, new name appears).
- **`ALTER SUBSCRIPTION … REFRESH PUBLICATION`** — operational verb;
  rejected in source. Run out-of-band when needed.
- **`ALTER SUBSCRIPTION … SKIP (lsn = …)`** — point-in-time skip of a
  replication conflict; not a declarative property. Run out-of-band.
- **Subscription statistics** (`pg_stat_subscription`, worker tables) —
  runtime telemetry, not schema management state.
- **Replication slots** — cluster-level admin objects; see
  `docs/spec/cluster.md` for the cluster surface.
