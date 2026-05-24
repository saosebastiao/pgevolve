# Indexes

Index access methods and per-index options.

See [`../README.md`](./README.md) for the status legend.

## Access methods

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/index.rs::tests`, `parse/builder/index_stmt.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs` (per-method introspection).

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

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/index.rs::tests`, `parse/builder/index_stmt.rs::tests`, `render/index.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs`.

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
| `WITH (storage_parameter = ...)` (index reloptions) | 🟡 Partial | The IR doesn't yet model index storage parameters (`fillfactor`, `gin_pending_list_limit`, etc.). Planned alongside table reloptions in v0.2.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/reloptions.rs::tests`, `diff/reloptions.rs::tests`; tier-C: `objects/reloptions/index-fillfactor`, `index-brin-pages-per-range`, `index-gin-fastupdate` |
| `TABLESPACE <name>` | ✅ Implemented | Stored on the IR; assumes the tablespace exists. change_kinds: [create] |
| Index comment (`COMMENT ON INDEX`) | ✅ Implemented | **Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/comment_stmt.rs::tests` |

## Online-rewrite rules for indexes

| Rule | Status | Notes |
|---|---|---|
| `CREATE INDEX` on an existing table → `CREATE INDEX CONCURRENTLY` (non-unique) | ✅ Implemented | Gated by `[planner.online_rewrites].create_index_concurrent`. Concurrent creates run as their own non-transactional group.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/tests::create_index_on_existing_table_rewrites_to_concurrent`, `unique_create_index_does_not_rewrite_to_concurrent`, `atomic_policy_disables_concurrent_index_rewrite` |
| `DROP INDEX` on an existing index → `DROP INDEX CONCURRENTLY` (non-unique) | ✅ Implemented | Same gating.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/concurrent_index.rs`, `crates/pgevolve-core/src/plan/rewrite/tests` |
| `CREATE UNIQUE INDEX CONCURRENTLY` | ⛔ Not planned (v0.1) | A failed concurrent unique-index build leaves behind an `INVALID` index that must be cleaned up out-of-band. v0.1 plays it safe and uses the locking variant; an opt-in policy may land in v0.2.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/tests::unique_create_index_does_not_rewrite_to_concurrent` |
| INVALID index drift detection and auto-resolution | ✅ Implemented | The catalog reader detects `pg_index.indisvalid = false` (from a failed `CREATE INDEX CONCURRENTLY`) and the differ emits `Change::RecreateIndex`. The planner emits `DROP INDEX + CREATE INDEX`. No user action required. See [`pipeline.md`](./pipeline.md).<br>**Tests:** tier-2: `crates/pgevolve-core/tests/catalog_drift.rs` |
| `REINDEX [CONCURRENTLY]` | 🔮 Future | Useful as an ops command; not yet a planner step kind. |

## Index naming

**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/index_stmt.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs` (constraint-backing index identification).

| Aspect | Status | Notes |
|---|---|---|
| Explicit index name (`CREATE INDEX <name> ON ...`) | ✅ Implemented | The standard case; the IR requires a name. change_kinds: [create] |
| Anonymous index (PG auto-generated name) | ⛔ Not planned | All managed indexes must be named in source. Anonymous indexes from `PRIMARY KEY` / `UNIQUE` constraints are tied to those constraints, not standalone indexes. |
| Constraint-backing indexes (PK / UNIQUE auto-created) | ✅ Implemented | Tracked as part of the constraint, not as a separate `Index`. change_kinds: [create] |
