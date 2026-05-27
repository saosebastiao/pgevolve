# SUBSCRIPTION sub-spec — v0.3.5 design

**Status:** approved 2026-05-26. Successor to v0.3.4 PUBLICATION; second sub-spec in the post-v0.3.3 agreed roadmap.

**Goal:** Model Postgres `CREATE SUBSCRIPTION` as a first-class IR object so logical-replication subscriber-side state is declarative under pgevolve, including a credential-handling strategy that keeps source SQL secret-free.

**Non-goals:**
- `REFRESH PUBLICATION`, `SKIP (lsn = …)`, standalone `ENABLE`/`DISABLE` ALTERs — operational verbs, not declarative state; rejected in source, lives in runbook.
- Subscription statistics (`pg_stat_subscription*`) — observability, not state.
- Cross-cluster orchestration: pgevolve manages the subscriber-side metadata only; the publisher cluster's replication slot is owned by the operator.
- Connection-string parsing into structured parts (opaque text chosen for flexibility).
- `RENAME TO`.

## Mental model

A `Subscription` is a subscriber-side declarative model of the connection + publication list + per-subscription options that PG materializes via `CREATE SUBSCRIPTION`. The IR is **secret-free**: the credential portion of the CONNECTION string uses `${VAR}` placeholders that the executor resolves at apply time from the process environment.

Lenient drift at two grains, identical to v0.3.4 PUBLICATION:
- *Whole-subscription*: catalog has a subscription source doesn't declare → `unmanaged-subscription` lint warning, never auto-DROP.
- *Per-field*: every `SubscriptionOptions` field is `Option<T>`. Source `None` = unmanaged → no diff emitted for that field. Lets operators DISABLE for maintenance without pgevolve fighting them.

## IR shape

```rust
pub struct Subscription {
    pub name:         Identifier,                  // global; not schema-qualified
    pub connection:   String,                      // opaque libpq connstr; ${VAR} unresolved
    pub publications: Vec<Identifier>,             // sorted, deduped by canon
    pub options:      SubscriptionOptions,
    pub owner:        Option<Identifier>,          // v0.3.1 lenient
    pub comment:      Option<String>,
}

pub struct SubscriptionOptions {
    pub enabled:            Option<bool>,
    pub slot_name:          Option<Identifier>,    // None = PG uses subscription name
    pub create_slot:        Option<bool>,
    pub copy_data:          Option<bool>,
    pub synchronous_commit: Option<String>,        // GUC string ('on' | 'off' | 'remote_write' | …)
    pub binary:             Option<bool>,
    pub streaming:          Option<StreamingMode>,
    pub two_phase:          Option<bool>,
    pub disable_on_error:   Option<bool>,          // PG 15+
    pub password_required:  Option<bool>,          // PG 16+
    pub run_as_owner:       Option<bool>,          // PG 16+
    pub origin:             Option<OriginMode>,    // PG 16+
    pub failover:           Option<bool>,          // PG 17+
}

pub enum StreamingMode {
    Off,
    On,
    Parallel,                                      // PG 16+
}

pub enum OriginMode {
    Any,
    None,
}
```

`Catalog::subscriptions: Vec<Subscription>` — sorted by `name` in `sort_and_dedupe`. New module: `crates/pgevolve-core/src/ir/subscription.rs`.

**Canon validation (`ir/canon/subscriptions.rs`):**
- `publications` is empty → `IrError::EmptySubscriptionPublications` (PG requires at least one).
- `connection` is empty or whitespace-only → `IrError::EmptyConnection`.
- `publications` deduplicated silently (no error — order may vary from source).
- `publications` sorted (deterministic).

## Source surface

Subscriptions are global. New layout slot: `schema/subscriptions/<name>.sql` for `schema-mirror`.

```sql
-- schema/subscriptions/main_replication.sql

CREATE SUBSCRIPTION main_replication
    CONNECTION 'host=replica.example.com port=5432 dbname=app user=replicator password=${REPL_PASSWORD}'
    PUBLICATION main, audit
    WITH (
        enabled            = true,
        slot_name          = main_slot,
        binary             = true,
        streaming          = parallel,           -- PG 16+
        synchronous_commit = on,
        two_phase          = true
    );

-- @pgevolve owner: replication_admin
COMMENT ON SUBSCRIPTION main_replication IS 'main → app replication stream';
```

```sql
-- schema/subscriptions/readonly_replica.sql

CREATE SUBSCRIPTION readonly_replica
    CONNECTION 'service=primary_repl'         -- libpq service file (pg_service.conf)
    PUBLICATION readonly;
```

`${VAR}` is the only interpolation syntax. Missing env var at apply → preflight error with the variable name; no DB connection attempted. Source IR stores the literal `${REPL_PASSWORD}` text; `git diff` shows the env-var-ref change, never the resolved value.

The parser folds `CREATE SUBSCRIPTION … WITH (…)` and subsequent `ALTER SUBSCRIPTION` operations into one canonical record per name (same mechanism as v0.3.4 PUBLICATION).

**Source-side rejections (parse-time):**
- `ALTER SUBSCRIPTION s REFRESH PUBLICATION` — operational; runbook only.
- `ALTER SUBSCRIPTION s SKIP (lsn = …)` — one-shot recovery; runbook only.
- `ALTER SUBSCRIPTION s ENABLE` / `DISABLE` standalone — set via `WITH (enabled = …)` instead.
- `ALTER SUBSCRIPTION s RENAME TO …` — no renames.
- `CREATE SUBSCRIPTION … CONNECTION 'host=… password=plaintext'` — `subscription-password-in-source` lint (Error). Source must use `${VAR}` for the password.

## `${VAR}` interpolation

| Pipeline stage | Behavior |
|---|---|
| Parse | Source IR stores literal `${REPL_PASSWORD}` text. No resolution. |
| Canon | Detect `password=…` tokens; lint `subscription-password-in-source` if the value isn't `${…}`. |
| Diff | Compare connection strings **modulo password**: a minimal libpq-style tokenizer strips `password=` from both source and catalog before text-compare. (Other keys participate in diff normally.) |
| Plan / `plan.sql` | The on-disk plan contains the **unresolved** `${VAR}` form. Reviewing `plan.sql` in a PR never exposes the secret. |
| Apply preflight | Walk every `Change::CreateSubscription` / `AlterSubscriptionConnection`; resolve every `${VAR}` against process env. Missing env var → `ApplyError::MissingEnvVar(name)`; abort before any DB connection. |
| Apply execution | Resolved CONNECTION string goes into the actual SQL sent to PG. In-memory only; never written to disk. |

The diff-modulo-password tokenizer is a small helper in `diff/subscriptions.rs`; doesn't need a full libpq parser.

## Catalog reader

`pg_subscription` is the source. Per-PG query variants:

| PG | Columns read |
|---|---|
| 14 | `subname, subowner, subenabled, subconninfo, subslotname, subsynccommit, subpublications, subbinary, substream, subtwophasestate` |
| 15+ | + `subdisableonerr` |
| 16+ | + `subpasswordrequired, subrunasowner, suborigin`; `substream` enum extended (`p` = parallel) |
| 17+ | + `subfailover` |

**Security restriction**: `pg_subscription` is superuser-readable only (the subconninfo column would otherwise leak credentials). For non-superuser connections the catalog reader returns an empty `subscriptions: Vec<_>` plus a `DriftReport::UnreadableSubscriptions` warning so the operator knows what's hidden. The `pgevolve` system role (created by `bootstrap`) needs `pg_read_all_data` or equivalent for full visibility; without it, subscription drift is invisible.

Comment via `pg_description` join (`classoid = 'pg_subscription'::regclass`, `objsubid = 0`).

`subenabled` (bool) → `enabled: Option<bool>` directly. `subtwophasestate` is an enum char (`'d'` = disabled, `'p'` = pending, `'e'` = enabled) → `two_phase: Option<bool>`: `'e'` → `Some(true)`, `'d'` → `Some(false)`. `'p'` is a transient setup state; catalog reader emits a `DriftReport::TwoPhasePending(sub_name)` and treats it as `Some(true)` for diff (matches the eventual state).

## Differ

Pair by `name`. Per-subscription cases:

| Source | Target | Emits |
|---|---|---|
| present | absent | `Change::CreateSubscription(Subscription)` (Safe, but expensive — see note) |
| absent | present | no auto-drop; `unmanaged-subscription` warning lint |
| both | both | granular diff (see below) |

**Granular diff when both present:**

- `connection` differs (modulo password) → `Change::AlterSubscriptionConnection { name, new_connection }`.
- Publications added in source → `Change::AlterSubscriptionAddPublication { name, publication }` per add.
- Publications removed in source → `Change::AlterSubscriptionDropPublication { name, publication }` per drop.

Always emit granular ADD/DROP (never `ALTER SUBSCRIPTION s SET PUBLICATION …`). Granular operations are audit-friendly and each step is independently rollback-safe. The wholesale `SET PUBLICATION` variant only saves SQL bytes, never operationally cleaner — drop it from the design. `AlterSubscriptionSetPublication` still appears as a StepKind for parser fold (when source writes an `ALTER … SET PUBLICATION` statement, it's normalized into the IR's `publications` field; the planner re-derives granular ADD/DROP from the diff).
- One or more `options` fields differ → `Change::AlterSubscriptionSetOptions { name, options: SubscriptionOptionsDelta }` — sparse, only changed fields populated.
- `owner` differs (v0.3.1 lenient) → `Change::AlterObjectOwner` with `kind: OwnerObjectKind::Subscription`.
- `comment` differs → `Change::CommentOnSubscription { name, comment }`.

**Note on `CreateSubscription` cost**: it triggers initial table sync (if `copy_data = true`, the default), which can take hours on large tables. Non-destructive in the data-loss sense, but operationally heavy. Operators planning migration cutovers typically set `copy_data = false` and pre-load tables out of band; pgevolve respects whichever the source declares.

## Planner step kinds

```rust
CreateSubscription,                    // destructive=false; expensive (initial sync)
DropSubscription,                      // destructive=true
AlterSubscriptionConnection,
AlterSubscriptionAddPublication,
AlterSubscriptionDropPublication,
AlterSubscriptionSetPublication,       // wholesale list swap
AlterSubscriptionSetOptions,           // sparse-delta WITH options
CommentOnSubscription,
```

8 new StepKinds. All transactional. `Create` is non-destructive in the data-loss sense but documented as expensive in `commands.md`.

## Lint rules

| Rule | Severity | Waivable | Fires on |
|---|---|---|---|
| `unmanaged-subscription` | Warning | yes | Catalog has subscription source doesn't declare. |
| `subscription-references-undeclared-publication` | Warning | yes | Source subscription's `PUBLICATION p, q` lists a name that has no matching `Publication` in source. (Cross-cluster by nature, but the lint helps catch local-source typos.) |
| `subscription-feature-requires-pg-version` | Error | no | Source uses a PG-version-gated option below the project's `[managed].min_pg_version`. Catches `streaming = parallel` on <16, `disable_on_error` on <15, `failover` on <17, etc. |
| `subscription-password-in-source` | Error | no | CONNECTION string contains `password=…` where the value is not `${…}` env-var-ref. Catches accidental commits of plaintext credentials at parse time. |

## Dependency graph

No edges. Subscriptions reference publications by name in a *different* cluster — the local dep graph has no anchor. The planner orders subscriptions via a tier rule: subscriptions create *last* (after every other object), drop *first* (before every other object). This minimizes the window where a referenced object might be missing.

`NodeId::Subscription(Identifier)` joins the enum for plan-step targeting / ordering bookkeeping.

## PG-version gating

Same `[managed].min_pg_version` config introduced in v0.3.4. Per-feature requirements:

- `streaming = parallel` → PG 16+
- `disable_on_error = …` → PG 15+
- `password_required = …` → PG 16+
- `run_as_owner = …` → PG 16+
- `origin = none|any` → PG 16+
- `failover = …` → PG 17+

Source using these on a project declaring older PG fails at lint time via `subscription-feature-requires-pg-version` (Error, not waivable).

## Conformance fixtures

12 fixtures under `crates/pgevolve-conformance/tests/cases/objects/subscriptions/`. Subscriptions cannot be applied end-to-end against the conformance harness (no second ephemeral PG for the publisher), so fixtures use a new `[fixture] apply = false` flag in `fixture.toml` — the harness validates parse + diff + plan.sql + lint but skips the apply step.

| # | Fixture | PG min |
|---|---|---|
| 1 | `for-publication-list/` | 14 |
| 2 | `with-binary-and-streaming/` (Off, On) | 14 |
| 3 | `with-streaming-parallel/` | 16 |
| 4 | `with-two-phase/` | 14 |
| 5 | `with-disable-on-error/` | 15 |
| 6 | `with-password-required-origin-runasowner/` | 16 |
| 7 | `with-failover/` | 17 |
| 8 | `alter-add-publication/` | 14 |
| 9 | `alter-drop-publication/` | 14 |
| 10 | `alter-connection-change/` (env-var name changed) | 14 |
| 11 | `lint/unmanaged-subscription/` | 14 |
| 12 | `lint/password-in-source/` (parse-time Error) | 14 |

(12 fixtures total.)

Tier-3 catalog round-trip *can* exercise the reader using `WITH (enabled = false, create_slot = false, copy_data = false)` — creates the `pg_subscription` row without any network activity. Two tier-3 tests: one PG14 (minimum surface), one PG17 (full surface including failover).

## File / module additions

```
crates/pgevolve-core/src/
├── ir/
│   ├── subscription.rs              NEW — Subscription, SubscriptionOptions, StreamingMode, OriginMode
│   ├── catalog.rs                   MODIFY — subscriptions field
│   ├── mod.rs                       MODIFY — re-export subscription
│   └── canon/
│       ├── mod.rs                   MODIFY — wire subscriptions pass
│       └── subscriptions.rs         NEW — validate non-empty publications, etc.
├── catalog/
│   ├── subscriptions.rs             NEW — decoder
│   ├── queries/
│   │   ├── shared.rs                MODIFY — PG15+/16+/17+ branch
│   │   └── pg14.rs                  MODIFY — PG14 variant
│   ├── assemble/
│   │   └── subscriptions.rs         NEW — assembler
│   └── mod.rs                       MODIFY — wire into read_catalog
├── parse/
│   └── builder/
│       ├── subscription_stmt.rs     NEW — CREATE/ALTER SUBSCRIPTION + fold
│       └── mod.rs                   MODIFY — dispatch
├── diff/
│   ├── subscriptions.rs             NEW — granular diff + connstr-modulo-password
│   ├── change.rs                    MODIFY — 8 new variants
│   ├── mod.rs                       MODIFY — call diff_subscriptions
│   └── owner_op.rs                  MODIFY — OwnerObjectKind::Subscription
├── plan/
│   ├── raw_step.rs                  MODIFY — 8 new StepKind variants
│   ├── plan.rs                      MODIFY — kind_name / parse_kind_name
│   ├── edges.rs                     MODIFY — NodeId::Subscription
│   └── rewrite/
│       ├── subscriptions.rs         NEW — SQL helpers
│       └── mod.rs                   MODIFY — dispatch
└── lint/
    ├── rules/
    │   ├── unmanaged_subscription.rs                              NEW
    │   ├── subscription_references_undeclared_publication.rs      NEW
    │   ├── subscription_feature_requires_pg_version.rs            NEW
    │   ├── subscription_password_in_source.rs                     NEW
    │   └── mod.rs                                                 MODIFY
    └── universal.rs                  MODIFY — wire 4 rules

crates/pgevolve/src/
├── executor/preflight.rs            MODIFY — resolve ${VAR} before any DB connection
└── commands/diff.rs                 MODIFY — print_human + change_kind_name for 8 variants

crates/pgevolve-conformance/
├── src/fixture.rs                   MODIFY — [fixture] apply: bool field
├── tests/run.rs                     MODIFY — honor apply = false
└── tests/cases/objects/subscriptions/  NEW — 12 fixtures

docs/spec/
├── objects.md                       MODIFY — SUBSCRIPTION row ✅ Supported
└── subscriptions.md                 NEW — capability page

CHANGELOG.md                          MODIFY — [0.3.5] section
Cargo.toml                            MODIFY — version 0.3.4 → 0.3.5
```

## Release

v0.3.5. Standard `docs/RELEASING.md` flow. Tag signed.

`unmanaged-subscription` wires into `run_drift_lints` alongside the other unmanaged-* rules. `subscription-password-in-source` and `subscription-feature-requires-pg-version` wire into the source-only lint dispatcher with `min_pg_version` already threaded from v0.3.4.
