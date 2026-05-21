# Object kinds

Every top-level Postgres object kind pgevolve does, will, or won't
manage. See [`../README.md`](./README.md) for the status legend.

## Tables and schemas — core surface

| Object | Status | Notes |
|---|---|---|
| `SCHEMA` | ✅ Implemented | `CREATE / DROP / COMMENT ON`. Schemas are listed in `[managed].schemas`; everything outside the list is ignored by the differ and lint. change_kinds: [create, drop, alter, comment_on] |
| `TABLE` | ✅ Implemented | `CREATE / DROP / ALTER` for every v0.1 column / constraint operation. See [`column-types.md`](./column-types.md) and [`constraints.md`](./constraints.md) for nested capability. Column reorder is detected but not yet applied. change_kinds: [create, drop, alter, comment_on] |
| `INDEX` | ✅ Implemented | Six access methods; partial, expression, INCLUDE, NULLS NOT DISTINCT, opclass, collation, tablespace. See [`indexes.md`](./indexes.md). change_kinds: [create, drop, recreate, set_comment] |
| `SEQUENCE` | ✅ Implemented | `CREATE / DROP / ALTER`. `OWNED BY` modeled. Identity-backing sequences derived from `SERIAL` / `GENERATED AS IDENTITY` columns. change_kinds: [create, drop, alter, comment_on] |
| `COMMENT` | ✅ Implemented | On schemas, tables, columns, indexes, sequences, constraints. change_kinds: [comment_on] |
| Inheritance (`INHERITS`) | ⛔ Not planned | Declarative partitioning supersedes inheritance for v0.1's target use cases. |

## Partitioning

| Feature | Status | Notes |
|---|---|---|
| Declarative partitioned table (`PARTITION BY`) | 📋 Planned, v0.2 | Range, list, hash partition strategies. Each partition is a `Table` with a `partition_of` parent. |
| Partition attach / detach | 📋 Planned, v0.2 | `ATTACH PARTITION` / `DETACH PARTITION CONCURRENTLY` lands with declarative partitioning. |
| Partition pruning at plan time | 🔮 Future | Plan can skip unaffected partitions when a change touches only the parent; v0.2 first ships the basic case. |

## Views

| Object | Status | Notes |
|---|---|---|
| `VIEW` | ✅ Implemented | Stored SQL view. `NormalizedBody::from_sql` canonicalizes the SELECT body on both the source side (T3/T4 parse pass) and the catalog side (T5 catalog reader), so cosmetically-different views diff equal. `security_barrier` and `security_invoker` reloptions are modeled. change_kinds: [create, drop, replace_compatible, replace_incompatible, set_reloption, set_comment] |
| `MATERIALIZED VIEW` | ✅ Implemented | Physically-stored view. `WITH NO DATA` initial state honored. `REFRESH MATERIALIZED VIEW` step kind lands with the planner; upgraded to `REFRESH MATERIALIZED VIEW CONCURRENTLY` under online strategy when the MV has a unique index (`refresh_mv_concurrently = true`). change_kinds: [create, drop, replace_body, refresh, set_comment] |
| `security_barrier` reloption | ✅ Implemented | Modeled as `View::security_barrier: Option<bool>`. Emitted as `ALTER VIEW … SET (security_barrier = …)` via the `alter_view_set_reloption` step kind. |
| `security_invoker` reloption | ✅ Implemented | Modeled as `View::security_invoker: Option<bool>`. Same step kind as `security_barrier`. |
| `CREATE VIEW ... WITH CHECK OPTION` | 🔮 Future | Plumbed alongside views; defaults off. |
| Recursive views (`WITH RECURSIVE`) | 🔮 Future | Requires cycle-aware dep-graph handling. |

## Functions, procedures, triggers

| Object | Status | Notes |
|---|---|---|
| `FUNCTION` (SQL language body) | ✅ Implemented | SQL bodies canonicalized via `NormalizedBody`. `CREATE OR REPLACE FUNCTION` for in-place changes; signature changes are Drop + Create. Full attribute matrix (volatility, strict, security, parallel, leakproof, cost, rows). change_kinds: [create, drop, create_or_replace, replace_with_cascade, comment_on] |
| `FUNCTION` (PL/pgSQL body) | ✅ Implemented | PL/pgSQL bodies parsed via `pg_query::parse_plpgsql`; static SQL deps extracted; dynamic SQL closed by `-- @pgevolve dep:` directives. change_kinds: [create, drop, create_or_replace, replace_with_cascade, comment_on] |
| `FUNCTION` (other PL languages — PL/Python, PL/Perl, etc.) | 🔮 Future | Requires support for `CREATE EXTENSION` for the language first. |
| `PROCEDURE` | ✅ Implemented | Same as functions, qname-only identity. COMMIT/ROLLBACK in body auto-detected; step runs with transactional=OutsideTransaction. change_kinds: [create, drop, create_or_replace, comment_on] |
| `TRIGGER` | ✅ Implemented | BEFORE/AFTER/INSTEAD OF; FOR EACH ROW/STATEMENT; WHEN clause; UPDATE OF columns; REFERENCING transition tables; CONSTRAINT TRIGGER with DEFERRABLE/INITIALLY DEFERRED. Any structural diff → Drop + Create. change_kinds: [create, drop, comment_on] |
| `EVENT TRIGGER` | 🔮 Future | Lower priority; intersects with admin/security tooling. |
| `AGGREGATE` | 🔮 Future | Custom aggregates require user-defined functions; lands with PL languages. |

## Custom types

| Object | Status | Notes |
|---|---|---|
| `ENUM` (`CREATE TYPE ... AS ENUM`) | ✅ Implemented | `ALTER TYPE … ADD VALUE [BEFORE\|AFTER]`, `RENAME VALUE`. Dropping or reordering values triggers `ReplaceWithCascade` (`DROP TYPE CASCADE` + `CREATE TYPE`). change_kinds: [create, drop, alter_type_add_value, alter_type_rename_value, comment_on, replace_with_cascade] |
| `DOMAIN` (`CREATE DOMAIN`) | ✅ Implemented | `NOT NULL`, `CHECK`, default. `ALTER DOMAIN ADD/DROP CONSTRAINT`, `SET/DROP DEFAULT`, `SET/DROP NOT NULL`. Base-type change triggers `ReplaceWithCascade`. change_kinds: [create, drop, alter_domain_add_constraint, alter_domain_drop_constraint, alter_domain_set_default, alter_domain_set_not_null, comment_on, replace_with_cascade] |
| `COMPOSITE TYPE` (`CREATE TYPE ... AS (...)`) | ✅ Implemented | `ADD ATTRIBUTE`, `DROP ATTRIBUTE`, `ALTER ATTRIBUTE TYPE`. Attribute reordering triggers `ReplaceWithCascade`. change_kinds: [create, drop, alter_type_add_attribute, alter_type_drop_attribute, alter_type_alter_attribute_type, comment_on, replace_with_cascade] |
| `RANGE TYPE` (`CREATE TYPE ... AS RANGE`) | 🔮 Future | Lands when range-typed columns become first-class. |
| `BASE TYPE` (`CREATE TYPE ... ( INPUT = ..., OUTPUT = ... )`) | ⛔ Not planned | Requires C-language functions; out of scope. |

## Extensions

| Object | Status | Notes |
|---|---|---|
| `EXTENSION` | ✅ Implemented | Source: `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']` in `.sql` files. Catalog: `pg_extension` joined with `pg_namespace`. Differ: Create, Drop (CASCADE; intent required), AlterUpdate, ReplaceWithCascade for schema changes (intent required), CommentOn. Objects installed by extensions (`pg_depend.deptype='e'`) are excluded from every other catalog query. change_kinds: [create, drop, alter_update, replace_with_cascade, comment_on] |
| Extension version upgrade (`ALTER EXTENSION ... UPDATE`) | ✅ Implemented | Non-destructive. Emits `ALTER EXTENSION foo UPDATE TO 'v';` when source pins a version different from the installed one. |

## Triggers

### IR shape

`Trigger` is a flat struct in `pgevolve-core::ir::trigger`:

| Field | Type | Notes |
|---|---|---|
| `qname` | `QualifiedName` | `schema.trigger_name` — pgevolve uses the schema of the *table*, not a separate trigger namespace |
| `table_name` | `QualifiedName` | Target relation (table, view, or MV) |
| `function_name` | `QualifiedName` | Trigger function (must return `TRIGGER`) |
| `timing` | `TriggerTiming` | `Before` \| `After` \| `InsteadOf` |
| `events` | `Vec<TriggerEvent>` | One or more of `Insert` \| `Update` \| `Delete` \| `Truncate` |
| `for_each` | `ForEach` | `Row` \| `Statement` |
| `when_clause` | `Option<NormalizedExpr>` | WHEN predicate, normalized for canonical comparison |
| `update_columns` | `Vec<Identifier>` | Column list for `UPDATE OF col, …`; empty means all columns |
| `referencing` | `Option<TransitionTables>` | `OLD TABLE AS old_tbl` / `NEW TABLE AS new_tbl` names |
| `constraint` | `bool` | `true` for `CREATE CONSTRAINT TRIGGER` |
| `deferrable` | `bool` | Constraint trigger deferred-ability flag |
| `initially_deferred` | `bool` | `true` for `INITIALLY DEFERRED`; `false` for `INITIALLY IMMEDIATE` |
| `comment` | `Option<String>` | `COMMENT ON TRIGGER` value |

`Catalog::triggers: Vec<Trigger>` — flat collection, sorted by `(table_name, qname)` after `canonicalize()`.

### Parser support

- `CREATE [CONSTRAINT] TRIGGER name timing event [OR event …] ON table [REFERENCING …] [FOR [EACH] {ROW|STATEMENT}] [WHEN (expr)] EXECUTE {FUNCTION|PROCEDURE} fn()` — all documented Postgres syntax variants accepted.
- `COMMENT ON TRIGGER name ON table IS '…'` — accepted alongside `COMMENT ON FUNCTION` and `COMMENT ON EXTENSION`.
- `ALTER TRIGGER` in source files — rejected at statement classification with a structural error. The only `ALTER TRIGGER` Postgres exposes is a rename; pgevolve does not support trigger renames.

### Catalog reader

Queries `pg_trigger` joined with `pg_class` (for the relation name), `pg_namespace`, and `pg_description`. Two filters apply:

- `NOT tgisinternal` — excludes system-generated internal triggers (e.g., deferrable constraint enforcement triggers).
- `NOT EXISTS (SELECT 1 FROM pg_depend WHERE objid = tg.oid AND deptype = 'e')` — excludes triggers installed by extensions; consistent with the `deptype='e'` filter applied to every other catalog query.

### Differ

| Scenario | Change variant |
|---|---|
| Trigger present in source, absent in catalog | `TriggerChange::Create` |
| Trigger absent in source, present in catalog | `TriggerChange::Drop` (destructive — intent required) |
| Comment-only diff | `TriggerChange::CommentOn` |
| Any structural diff (timing, events, for-each, function, WHEN clause, UPDATE OF columns, REFERENCING, constraint/deferrable flags) | `TriggerChange::Drop` + `TriggerChange::Create` |

There is no `ALTER TRIGGER` for body-level changes in Postgres; the only path is drop + recreate. `CommentOn` is always emitted separately when only the comment differs.

### Planner steps

| Step kind | Description |
|---|---|
| `CreateTrigger` | `CREATE [CONSTRAINT] TRIGGER …` |
| `DropTrigger` | `DROP TRIGGER name ON table` — destructive; gated on intent approval |
| `CommentOnTrigger` | `COMMENT ON TRIGGER name ON table IS '…'` |

`DropTrigger` is placed in the same destructive ordering bucket as `DropTable` and `DropFunction`. `CreateTrigger` is placed after the target relation and trigger function are both created/updated.

### Dependency edges

| Edge | Meaning |
|---|---|
| `Trigger → Table / View / MV` | Target relation must exist before the trigger is created |
| `Trigger → Function` | Trigger function must exist (and be up-to-date) before the trigger is created |

Both edges are `DepSource::Structural`. The function edge also ensures that a function change that triggers `ReplaceWithCascade` (drop + recreate the function) will cascade a drop + recreate of any trigger that references it, in the correct order.

### Lint rules

| Rule | Severity | Condition |
|---|---|---|
| `trigger-references-unmanaged-table` | Warning | The trigger's `table_name` schema is not in `[managed].schemas` |
| `trigger-references-unmanaged-function` | Warning | The trigger's `function_name` schema is not in `[managed].schemas` |

### Out of scope / notable gaps

- **`ALTER TRIGGER … RENAME TO`** — not supported. Rename is treated as Drop + Create (old name disappears, new name appears).
- **Event triggers** (`CREATE EVENT TRIGGER`) — a separate object kind; tracked as 🔮 Future in the table above.
- **`WHEN` clause dependency extraction** — the WHEN predicate is stored as a `NormalizedExpr` for canonical diffing but its column references are not added as explicit dep edges. Renames of referenced columns will surface as a structural diff, prompting a Drop + Create.

## Security and roles

| Object | Status | Notes |
|---|---|---|
| `ROLE` (`CREATE ROLE / USER`) | 📋 Planned, v0.3 | Membership and inheritance modeled. `LOGIN` attribute kept. Passwords are *not* stored in source — set out-of-band. |
| `GRANT` / `REVOKE` (object permissions) | 📋 Planned, v0.3 | Per-object grant lists in IR; diff produces minimal GRANT/REVOKE sequences. Default privileges (`ALTER DEFAULT PRIVILEGES`) included. |
| Row-level security policies (`POLICY`) | 📋 Planned, v0.3 | Including `ENABLE ROW LEVEL SECURITY` toggle on tables. |
| Security barriers / leakproof flags | 🔮 Future | Less commonly used; lands alongside fine-grained policy review. |
| `SECURITY LABEL` | ⛔ Not planned | Used primarily by SE-Linux integration; out of scope. |

## Replication and federation

| Object | Status | Notes |
|---|---|---|
| `PUBLICATION` | 🔮 Future | Logical replication source-side metadata. |
| `SUBSCRIPTION` | 🔮 Future | Logical replication consumer; connection strings introduce secrets-management questions. |
| `FOREIGN DATA WRAPPER` (`FDW`) | 🔮 Future | First-class FDW lifecycle (`CREATE SERVER`, `USER MAPPING`, `IMPORT FOREIGN SCHEMA`). |
| `FOREIGN TABLE` | 🔮 Future | Lands with FDWs. |

## Storage and physical layout

| Object | Status | Notes |
|---|---|---|
| `TABLESPACE` | 🔮 Future | The IR carries the `tablespace` attribute on tables and indexes, but pgevolve does not create / drop tablespaces — they're cluster-level admin objects outside the schema-management remit. |
| `TABLE ... USING <access method>` | 🔮 Future | Custom table access methods (zheap, columnar, etc.). |
| `WITH (storage_parameter = ...)` (table reloptions) | 🟡 Partial | The IR doesn't yet model `fillfactor`, autovacuum overrides, etc. Planned for v0.2. change_kinds: [alter] |
| Toast options (`STORAGE EXTERNAL` / `EXTENDED` / `PLAIN` / `MAIN`) | 📋 Planned, v0.2 | Per-column toast strategy lands with extended `[storage]` modeling. |

## Operators, casts, collations, text search

| Object | Status | Notes |
|---|---|---|
| `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` | 🔮 Future | Heavy admin objects; lower priority than user-facing surface. |
| `CAST` | 🔮 Future | Custom casts; lands with custom types. |
| `COLLATION` | 🟡 Partial | Per-column collation **is** modeled in v0.1; `CREATE COLLATION` (defining new collations) is 🔮 Future. change_kinds: [alter] |
| `TEXT SEARCH CONFIGURATION` / `DICTIONARY` / `PARSER` / `TEMPLATE` | 🔮 Future | Lands with full-text-search-aware index methods (`gin` is already supported as a method but text-search dictionaries are not modeled). |

## Statistics, rules, and other helpers

| Object | Status | Notes |
|---|---|---|
| `STATISTICS` (`CREATE STATISTICS`) | 📋 Planned, v0.3 | Multi-column statistics objects (`ndistinct`, `dependencies`, `mcv`). |
| `RULE` | ⛔ Not planned | Largely superseded by triggers; pg_query already discourages new rules. |
| `SERVER` (FDW server) | 🔮 Future | Lands with FDWs. |
| `USER MAPPING` | 🔮 Future | Lands with FDWs. |

## What `pgevolve` deliberately does not manage

| Object | Status | Reason |
|---|---|---|
| `DATABASE` itself | ⛔ Not planned | Database creation is a cluster-admin step; pgevolve assumes the DB exists. |
| `TABLESPACE` directories | ⛔ Not planned | Filesystem-level setup. |
| Cluster-wide settings (`postgresql.conf`) | ⛔ Not planned | Different lifecycle and audit story. |
| Backups, restores, and physical replication | ⛔ Not planned | Outside the schema-management remit. |
| Data itself (row contents) | ⛔ Not planned | pgevolve plans never `INSERT` / `UPDATE` / `DELETE`. Data migrations are users' responsibility. |
