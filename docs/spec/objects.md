# Object kinds

Every top-level Postgres object kind pgevolve does, will, or won't
manage. See [`../README.md`](./README.md) for the status legend.

## Tables and schemas — core surface

| Object | Status | Notes |
|---|---|---|
| `SCHEMA` | ✅ Implemented | `CREATE / DROP / COMMENT ON`. Schemas are listed in `[managed].schemas`; everything outside the list is ignored by the differ and lint.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/schema.rs::tests`, `parse/builder/create_schema_stmt.rs::tests`, `diff/schemas.rs::tests`; tier-C: `failure/parse/duplicate-schema` |
| `TABLE` | ✅ Implemented | `CREATE / DROP / ALTER` for every v0.1 column / constraint operation. See [`column-types.md`](./column-types.md) and [`constraints.md`](./constraints.md) for nested capability. Column reorder is detected but not yet applied.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/table.rs::tests`, `parse/builder/create_stmt.rs::tests`, `diff/tables.rs::tests`; tier-C: `objects/tables/create-simple`, `drop-simple`, `add-column-nullable`, `comment-on-table` |
| `INDEX` | ✅ Implemented | Six access methods; partial, expression, INCLUDE, NULLS NOT DISTINCT, opclass, collation, tablespace. See [`indexes.md`](./indexes.md).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/index.rs::tests`, `parse/builder/index_stmt.rs::tests`, `diff/indexes.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs` |
| `SEQUENCE` | ✅ Implemented | `CREATE / DROP / ALTER`. `OWNED BY` modeled. Identity-backing sequences derived from `SERIAL` / `GENERATED AS IDENTITY` columns.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/sequence.rs::tests`, `parse/builder/create_seq_stmt.rs::tests`, `diff/sequences.rs::tests`, `diff/sequence_op.rs::tests`; tier-2: `parser/equivalent_pairs/0002-serial-desugar` |
| `COMMENT` | ✅ Implemented | On schemas, tables, columns, indexes, sequences, constraints.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/comment_stmt.rs::tests`; tier-C: `objects/tables/comment-on-table`, `comment-on-column` |
| Inheritance (`INHERITS`) | ⛔ Not planned | Declarative partitioning supersedes inheritance for v0.1's target use cases. |

## Partitioning

| Feature | Status | Notes |
|---|---|---|
| Declarative partitioned table (`PARTITION BY`) | ✅ Implemented | Range, list, hash partition strategies. `partition_by: Option<PartitionBy>` on `Table`. Source Forms 1, 2, and 3 all unified into the same IR.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/partition.rs::tests`; tier-C: `objects/partitions/create-range-parent-and-two-partitions`, `create-list-parent`, `create-hash-parent-and-partitions`, `create-default-partition` |
| Partition attach / detach (`ATTACH PARTITION` / `DETACH PARTITION`) | ✅ Implemented | `TableChange::AttachPartition` / `DetachPartition`. Bounds rebound = detach + reattach. `DetachPartition` is destructive; intent required.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/alter_table_attach_partition.rs::tests`, `plan/rewrite/partitions.rs::tests`; tier-C: `objects/partitions/attach-existing-standalone`, `attach-form-vs-declarative-form-equivalent`, `detach-to-standalone`, `replace-bounds`, `add-partition`, `drop-partition` |
| Sub-partitioning | ✅ Implemented | A table may have both `partition_by` (is a partitioned parent) and `partition_of` (is a partition child).<br>**Tests:** tier-C: `objects/partitions/subpartitioned` |
| `DETACH PARTITION CONCURRENTLY` | ⛔ Not planned | The non-concurrent form is used for now; concurrent detach adds apply-time complexity for minimal benefit. |
| Partition pruning at plan time | 🔮 Future | Plan can skip unaffected partitions when a change touches only the parent. |

## Views

| Object | Status | Notes |
|---|---|---|
| `VIEW` | ✅ Implemented | Stored SQL view. `NormalizedBody::from_sql` canonicalizes the SELECT body on both the source side (T3/T4 parse pass) and the catalog side (T5 catalog reader), so cosmetically-different views diff equal. `security_barrier` and `security_invoker` reloptions are modeled.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/view.rs::tests`, `parse/builder/create_view_stmt.rs::tests`, `parse/normalize_body.rs::tests`, `diff/views.rs::tests`; tier-C: `objects/views/create-simple`, `create-with-aliases`, `drop`, `replace-body-compatible`, `replace-body-incompatible`, `comment-on-view` |
| `MATERIALIZED VIEW` | ✅ Implemented | Physically-stored view. `WITH NO DATA` initial state honored. `REFRESH MATERIALIZED VIEW` step kind lands with the planner; upgraded to `REFRESH MATERIALIZED VIEW CONCURRENTLY` under online strategy when the MV has a unique index (`refresh_mv_concurrently = true`).<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/create_materialized_view_stmt.rs::tests`, `plan/rewrite/refresh_mv_concurrently.rs::tests`; tier-C: `objects/materialized_views/create-simple`, `index-on-mv`, `refresh-concurrently`, `replace-body`, `with-no-data-override` |
| `security_barrier` reloption | ✅ Implemented | Modeled as `View::security_barrier: Option<bool>`. Emitted as `ALTER VIEW … SET (security_barrier = …)` via the `alter_view_set_reloption` step kind.<br>**Tests:** tier-C: `objects/views/security-barrier-toggle` |
| `security_invoker` reloption | ✅ Implemented | Modeled as `View::security_invoker: Option<bool>`. Same step kind as `security_barrier`.<br>**Tests:** tier-C: `objects/views/security-invoker-toggle` |
| `CREATE VIEW ... WITH CHECK OPTION` | ✅ Supported | Per-view `check_option: Option<CheckOption>` (`Local` / `Cascaded`). Both source forms parsed (SQL clause + WITH-options). Diff emits `CREATE OR REPLACE VIEW`. change_kinds: [alter_view_set_check_option] |
| Recursive views (`WITH RECURSIVE`) | 📋 Planned, v0.5.3 | Requires cycle-aware dep-graph handling. See [`roadmap.md`](./roadmap.md). |

## Functions, procedures, triggers

| Object | Status | Notes |
|---|---|---|
| `FUNCTION` (SQL language body) | ✅ Implemented | SQL bodies canonicalized via `NormalizedBody`. `CREATE OR REPLACE FUNCTION` for in-place changes; signature changes are Drop + Create. Full attribute matrix (volatility, strict, security, parallel, leakproof, cost, rows).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/function.rs::tests`, `parse/builder/create_function_stmt.rs::tests`, `diff/routines.rs::tests`; tier-2: `crates/pgevolve-core/tests/functions_round_trip.rs`; tier-C: `objects/functions/create-sql-simple`, `replace-body`, `replace-volatility`, `replace-return-type-cascade`, `create-with-overload-pair`, `create-with-table-return`, `comment-on-function` |
| `FUNCTION` (PL/pgSQL body) | ✅ Implemented | PL/pgSQL bodies parsed via `pg_query::parse_plpgsql`; static SQL deps extracted; dynamic SQL closed by `-- @pgevolve dep:` directives.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/plpgsql.rs::tests`; tier-C: `objects/functions/create-plpgsql-simple`, `function-with-dynamic-sql-directive`, `create-trigger-function` |
| `FUNCTION` (other PL languages — PL/Python, PL/Perl, etc.) | 📋 Planned, v0.4.2 | Requires support for `CREATE EXTENSION` for the language first. See [`roadmap.md`](./roadmap.md). |
| `PROCEDURE` | ✅ Implemented | Same as functions, qname-only identity. COMMIT/ROLLBACK in body auto-detected; step runs with transactional=OutsideTransaction.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/procedure.rs::tests`; tier-C: `objects/procedures/create-simple`, `create-with-commit`, `replace-body`, `drop-procedure`, `comment-on-procedure` |
| `TRIGGER` | ✅ Implemented | BEFORE/AFTER/INSTEAD OF; FOR EACH ROW/STATEMENT; WHEN clause; UPDATE OF columns; REFERENCING transition tables; CONSTRAINT TRIGGER with DEFERRABLE/INITIALLY DEFERRED. Any structural diff → Drop + Create.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/trigger.rs::tests`, `parse/builder/create_trigger_stmt.rs::tests`, `diff/triggers.rs::tests`, `plan/rewrite/triggers.rs::tests`, `plan/rewrite/emit/trigger.rs::tests`; tier-C: `objects/triggers/create-row-trigger-simple`, `create-statement-trigger`, `create-instead-of-on-view`, `create-with-transition-tables`, `create-constraint-trigger`, `replace-event-list`, `replace-function`, `replace-when-clause`, `drop-simple`, `comment-on` |
| <a id="objects.event_trigger"></a>`EVENT TRIGGER` | ✅ Supported | Database-global object (bare name, no schema). `CREATE EVENT TRIGGER … ON <event>` with optional `WHEN TAG IN (…)` command-tag filter; `ALTER … ENABLE/DISABLE/ENABLE REPLICA/ENABLE ALWAYS`, `ALTER … OWNER TO`, `DROP`, and `COMMENT ON`. Lenient owner + lenient drop, mirroring publications/subscriptions: unmanaged event triggers surface via the `unmanaged-event-trigger` lint and are never auto-dropped. Extension-owned event triggers are excluded from introspection. change_kinds: [create_event_trigger, drop_event_trigger, alter_event_trigger_enable, alter_event_trigger_owner, comment_on_event_trigger] |
| <a id="objects.aggregate"></a>`AGGREGATE` | ✅ Supported | User-defined aggregates: `CREATE AGGREGATE` (ordinary form — SFUNC + STYPE + optional FINALFUNC/INITCOND), `ALTER … OWNER TO`, `DROP`, and `COMMENT ON`. State and final functions must be managed SQL/plpgsql functions; source rejects references to unmanaged or built-in functions via the `aggregate-references-unmanaged-function` lint. The reader skips ordered-set aggregates, moving aggregates, and aggregates whose state function is in an unreadable language. Rename is drop + create; identity is `(schema, name, arg_types)`. change_kinds: [create_aggregate, drop_aggregate, alter_aggregate_owner, comment_on_aggregate] |

## Custom types

| Object | Status | Notes |
|---|---|---|
| `ENUM` (`CREATE TYPE ... AS ENUM`) | ✅ Implemented | `ALTER TYPE … ADD VALUE [BEFORE\|AFTER]`, `RENAME VALUE`. Dropping or reordering values triggers `ReplaceWithCascade` (`DROP TYPE CASCADE` + `CREATE TYPE`).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/user_type.rs::tests`, `parse/builder/create_enum_stmt.rs::tests`, `diff/types.rs::tests`, `ir/canon/renumber_enum_sort_orders.rs::tests`; tier-2: `crates/pgevolve-core/tests/types_round_trip.rs`; tier-C: `objects/enums/create-simple`, `add-value-at-end`, `add-value-before-existing`, `rename-value`, `drop-value-cascade-recreate`, `comment-on-enum` |
| `DOMAIN` (`CREATE DOMAIN`) | ✅ Implemented | `NOT NULL`, `CHECK`, default. `ALTER DOMAIN ADD/DROP CONSTRAINT`, `SET/DROP DEFAULT`, `SET/DROP NOT NULL`. Base-type change triggers `ReplaceWithCascade`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/create_domain_stmt.rs::tests`; tier-C: `objects/domains/create-simple`, `create-with-check-and-default`, `add-check-constraint`, `set-default`, `toggle-not-null`, `comment-on-domain` |
| `COMPOSITE TYPE` (`CREATE TYPE ... AS (...)`) | ✅ Implemented | `ADD ATTRIBUTE`, `DROP ATTRIBUTE`, `ALTER ATTRIBUTE TYPE`. Attribute reordering triggers `ReplaceWithCascade`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/create_composite_type_stmt.rs::tests`; tier-C: `objects/composites/create-simple`, `add-attribute`, `alter-attribute-type`, `comment-on-composite` |
| `RANGE TYPE` (`CREATE TYPE ... AS RANGE`) | ✅ Implemented (v0.3.8) | `UserTypeKind::Range` variant: `subtype`, `subtype_opclass`, `collation`, `canonical`, `subtype_diff`, `multirange_type_name`. Structural changes go through `ReplaceWithCascade` (PG has no in-place ALTER for these fields). Auto-generated multirange types filtered from `pg_type` via `typtype != 'm'`. Dep edges: `Range → subtype Type`, `canonical Function`, `subtype_diff Function`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/user_type.rs::tests`, `parse/builder/create_stmt.rs::tests`, `diff/types.rs::tests`, `plan/rewrite/emit/user_type.rs::tests`, `plan/edges.rs::tests`; tier-C: `objects/ranges/create-simple-int4range`, `create-with-opclass`, `create-with-subtype-diff-fn`, `column-with-range-type`, `drop` |
| `BASE TYPE` (`CREATE TYPE ... ( INPUT = ..., OUTPUT = ... )`) | ⛔ Not planned | Requires C-language functions; out of scope. |

## Extensions

| Object | Status | Notes |
|---|---|---|
| `EXTENSION` | ✅ Implemented | Source: `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']` in `.sql` files. Catalog: `pg_extension` joined with `pg_namespace`. Differ: Create, Drop (CASCADE; intent required), AlterUpdate, ReplaceWithCascade for schema changes (intent required), CommentOn. Objects installed by extensions (`pg_depend.deptype='e'`) are excluded from every other catalog query.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/extension.rs::tests`, `parse/builder/create_extension_stmt.rs::tests`, `diff/extensions.rs::tests`, `plan/rewrite/extensions.rs::tests`; tier-C: `objects/extensions/create-simple`, `create-with-schema`, `create-with-version`, `drop-simple`, `replace-schema`, `comment-on`, `scenarios/extension-owned-objects-ignored` |
| Extension version upgrade (`ALTER EXTENSION ... UPDATE`) | ✅ Implemented | Non-destructive. Emits `ALTER EXTENSION foo UPDATE TO 'v';` when source pins a version different from the installed one.<br>**Tests:** tier-C: `objects/extensions/version-pin-noop`, `version-unpinned-noop`, `lint-unpinned-warning` |

## Triggers

**Tests (whole Triggers section):** tier-1: `crates/pgevolve-core/src/ir/trigger.rs::tests`, `parse/builder/create_trigger_stmt.rs::tests`, `diff/triggers.rs::tests`, `plan/rewrite/triggers.rs::tests`, `plan/rewrite/emit/trigger.rs::tests`, `lint/rules/trigger_references_unmanaged_function.rs`, `lint/rules/trigger_references_unmanaged_table.rs`; tier-C: every fixture under `crates/pgevolve-conformance/tests/cases/objects/triggers/` (12 fixtures) plus `objects/triggers/lint-unmanaged-function`, `lint-unmanaged-table`.

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
- **Event triggers** (`CREATE EVENT TRIGGER`) — a separate object kind, now ✅ Supported. See the [`EVENT TRIGGER`](#objects.event_trigger) row in the table above.
- **`WHEN` clause dependency extraction** — the WHEN predicate is stored as a `NormalizedExpr` for canonical diffing but its column references are not added as explicit dep edges. Renames of referenced columns will surface as a structural diff, prompting a Drop + Create.

## Partitioning (detail)

**Tests (whole Partitioning section):** tier-1: `crates/pgevolve-core/src/ir/partition.rs::tests`, `parse/builder/alter_table_attach_partition.rs::tests`, `plan/rewrite/partitions.rs::tests`, `lint/rules/partition_references_unmanaged_parent.rs`; tier-2: `crates/pgevolve-core/src/catalog/queries/partitions.rs`, `partitioned_tables.rs`; tier-C: every fixture under `objects/partitions/` (12 fixtures) plus `failure/partitions/reject-partition-to-nonpartitioned`, `reject-rekey`.

### IR shape

Partitioning is modeled as two optional fields on `Table` in `pgevolve-core::ir::table`, backed by types in `pgevolve-core::ir::partition`:

**`partition_by: Option<PartitionBy>`** — present on partitioned-parent tables.

| Field | Type | Notes |
|---|---|---|
| `strategy` | `PartitionStrategy` | `Range` \| `List` \| `Hash` |
| `columns` | `Vec<PartitionColumn>` | Ordered partition key elements |

Each `PartitionColumn` carries `kind: PartitionColumnKind` (`Column(Identifier)` or `Expr(NormalizedExpr)`), an optional `collation: Option<QualifiedName>`, and an optional `opclass: Option<QualifiedName>`.

**`partition_of: Option<PartitionOf>`** — present on partition-child tables.

| Field | Type | Notes |
|---|---|---|
| `parent` | `QualifiedName` | Schema-qualified parent table name |
| `bounds` | `PartitionBounds` | The `FOR VALUES …` clause |

`PartitionBounds` variants:

| Variant | Syntax | Fields |
|---|---|---|
| `Range { from, to }` | `FOR VALUES FROM (…) TO (…)` | `from: Vec<BoundDatum>`, `to: Vec<BoundDatum>` |
| `List { values }` | `FOR VALUES IN (…)` | `values: Vec<BoundDatum>` |
| `Hash { modulus, remainder }` | `FOR VALUES WITH (MODULUS m, REMAINDER r)` | `modulus: u32`, `remainder: u32` |
| `Default` | `DEFAULT` | — |

`BoundDatum` — `Literal(NormalizedExpr)` | `MinValue` | `MaxValue`.

A table may have both `partition_by` and `partition_of` set simultaneously (sub-partitioning).

### Source surface — three syntactic forms

All three forms parse into the same `Table` IR:

| Form | Source syntax | Notes |
|---|---|---|
| Form 1 (inline) | `CREATE TABLE child PARTITION OF parent FOR VALUES …` | Parent and child in the same file or directory; child inherits columns from parent. |
| Form 2 (standalone) | `CREATE TABLE child PARTITION OF parent FOR VALUES …` in a separate file | Identical parse result to Form 1. |
| Form 3 (attach) | Plain `CREATE TABLE child (…)` + separate `ALTER TABLE parent ATTACH PARTITION child FOR VALUES …` | The parser merges the `ATTACH PARTITION` statement into the child's `partition_of`, producing the same IR as Form 2. A conformance fixture verifies Form 2 and Form 3 generate identical plans. |

### Catalog reader

Two catalog queries:

- **`SELECT_PARTITIONED_TABLES`** — `pg_class.relkind = 'p'` + `pg_get_partkeydef(c.oid)`. Reads the partition key definition for each partitioned-parent table and re-parses it into `PartitionBy`. Filters: NOT extension-owned.
- **`SELECT_PARTITIONS`** — `pg_class.relispartition = true` + `pg_get_expr(c.relpartbound, c.oid)`. Reads the partition bounds text and re-parses it into `PartitionOf`. Joins `pg_inherits` to get the parent name. Filters: NOT extension-owned; scoped to managed schemas.

Both queries apply the `NOT EXISTS (pg_depend deptype='e')` filter consistent with every other catalog query.

### Differ

| Scenario | Change variant |
|---|---|
| `partition_of` present in source, absent in catalog | `TableChange::AttachPartition { parent, child, bounds }` |
| `partition_of` absent in source, present in catalog | `TableChange::DetachPartition { parent, child }` |
| `partition_of` present on both sides, bounds differ | `TableChange::DetachPartition` + `TableChange::AttachPartition` (rebound) |
| `partition_by` present on both sides, strategy or key differs | `UnsupportedDiff` — no safe in-place rekey path in Postgres |
| Either side is a partition (`partition_of.is_some()`) | Column and constraint diff suppressed — partition children inherit columns from the parent |

### Planner steps

| Step kind | Description |
|---|---|
| `AttachPartition` | `ALTER TABLE parent ATTACH PARTITION child FOR VALUES …` — non-destructive |
| `DetachPartition` | `ALTER TABLE parent DETACH PARTITION child` — destructive; gated on intent approval |

`AttachPartition` is ordered in the same post-create bucket as `CreateIndex` (after the parent and child tables both exist). `DetachPartition` is ordered in the same destructive bucket as `DropTable`.

For a `CreateTable` on a partition child (Form 1 / Form 2 source), the planner emits the `CREATE TABLE … PARTITION OF parent FOR VALUES …` SQL directly; no separate `AttachPartition` step is needed. `AttachPartition` is emitted only when an existing standalone table is being attached to a parent, or when bounds are rebounding.

### Dependency edges

| Edge | Meaning |
|---|---|
| `Table (child partition) → Table (parent)` | Parent table must exist before the child partition is created or attached |

The edge is `DepSource::Structural`. It ensures that when both a parent and a child partition are new, the parent's `CreateTable` is ordered before the child's.

### Lint rules

| Rule | Severity | Condition |
|---|---|---|
| `partition-references-unmanaged-parent` | Error | `partition_of.parent` schema is not in `[managed].schemas` |

### Out of scope / notable gaps

- **`DETACH PARTITION CONCURRENTLY`** — not emitted. The non-concurrent `DETACH PARTITION` is used, which takes an `AccessExclusiveLock`. Concurrent detach is listed as ⛔ not planned for now.
- **`FOREIGN TABLE PARTITION OF`** — foreign-table partitions are not modeled. Foreign tables are 🔮 Future.
- **Per-partition `TABLESPACE` and storage parameters** — partition bounds + reloptions are modeled (partitions are `Table` in IR, so they inherit table reloptions automatically). Per-partition `TABLESPACE` overrides are ✅ Supported (see the `TABLESPACE` row under Storage and physical layout).
- **Partition pruning at plan time** — pgevolve does not skip unaffected partitions when only the parent changes. All managed partitions are included in every diff. Pruning is 🔮 Future.
- **Pre-flight partition-overlap detection** — pgevolve does not validate that declared bounds are non-overlapping before applying. Postgres enforces this at DDL time; a failed `ATTACH PARTITION` will surface as an apply error.

## Security and roles

| Object | Status | Notes |
|---|---|---|
| `ROLE` (`CREATE ROLE / USER`) | ✅ Supported | Cluster-level surface (`pgevolve cluster …`). Full attribute matrix + role membership. Passwords intentionally not modeled — set out-of-band. See [`cluster.md`](./cluster.md).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/cluster/role.rs::tests`, `parse/cluster/create_role.rs`, `parse/cluster/alter_role.rs`, `diff/cluster.rs::tests`; tier-2: `crates/pgevolve-core/tests/cluster_parse.rs`, `cluster_catalog.rs`; tier-C: `cluster/roles/` (8 fixtures) |
| `GRANT` / `REVOKE` (object permissions) | ✅ Supported | Per-object `grants: Vec<Grant>` on all 8 grantable IR types (Schema, Sequence, Table, View, MV, Function, Procedure, UserType). Column-level grants on tables/views/MVs. Lenient drift policy: catalog grants to roles outside source surface as `grants-to-unmanaged-role` warning, never silently revoked. See [`grants.md`](./grants.md).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/grant.rs::tests`, `diff/grants.rs::tests`, `diff/default_privileges.rs::tests`, `diff/owner_op.rs::tests`; tier-2: `crates/pgevolve-core/tests/catalog_grants.rs`; tier-C: `objects/grants/` (7 fixtures) |
| Row-level security policies (`POLICY`) | ✅ Supported | Per-table `rls_enabled` + `rls_forced` flags + embedded `policies: Vec<Policy>`. USING / WITH CHECK use NormalizedExpr canon (shared with check constraints). Command-kind changes go through DROP + CREATE. See [`policies.md`](./policies.md).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/policy.rs::tests`, `diff/policies.rs::tests`, `plan/rewrite/policies.rs::tests`; tier-2: `crates/pgevolve-core/tests/catalog_policies.rs`; tier-C: `objects/policies/` (11 fixtures) |
| Security barriers / leakproof flags | 🔮 Future | Less commonly used; lands alongside fine-grained policy review. |
| `SECURITY LABEL` | ⛔ Not planned | Used primarily by SE-Linux integration; out of scope. |

## Replication and federation

| Object | Status | Notes |
|---|---|---|
| `PUBLICATION` | ✅ Supported | Logical-replication source-side metadata. All 5 forms (explicit FOR TABLE, FOR ALL TABLES, FOR TABLES IN SCHEMA PG15+, row filters PG15+, column lists PG15+). publish bitset + publish_via_partition_root. Lenient drift via unmanaged-publication. change_kinds: [create, drop, replace, alter_add_table, alter_drop_table, alter_set_table, alter_add_schema, alter_drop_schema, alter_set_publish, alter_set_via_root, comment_on]<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/publication.rs::tests`, `parse/builder/publication_stmt.rs`, `diff/publications.rs`; tier-C: `objects/publications/` (12 fixtures) |
| `SUBSCRIPTION` | ✅ Supported | Logical-replication subscriber-side metadata. Per-field lenient WITH options (enabled, slot_name, binary, streaming, two_phase, disable_on_error PG15+, password_required PG16+, run_as_owner PG16+, origin PG16+, failover PG17+). CONNECTION supports `${VAR}` env-var interpolation resolved at apply preflight; plan.sql stores unresolved placeholders. Lenient drift via unmanaged-subscription; hard-error on plaintext password in source. change_kinds: [create, drop, alter_connection, alter_add_publication, alter_drop_publication, alter_set_options, comment_on]<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/subscription.rs::tests`, `parse/builder/subscription_stmt.rs`, `diff/subscriptions.rs`; tier-C: `objects/subscriptions/` (12 fixtures) |
| `FOREIGN DATA WRAPPER` (`FDW`) | 📋 Planned, v0.5.0 | First-class FDW lifecycle (`CREATE SERVER`, `USER MAPPING`, `IMPORT FOREIGN SCHEMA`). See [`roadmap.md`](./roadmap.md). |
| `FOREIGN TABLE` | 📋 Planned, v0.5.0 | Lands with FDWs. See [`roadmap.md`](./roadmap.md). |

## Storage and physical layout

| Object | Status | Notes |
|---|---|---|
| <a id="objects.tablespace"></a>`TABLESPACE` | ✅ Supported | Cluster-level object (bare name, no schema), managed via the `pgevolve cluster …` surface. `CREATE TABLESPACE` (with `OWNER`, `LOCATION`, `WITH (options)`), `ALTER … OWNER TO`, `ALTER … SET (options)`, `DROP` (intent-gated), and `COMMENT ON`. Lenient owner + lenient options. `LOCATION` is immutable, so a location drift surfaces via the `tablespace-location-drift` advisory rather than a destructive recreate. Filesystem-layout management (directory creation, mount points) stays out of scope. The IR also carries the `tablespace` attribute on tables and indexes; per-partition overrides are ✅ Supported (see row below). Tablespaces are declared in a `tablespaces/` cluster-source directory. change_kinds: [create_tablespace, drop_tablespace, alter_tablespace_owner, set_tablespace_options, comment_on_tablespace] |
| `TABLE … TABLESPACE` / per-partition `TABLESPACE` override | ✅ Supported | `CREATE TABLE … TABLESPACE <ts>` and `CREATE TABLE … PARTITION OF … TABLESPACE <ts>` on regular tables, partitioned parents, and partition children. `ALTER TABLE … SET TABLESPACE` is RequiresApproval on a leaf table (full rewrite + ACCESS EXCLUSIVE lock) and Safe on a partitioned parent (metadata-only, no rewrite). `pg_default` is normalized to the implicit default — declaring `TABLESPACE pg_default` is a no-op and causes no spurious diff. Per-partition overrides are tracked on the `Table.tablespace` field and compared independently of the parent. change_kinds: [set_tablespace] |
| `TABLE ... USING <access method>` | ✅ Supported | Per-table access method on `CREATE TABLE … USING <am>`. Parsed and rendered from the `access_method` attribute on tables, read from `pg_class.relam`. The built-in `heap` is the implicit default, so it is canonicalized to "none" on both source and catalog sides — declaring `USING heap` is a no-op. Changing the access method on an existing table is **not** auto-rewritten (a full table rewrite is out of scope and `ALTER TABLE … SET ACCESS METHOD` is PG 15+); instead the change surfaces via the `table-access-method-change` advisory. |
| `WITH (storage_parameter = ...)` (table reloptions) | ✅ Supported | Typed fields for fillfactor + autovacuum_* + parallel_workers + toast_tuple_target + user_catalog_table + vacuum_truncate; `extra: BTreeMap` for unknown/extension keys. Lenient drift policy. See [`reloptions.md`](./reloptions.md).<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/reloptions.rs::tests`, `diff/reloptions.rs::tests`; tier-2: `crates/pgevolve-core/tests/catalog_reloptions.rs`; tier-C: `objects/reloptions/table-fillfactor`, `table-autovacuum-disabled`, `table-multi-set`, `alter-table-set-after-create`, `partition-inherits-reloptions` |
| Index reloptions | ✅ Supported | Per-AM validation: B-tree 50..=100 fillfactor, GiST 10..=100, SP-GiST 90..=100, BRIN/GIN no fillfactor; fastupdate (GIN), gin_pending_list_limit (GIN), buffering (GiST), deduplicate_items (B-tree), pages_per_range + autosummarize (BRIN).<br>**Tests:** tier-C: `objects/reloptions/index-fillfactor`, `index-brin-pages-per-range`, `index-gin-fastupdate` |
| Materialized view reloptions | ✅ Supported | Same key set as tables (autovacuum_*, fillfactor, etc.).<br>**Tests:** tier-C: `objects/reloptions/mv-fillfactor` |
| Toast options (`STORAGE EXTERNAL` / `EXTENDED` / `PLAIN` / `MAIN`) | ✅ Supported | Per-column TOAST storage; canon strips type-default.<br>**Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/storage_downgrade_not_retroactive.rs::tests`; tier-C: `objects/columns/set-storage-external`, `set-storage-plain-warning`, `set-storage-type-default-noop`, `create-table-with-storage` |
| TOAST compression (`COMPRESSION pglz` / `lz4`) | ✅ Supported | Per-column codec; canon preserves `None` (cluster `default_toast_compression` GUC).<br>**Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/compression_change_not_retroactive.rs::tests`; tier-C: `objects/columns/set-compression-lz4` |

## Operators, casts, collations, text search

| Object | Status | Notes |
|---|---|---|
| `OPERATOR` / `OPERATOR CLASS` / `OPERATOR FAMILY` | 📋 Planned, v0.5.1 | Heavy admin objects; lower priority than user-facing surface. See [`roadmap.md`](./roadmap.md). |
| <a id="objects.cast"></a>`CAST` | ✅ Supported | Managed; `WITH FUNCTION` / `WITHOUT FUNCTION` / `WITH INOUT`; `EXPLICIT` / `ASSIGNMENT` / `IMPLICIT` contexts. `CREATE CAST`, `DROP`, and `COMMENT ON`. `WITH FUNCTION` is constrained to managed SQL/plpgsql functions — source rejects references to unmanaged or built-in functions via the `cast-references-unmanaged-function` lint. System casts (function `oid < 16384`) and extension-owned casts are excluded from introspection. No `ALTER CAST` in Postgres, so any structural change is drop + create; identity is `(source_type, target_type)`. change_kinds: [create_cast, drop_cast, comment_on_cast] |
| `COLLATION` | ✅ Implemented (v0.3.8) | Per-column collation supported since v0.1; `CREATE COLLATION` lands as a first-class IR object in v0.3.8: libc / ICU / PG 17+ builtin providers, deterministic toggle, `COMMENT`, `RENAME`. Source uses `locale = 'X'` shorthand or explicit `lc_collate` + `lc_ctype`; IR always stores the latter. `version` field is read-only (`ALTER COLLATION … REFRESH VERSION` deferred to v0.3.9). Structural changes go through `ReplaceCollation` (destructive). See [`collations.md`](./collations.md).<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/collation.rs::tests`, `parse/builder/create_collation_stmt.rs::tests`, `diff/collations.rs::tests`; tier-C: `objects/collations/` (6 fixtures), `scenarios/column-references-managed-collation`. change_kinds: [create_collation, drop_collation, rename_collation, replace_collation, comment_on_collation, alter] |
| `TEXT SEARCH DICTIONARY` | ✅ Supported | Managed. `CREATE TEXT SEARCH DICTIONARY` (TEMPLATE reference + OPTIONS list), `ALTER … RENAME TO`, `ALTER … SET OWNER`, `ALTER … SET SCHEMA`, `ALTER … (options)`, `DROP`, and `COMMENT ON`. TEMPLATE is an unmanaged environment reference (C-language function; never auto-created or dropped by pgevolve). change_kinds: [create_ts_dictionary, drop_ts_dictionary, rename_ts_dictionary, alter_ts_dictionary_owner, alter_ts_dictionary_schema, alter_ts_dictionary_options, comment_on_ts_dictionary] |
| `TEXT SEARCH CONFIGURATION` | ✅ Supported | Managed. `CREATE TEXT SEARCH CONFIGURATION` (PARSER reference + token→dictionary MAPPING list), `ALTER … ADD MAPPING FOR`, `ALTER … ALTER MAPPING FOR`, `ALTER … DROP MAPPING FOR`, `ALTER … RENAME TO`, `ALTER … SET OWNER`, `ALTER … SET SCHEMA`, `DROP`, and `COMMENT ON`. PARSER is an unmanaged environment reference (C-language function; never auto-created or dropped by pgevolve). `COPY=` on `CREATE CONFIGURATION` is out of scope. **Known limitation:** a functional index or generated column whose expression calls `to_tsvector('schema.config', …)` carries an implicit dependency on that text-search configuration that the dep-graph does NOT track (no expression-level TS-config dep edges); such an index may be ordered before its configuration at apply time. The TS objects themselves round-trip correctly; this is a planner gap to address in a future release. change_kinds: [create_ts_configuration, drop_ts_configuration, rename_ts_configuration, alter_ts_configuration_owner, alter_ts_configuration_schema, add_ts_configuration_mapping, alter_ts_configuration_mapping, drop_ts_configuration_mapping, comment_on_ts_configuration] |
| `TEXT SEARCH PARSER` | ⛔ Not planned | Requires C-language functions; unmanaged environment reference only. |
| `TEXT SEARCH TEMPLATE` | ⛔ Not planned | Requires C-language functions; unmanaged environment reference only. |

## Statistics, rules, and other helpers

| Object | Status | Notes |
|---|---|---|
| `STATISTICS` (`CREATE STATISTICS`) | ✅ Supported | Multi-column statistics objects (ndistinct, dependencies, mcv) + PG14+ expression statistics. Explicit names required (no anonymous form). Granular differ — `ALTER SET STATISTICS` for target, `ReplaceStatistic` for any other change. `unmanaged-statistic` lint. change_kinds: [create_statistic, drop_statistic, replace_statistic, alter_statistic_set_target, comment_on_statistic] |
| `RULE` | ⛔ Not planned | Largely superseded by triggers; pg_query already discourages new rules. |
| `SERVER` (FDW server) | 📋 Planned, v0.5.0 | Lands with FDWs. See [`roadmap.md`](./roadmap.md). |
| `USER MAPPING` | 📋 Planned, v0.5.0 | Lands with FDWs. See [`roadmap.md`](./roadmap.md). |

## What `pgevolve` deliberately does not manage

| Object | Status | Reason |
|---|---|---|
| `DATABASE` itself | ⛔ Not planned | Database creation is a cluster-admin step; pgevolve assumes the DB exists. |
| `TABLESPACE` directories | ⛔ Not planned | Filesystem-level setup. |
| Cluster-wide settings (`postgresql.conf`) | ⛔ Not planned | Different lifecycle and audit story. |
| Backups, restores, and physical replication | ⛔ Not planned | Outside the schema-management remit. |
| Data itself (row contents) | ⛔ Not planned | pgevolve plans never `INSERT` / `UPDATE` / `DELETE`. Data migrations are users' responsibility. |

## PG 18-only features

These features ship only on Postgres 18+. They are *not* part of the
v0.3.6 PG 18 catalog-support work; each gets its own roadmap entry.

| Feature | Status | Notes |
|---|---|---|
| Virtual generated columns (`GENERATED ALWAYS AS (...) VIRTUAL`) | 📋 Planned, v0.4.1 | New `GeneratedKind::Virtual` variant alongside the existing stored generated columns. Requires `[managed].min_pg_version >= 18`. |
| `NOT NULL NOT VALID` constraint variant | 🔮 Future | Allows declaring a NOT NULL constraint without validating existing rows. Useful for large-table migrations. |
