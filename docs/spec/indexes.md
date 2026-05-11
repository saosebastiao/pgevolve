# Indexes

Index access methods and per-index options.

See [`../README.md`](./README.md) for the status legend.

## Access methods

| Method | Status | Notes |
|---|---|---|
| `btree` | ✅ Implemented | The default; works on every type with a default opclass. |
| `hash` | ✅ Implemented | |
| `gin` | ✅ Implemented | Useful for `jsonb`, full-text search, array containment. |
| `gist` | ✅ Implemented | The R-tree-style method; used by geometric types, range types, exclusion constraints. |
| `brin` | ✅ Implemented | Block-range; large append-only tables. |
| `spgist` | ✅ Implemented | Space-partitioned GiST. |
| Custom access methods (e.g., from extensions) | 🔮 Future | Once `CREATE EXTENSION` lands, the IR can store opaque method names. |

## Per-index options

| Option | Status | Notes |
|---|---|---|
| Indexed column or expression | ✅ Implemented | Both column references and `(expr)` indexes. |
| Column sort order (`ASC` / `DESC`) | ✅ Implemented | |
| `NULLS FIRST` / `NULLS LAST` | ✅ Implemented | |
| Per-column collation | ✅ Implemented | |
| Per-column operator class (`<col> <opclass>`) | ✅ Implemented | |
| `UNIQUE` | ✅ Implemented | |
| `NULLS NOT DISTINCT` (PG 15+) | ✅ Implemented | UNIQUE indexes only. |
| `INCLUDE (col, …)` covering columns | ✅ Implemented | |
| `WHERE <predicate>` (partial index) | ✅ Implemented | Predicate preserved as canonical text. |
| `WITH (storage_parameter = ...)` (index reloptions) | 🟡 Partial | The IR doesn't yet model index storage parameters (`fillfactor`, `gin_pending_list_limit`, etc.). Planned alongside table reloptions in v0.2. |
| `TABLESPACE <name>` | ✅ Implemented | Stored on the IR; assumes the tablespace exists. |
| Index comment (`COMMENT ON INDEX`) | ✅ Implemented | |

## Online-rewrite rules for indexes

| Rule | Status | Notes |
|---|---|---|
| `CREATE INDEX` on an existing table → `CREATE INDEX CONCURRENTLY` (non-unique) | ✅ Implemented | Gated by `[planner.online_rewrites].create_index_concurrent`. Concurrent creates run as their own non-transactional group. |
| `DROP INDEX` on an existing index → `DROP INDEX CONCURRENTLY` (non-unique) | ✅ Implemented | Same gating. |
| `CREATE UNIQUE INDEX CONCURRENTLY` | ⛔ Not planned (v0.1) | A failed concurrent unique-index build leaves behind an `INVALID` index that must be cleaned up out-of-band. v0.1 plays it safe and uses the locking variant; an opt-in policy may land in v0.2. |
| `REINDEX [CONCURRENTLY]` | 🔮 Future | Useful as an ops command; not yet a planner step kind. |

## Index naming

| Aspect | Status | Notes |
|---|---|---|
| Explicit index name (`CREATE INDEX <name> ON ...`) | ✅ Implemented | The standard case; the IR requires a name. |
| Anonymous index (PG auto-generated name) | ⛔ Not planned | All managed indexes must be named in source. Anonymous indexes from `PRIMARY KEY` / `UNIQUE` constraints are tied to those constraints, not standalone indexes. |
| Constraint-backing indexes (PK / UNIQUE auto-created) | ✅ Implemented | Tracked as part of the constraint, not as a separate `Index`. |
