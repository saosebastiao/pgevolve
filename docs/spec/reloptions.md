# Storage parameters / reloptions

pgevolve models PG `WITH (storage_parameter = …)` reloptions on tables,
indexes, and materialized views. Each relkind has a typed `*StorageOptions`
struct with named fields for the well-known options plus an `extra:
BTreeMap<String, String>` for extension-registered or otherwise-unknown
keys.

## Semantics — `None` always means "unmanaged"

Every typed field is `Option<T>`. The semantics follow v0.3.1's owner
pattern:

**Tests (whole semantics table):** tier-1: `crates/pgevolve-core/src/diff/reloptions.rs::tests`, `crates/pgevolve-core/src/ir/reloptions.rs::tests`, `crates/pgevolve-core/src/ir/canon/reloptions.rs::tests`.

| source | catalog | differ action |
|---|---|---|
| `None` | `None` | no-op |
| `None` | `Some(x)` | **no-op** — surface as `unmanaged-reloption` lint warning |
| `Some(x)` | `None` | `ALTER … SET (key = x);` |
| `Some(x)` | `Some(x)` | no-op |
| `Some(x)` | `Some(y)` (x ≠ y) | `ALTER … SET (key = x);` |
| `Some(x)` removed from source → `None` | `Some(x)` | **no-op** (lenient) |

**Removing a reloption from source does NOT issue `RESET`.** To clear a
managed reloption:

1. Issue `ALTER TABLE t RESET (fillfactor)` out-of-band.
2. On the next plan run, catalog reads `None`, source is also `None`,
   diff is empty.

This is the same trade-off as v0.3.1's `owner: Option<Identifier>` —
"unmanaged" must be safe to declare without triggering destructive resets.

## Source surface

```sql
-- Inline at creation:
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80, autovacuum_enabled = false);

-- ALTER post-creation:
ALTER TABLE app.t SET (parallel_workers = 4);

-- Indexes:
CREATE INDEX i ON app.t (id) WITH (fillfactor = 70);

-- Materialized views:
CREATE MATERIALIZED VIEW m WITH (fillfactor = 90) AS SELECT * FROM ...;
```

`ALTER ... RESET (...)` and `ALTER ... RESET ()` are **rejected** in source.

**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/reloptions.rs::tests`; tier-2: `crates/pgevolve-core/tests/catalog_reloptions.rs`; tier-C: `objects/reloptions/table-fillfactor`, `table-autovacuum-disabled`, `table-multi-set`, `mv-fillfactor`, `alter-table-set-after-create`, `index-fillfactor`, `index-brin-pages-per-range`, `index-gin-fastupdate`, `partition-inherits-reloptions`.

## Per-relkind validation

**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/reloptions.rs::tests` (per-relkind range checks).

Parser enforces PG's documented ranges at parse time:

- **Tables / MVs `fillfactor`**: 10..=100
- **B-tree index `fillfactor`**: 50..=100
- **GiST / Hash index `fillfactor`**: 10..=100
- **SP-GiST index `fillfactor`**: 90..=100
- **BRIN / GIN index `fillfactor`**: not supported → `ParseError`
- **`parallel_workers`**: 0..=1024
- **`toast_tuple_target`**: 128..=8160
- **`pages_per_range`** (BRIN): 1..=131072
- **Numeric scale factors**: NaN rejected

## Supported keys

### Tables / Materialized Views

`fillfactor`, `parallel_workers`, `toast_tuple_target`, `user_catalog_table`,
`vacuum_truncate`, plus all 16 `autovacuum_*` keys including
`log_autovacuum_min_duration`.

### Indexes

`fillfactor`, `fastupdate` (GIN), `gin_pending_list_limit` (GIN),
`buffering` (GiST/SP-GiST, values: on/off/auto), `deduplicate_items`
(B-tree, PG 13+), `pages_per_range` + `autosummarize` (BRIN).

## Lint

- **`unmanaged-reloption`** (warning, waivable) — catalog has a typed
  reloption or extra-bag key not declared in source. Per the lenient
  drift policy, the differ doesn't RESET; the lint surfaces the drift
  so operators can decide.
  **Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/unmanaged_reloption.rs::tests`; tier-C: `objects/reloptions/lint`.

## Out of scope

- `toast.*` prefixed options (apply to TOAST tables). Rare; deferred.
- Active RESET via source. Operators clear out-of-band.
- Per-partition tablespace overrides. Per-partition reloptions are
  supported (partitions are `Table` in IR).

## Known limitation: new objects with inline reloptions

Currently `CREATE TABLE … WITH (…)` / `CREATE INDEX … WITH (…)` / `CREATE MATERIALIZED VIEW … WITH (…)` source statements that target a brand-new (not-yet-in-catalog) object emit only the `CREATE` step without the `WITH (…)` clause. The reloptions are picked up on the *next* plan run as `unmanaged-reloption` warnings or, if source still declares them, as an `ALTER … SET (…)` step.

Workaround: run `pgevolve` twice (or apply once, plan again). Convergent in 2 iterations.

This is the same general gap that affects owner/grants/policies/RLS on new objects in v0.3.x — the inline new-object rendering doesn't yet include cross-cutting state. Tracked for a future v0.3.x maintenance release that closes the gap uniformly.
