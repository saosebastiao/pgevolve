# v0.2 sub-spec #6: Partitioning — design

**Status:** Approved 2026-05-21. Implementation plan to follow.

## Goal

First-class management of Postgres declarative partitioning:

- Partitioned parents — `CREATE TABLE ... PARTITION BY {RANGE,LIST,HASH} (...)`
- Child partitions — both declarative `CREATE TABLE p PARTITION OF
  parent FOR VALUES ...` and explicit `CREATE TABLE p (...);
  ALTER TABLE parent ATTACH PARTITION p FOR VALUES ...;` syntax, both
  normalizing to the same IR.
- Sub-partitioning — a partition may itself be `PARTITION BY ...`.
- All four bound shapes — `FROM ... TO ...` (RANGE), `IN (...)` (LIST),
  `WITH (MODULUS m, REMAINDER r)` (HASH), `DEFAULT`.

Partition-bound changes diff as `DETACH PARTITION` + `ATTACH PARTITION`
with new bounds (data-preserving). Changes to a parent's `PARTITION BY`
clause emit a hard `UnsupportedDiff` error pointing at manual
migration — Postgres has no in-place rekey, and a destructive rebuild
should not be implicit.

## Non-goals

- `ATTACH PARTITION ... CONCURRENTLY`. Not needed in declarative
  tooling.
- `FOREIGN TABLE ... PARTITION OF`. FDW-backed partitions are
  out-of-scope for v0.2.
- Per-partition `TABLESPACE` declarations.
- Pre-flight overlap/gap detection for ATTACH bounds. Postgres
  validates this at ATTACH time; replicating the algorithm is a tar
  pit.
- `ALTER TABLE ... ALTER PARTITION ...` (no such command in PG).
- Index/constraint propagation logic. Postgres handles this
  automatically via partitioned-table semantics; the existing IR
  differ for indexes and constraints should produce the right plan
  without partition-aware changes.

## IR shape

Two new optional fields on the existing `Table` struct:

```rust
pub partition_by: Option<PartitionBy>,
pub partition_of: Option<PartitionOf>,
```

- `partition_by.is_some()` → this Table is a partitioned parent.
- `partition_of.is_some()` → this Table is itself a partition.
- Both set → this Table is a sub-partitioned partition (a partition
  that has its own children).

```rust
pub struct PartitionBy {
    pub strategy: PartitionStrategy,
    pub columns: Vec<PartitionColumn>,
}

pub enum PartitionStrategy { Range, List, Hash }

pub struct PartitionColumn {
    pub kind: PartitionColumnKind,
    pub collation: Option<QualifiedName>,
    pub opclass: Option<QualifiedName>,
}

pub enum PartitionColumnKind {
    Column(Identifier),
    Expr(NormalizedExpr),
}

pub struct PartitionOf {
    pub parent: QualifiedName,
    pub bounds: PartitionBounds,
}

pub enum PartitionBounds {
    Range { from: Vec<BoundDatum>, to: Vec<BoundDatum> },
    List { values: Vec<BoundDatum> },
    Hash { modulus: u32, remainder: u32 },
    Default,
}

pub enum BoundDatum {
    Literal(NormalizedExpr),
    MinValue,
    MaxValue,
}
```

The `NormalizedExpr` reuse means partition expressions and bound
literals canonicalize through the same path as view bodies, function
bodies, default expressions, and trigger `WHEN` predicates.

### IR invariants

1. `partition_by.columns` is non-empty.
2. For HASH strategy, `partition_by.columns.len() == 1` (PG
   restriction).
3. For HASH bounds, `0 <= remainder < modulus` and `modulus > 0`.
4. A partition's `partition_of.bounds` variant matches its parent's
   `partition_by.strategy`. (Validated at differ time, not in IR
   construction — we want IR construction to tolerate temporarily
   incomplete state during parsing.)
5. Default partitions are valid only for `Range` and `List`
   strategies, not `Hash`.

### Sort & dedupe

`catalog::canon::sort_and_dedupe::tables` already exists and stays
unchanged at this layer — partition fields are part of `Table`'s
existing equality.

## Source SQL surface

Three syntactic forms, one IR shape.

### Form 1 — partitioned parent

```sql
CREATE TABLE app.orders (
    id      bigserial,
    region  text NOT NULL,
    placed  timestamptz NOT NULL,
    PRIMARY KEY (id, region)
) PARTITION BY LIST (region);
```

Sets `partition_by` on the resulting `Table`. The rest of the
`CreateStmt` parses as it does today (columns, constraints, etc.).

### Form 2 — declarative child

```sql
CREATE TABLE app.orders_us PARTITION OF app.orders
    FOR VALUES IN ('us', 'ca');
```

Parses as a `Table` with:
- `partition_of: Some(PartitionOf { parent: app.orders, bounds: List { values: [...] } })`
- Empty `columns` (inherited from parent at apply time)
- Empty `constraints` (inherited)

Sub-partitioning chains the same form:

```sql
CREATE TABLE app.orders_us PARTITION OF app.orders
    FOR VALUES IN ('us', 'ca')
    PARTITION BY RANGE (placed);
```

Sets BOTH `partition_of` AND `partition_by` on the same Table.

### Form 3 — explicit attach

```sql
CREATE TABLE app.orders_us (LIKE app.orders INCLUDING ALL);
ALTER TABLE app.orders ATTACH PARTITION app.orders_us
    FOR VALUES IN ('us', 'ca');
```

Parsing the `CREATE TABLE` produces a Table without `partition_of`.
Parsing the subsequent `AlterTableStmt` with an `ATTACH PARTITION`
sub-command finds that already-parsed Table in `catalog.tables` and
back-fills its `partition_of`. If the referenced child is not yet in
the catalog, emit `ParseError::Structural` — ATTACH PARTITION must
follow the CREATE TABLE for the child.

`DETACH PARTITION` in source is rejected (`ParseError::Structural`):
in declarative tooling, "this is no longer a partition" is expressed
by removing the `PARTITION OF` declaration or removing the ATTACH
statement, not by emitting a DETACH.

### Statement classifier

`Statement` enum gains no new variants. The dispatcher in
`parse/mod.rs` is extended so:
- `AlterTableStmt` with an `ATTACH PARTITION` sub-command routes
  through a new `parse/builder/alter_table_attach_partition.rs`
  builder.
- `AlterTableStmt` with anything else continues to be rejected via
  the existing fallthrough.

### Rejected source forms

- `ATTACH PARTITION ... CONCURRENTLY` — `UnsupportedClause`.
- `DETACH PARTITION` in source — `Structural`, with a hint telling
  the user to remove the declaration.
- `CREATE FOREIGN TABLE ... PARTITION OF` — already rejected via the
  existing `UnsupportedObjectKind` path.
- Partition with a `TABLESPACE` clause — `UnsupportedClause`.
- Per-partition `WITH (...)` storage parameters — punt to v0.3 with
  a clear `UnsupportedClause` error.

## Catalog reader

### Partitioned parents

Augment the existing `tables.sql` reader (or add a sibling query
`partitioned_tables.sql`) that joins:
- `pg_class c` where `c.relkind = 'p'`
- `pg_partitioned_table pt ON pt.partrelid = c.oid`
- `pg_get_partkeydef(c.oid)` → readable strategy + columns text
- Per-column: `pt.partattrs`, `pt.partclass` (opclass OIDs),
  `pt.partcollation`, `pt.partexprs` (for expression keys).

The simplest path is to re-parse `pg_get_partkeydef()`'s output — a
string of the form `LIST (region)` or `RANGE (placed, region)` — back
through the same parser path Form 1 uses. This keeps catalog reads
and source parsing in lockstep (the same pattern triggers and
indexes use).

### Child partitions

For every table with `pg_class.relispartition = true`:
- `pg_get_expr(c.relpartbound, c.oid)` → bound text such as
  `FOR VALUES IN ('us', 'ca')` or `FOR VALUES FROM (1) TO (10)`.
- `inhparent` (from `pg_inherits`) → parent OID → qualified name.

Re-parse the bound text through the source builder for
`PARTITION OF ... FOR VALUES ...`. Same single-source-of-truth
discipline as `pg_get_triggerdef` / `pg_get_indexdef`.

### Exclusions

- Extension-owned partitioned tables and partitions are filtered via
  the standard `NOT EXISTS (pg_depend WHERE deptype = 'e')` predicate
  used elsewhere.
- Internal partitions (none exist today, but be defensive — filter on
  `c.relkind IN ('r', 'p')` to avoid unexpected `pg_class` rows).

## Differ

The existing `diff::tables::diff_tables(target, source, &mut
ChangeSet)` is extended to compare the two new fields.

### Partitioned-parent diff matrix

| Source.partition_by | Catalog.partition_by | Action |
|---|---|---|
| None | None | (no partition logic involved) |
| Some(P) | None | Reject: cannot turn a non-partitioned table into a partitioned parent in-place. `UnsupportedDiff` error with a hint to manually rebuild. |
| None | Some(P) | Reject (symmetric): cannot un-partition. |
| Some(A) | Some(B), A == B | No change. |
| Some(A) | Some(B), A != B | Reject: changing the partition key is destructive and not supportable in-place. `UnsupportedDiff`. |

### Partition (`partition_of`) diff matrix

| Source.partition_of | Catalog.partition_of | Action |
|---|---|---|
| None | None | (no partition logic involved) |
| Some(P) | None | Emit `AttachPartition` (table already exists as a standalone). Niche but legal in PG. |
| None | Some(P) | Emit `DetachPartition`. The partition becomes a standalone table; data preserved. |
| Some(A) | Some(B), A.parent != B.parent | Two-step: `DetachPartition` from B.parent, then `AttachPartition` to A.parent with A.bounds. |
| Some(A) | Some(B), A.parent == B.parent, A.bounds != B.bounds | `DetachPartition` + `AttachPartition` with new bounds. |
| Some(A) | Some(B), A == B | No change. |

### New `Change` variants

```rust
pub enum TableChange {
    // ... existing variants ...
    AttachPartition { parent: QualifiedName, child: QualifiedName, bounds: PartitionBounds },
    DetachPartition { parent: QualifiedName, child: QualifiedName },
}
```

These hang off the existing `Change::Table(TableChange)` family.
Destructiveness:
- `AttachPartition`: `Safe` (PG validates bounds; data preserved).
- `DetachPartition`: `Safe` (the table becomes standalone; data
  preserved).

A bound change is emitted as `DetachPartition` followed by
`AttachPartition` — the emitter (TRG-style per-family dispatcher)
handles this as a single logical "replace bounds" operation.

### Ordering

- A new child partition's ATTACH must come AFTER both its CREATE
  TABLE (if new) and its parent's CREATE TABLE (if new).
- A DETACH must come BEFORE the parent is dropped, if both are
  changing in the same plan.

Encoded as new dep-graph edges:
- `AttachPartition` step depends on `Table(parent)` and
  `Table(child)`.
- `DetachPartition` step is a prerequisite for `DropTable(parent)`.

## Planner steps + SQL emission

Three new `StepKind` variants:

```rust
AttachPartition,
DetachPartition,
// CreateTable / DropTable are reused
```

SQL emission helpers (`crates/pgevolve-core/src/plan/rewrite/partitions.rs`):

```rust
pub(crate) fn attach_partition(parent: &QualifiedName, child: &QualifiedName, bounds: &PartitionBounds) -> String;
pub(crate) fn detach_partition(parent: &QualifiedName, child: &QualifiedName) -> String;
```

The existing `create_table` emitter is extended to:
- Append `PARTITION BY <strategy> (<cols/exprs>)` when
  `table.partition_by.is_some()`.
- Render `CREATE TABLE child PARTITION OF parent FOR VALUES ...` when
  `table.partition_of.is_some()` and the parent CREATE+ATTACH form
  isn't chosen (see below).

### CREATE form selection

For a brand-new partitioned table family, the planner emits:
1. `CREATE TABLE parent (...) PARTITION BY ...;`
2. For each partition, `CREATE TABLE child PARTITION OF parent FOR VALUES ...;` (Form 2 — the simplest).

For an existing standalone table that source declares as a partition,
the planner emits an `AttachPartition` step against the existing
child.

### Per-family emit dispatcher

A new `emit/partition.rs` (the 14th family file) handles the two new
`TableChange` variants. The existing `emit/table.rs` continues to
handle `CreateTable` / `DropTable` / `AlterTable` etc., and its
`CreateTable` arm picks up the new `partition_by` / `partition_of`
fields automatically once `rewrite::tables::create_table` renders
them.

## Lints

One new universal lint:

- **`partition-references-unmanaged-parent`** (`Error`) — fires when
  a Table has `partition_of: Some(P)` but `P.parent` is not in
  `catalog.tables` nor synthesizes from a partition in
  `catalog.tables`. Pattern matches the
  `trigger-references-unmanaged-{table,function}` rules from
  sub-spec #5.

No new lint for bound overlap — Postgres's ATTACH PARTITION already
rejects with a clear error.

## Dep edges

New edges in `plan/edges.rs`:

- `NodeId::Table(child)` → `NodeId::Table(child.partition_of.parent)`
  when `child.partition_of.is_some()`.

Inside the partition expression / bound literals, any column or
function reference participates in the same edge-walking that already
exists for view bodies, function bodies, and trigger WHEN clauses.

`NodeId` gains no new variants — partitions live at `NodeId::Table`
like any other table.

## Conformance fixtures

A new directory `tests/cases/objects/partitions/` with at least these
fixtures:

1. `create-range-parent-and-two-partitions/` — parent + 2 RANGE
   partitions in a single fixture.
2. `create-list-parent/` — LIST strategy.
3. `create-hash-parent-and-partitions/` — HASH with MODULUS/REMAINDER.
4. `create-default-partition/` — adds a DEFAULT partition to a LIST
   parent.
5. `add-partition/` — adds a new partition to an existing parent
   family.
6. `drop-partition/` — removes a partition (DROP, not DETACH).
7. `replace-bounds/` — changes one partition's `FOR VALUES FROM..TO`.
   Expects `DetachPartition` + `AttachPartition`.
8. `attach-existing-standalone/` — table exists as standalone in
   before.sql; after.sql declares it a partition; expects
   `AttachPartition`.
9. `detach-to-standalone/` — symmetric reverse of #8.
10. `subpartitioned/` — parent → child (itself partitioned) → grandchildren.
11. `lint-unmanaged-parent/` — partition declares
    `PARTITION OF external.t`, parent not in source; expects the
    lint to fire.
12. `reject-rekey/` — source changes a parent's PARTITION BY strategy;
    expects `UnsupportedDiff` error at plan time.
13. `reject-partition-to-nonpartitioned/` — source removes
    `PARTITION BY` from a parent; expects `UnsupportedDiff`.
14. `attach-form-vs-declarative-form-equivalent/` — same end-state
    written two ways; produces an identical plan (or no-op against
    each other).

## Out of scope (post-v0.2)

- `ATTACH PARTITION ... CONCURRENTLY`.
- Foreign-table partitions.
- Per-partition `TABLESPACE`, storage params, fillfactor.
- Generated columns as partition keys (PG 16+).
- Pre-flight overlap detection.
- `ALTER INDEX ... ATTACH PARTITION` for partition-of-index — the
  existing index differ produces correct DDL for declarative
  partitioned indexes without partition-aware special-casing.

## Validates which earlier specs

- `docs/spec/objects.md` partitioning row, currently "Planned".
- The v0.2 arch-readiness spec §17 line item on partitioning being
  expressible as additive fields on `Table` rather than a new
  top-level family.
- The IR-mutator / property-test scaffolding from sub-spec #2 (types)
  — partitioning shows that nested IR (PartitionBy.columns,
  PartitionBounds.from/to) works through the canonicalization
  pipeline.

## Risks + mitigations

| Risk | Mitigation |
|---|---|
| `pg_get_partkeydef()` output drifts across PG versions | Re-parse through the source parser; pin via conformance fixtures on PG 14-17. |
| Sub-partitioning recursion bombs the parser | Bound depth via the existing parser's expression recursion limits; add a depth assertion in the partition builder if needed. |
| ATTACH/DETACH bound rendering disagrees with PG's expected format | Conformance fixtures bless against actual PG output. |
| Differ emits ATTACH before CREATE TABLE for the partition | New dep edge `partition → parent` plus the existing `create_table` topo-sort handles this. |
| User accidentally rekeys a parent and loses data | `UnsupportedDiff` reject — never silently destructive. |
