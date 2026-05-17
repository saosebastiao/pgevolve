# Indexes

Index access methods and per-index options.

See [`../README.md`](./README.md) for the status legend.

## Access methods

| Method | Status | Notes |
|---|---|---|
| `btree` | ✅ Implemented | The default; works on every type with a default opclass. change_kinds: [create, drop] |
| `hash` | ✅ Implemented | change_kinds: [create, drop] |
| `gin` | ✅ Implemented | Useful for `jsonb`, full-text search, array containment. change_kinds: [create, drop] |
| `gist` | ✅ Implemented | The R-tree-style method; used by geometric types, range types, exclusion constraints. change_kinds: [create, drop] |
| `brin` | ✅ Implemented | Block-range; large append-only tables. change_kinds: [create, drop] |
| `spgist` | ✅ Implemented | Space-partitioned GiST. change_kinds: [create, drop] |
| Custom access methods (e.g., from extensions) | 🔮 Future | Once `CREATE EXTENSION` lands, the IR can store opaque method names. |

## Per-index options

| Option | Status | Notes |
|---|---|---|
| Indexed column or expression | ✅ Implemented | Both column references and `(expr)` indexes. change_kinds: [create] |
| Column sort order (`ASC` / `DESC`) | ✅ Implemented | change_kinds: [create] |
| `NULLS FIRST` / `NULLS LAST` | ✅ Implemented | change_kinds: [create] |
| Per-column collation | ✅ Implemented | change_kinds: [create] |
| Per-column operator class (`<col> <opclass>`) | ✅ Implemented | change_kinds: [create] |
| `UNIQUE` | ✅ Implemented | change_kinds: [create] |
| `NULLS NOT DISTINCT` (PG 15+) | ✅ Implemented | UNIQUE indexes only. change_kinds: [create] |
| `INCLUDE (col, …)` covering columns | ✅ Implemented | change_kinds: [create] |
| `WHERE <predicate>` (partial index) | ✅ Implemented | Predicate preserved as canonical text. change_kinds: [create] |
| `WITH (storage_parameter = ...)` (index reloptions) | 🟡 Partial | The IR doesn't yet model index storage parameters (`fillfactor`, `gin_pending_list_limit`, etc.). Planned alongside table reloptions in v0.2. change_kinds: [recreate] |
| `TABLESPACE <name>` | ✅ Implemented | Stored on the IR; assumes the tablespace exists. change_kinds: [create] |
| Index comment (`COMMENT ON INDEX`) | ✅ Implemented | change_kinds: [set_comment] |

## Online-rewrite rules for indexes

| Rule | Status | Notes |
|---|---|---|
| `CREATE INDEX` on an existing table → `CREATE INDEX CONCURRENTLY` (non-unique) | ✅ Implemented | Gated by `[planner.online_rewrites].create_index_concurrent`. Concurrent creates run as their own non-transactional group. change_kinds: [create] |
| `DROP INDEX` on an existing index → `DROP INDEX CONCURRENTLY` (non-unique) | ✅ Implemented | Same gating. change_kinds: [drop] |
| `CREATE UNIQUE INDEX CONCURRENTLY` | ⛔ Not planned (v0.1) | A failed concurrent unique-index build leaves behind an `INVALID` index that must be cleaned up out-of-band. v0.1 plays it safe and uses the locking variant; an opt-in policy may land in v0.2. |
| INVALID index drift detection and auto-resolution | ✅ Implemented | The catalog reader detects `pg_index.indisvalid = false` (from a failed `CREATE INDEX CONCURRENTLY`) and the differ emits `Change::RecreateIndex`. The planner emits `DROP INDEX + CREATE INDEX`. No user action required. See [`pipeline.md`](./pipeline.md). |
| `REINDEX [CONCURRENTLY]` | 🔮 Future | Useful as an ops command; not yet a planner step kind. |

## Index naming

| Aspect | Status | Notes |
|---|---|---|
| Explicit index name (`CREATE INDEX <name> ON ...`) | ✅ Implemented | The standard case; the IR requires a name. change_kinds: [create] |
| Anonymous index (PG auto-generated name) | ⛔ Not planned | All managed indexes must be named in source. Anonymous indexes from `PRIMARY KEY` / `UNIQUE` constraints are tied to those constraints, not standalone indexes. |
| Constraint-backing indexes (PK / UNIQUE auto-created) | ✅ Implemented | Tracked as part of the constraint, not as a separate `Index`. change_kinds: [create] |
