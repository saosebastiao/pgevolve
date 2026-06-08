---
status: design
target: v0.4.x
sub_spec: table-tablespace
---

# Table-level `TABLESPACE` (incl. per-partition) — design

Adds the `TABLESPACE` placement clause to managed tables — the v0.4.x
"per-partition `TABLESPACE`" roadmap row. Because a partition is just a table
and `CREATE TABLE … TABLESPACE`, `ALTER TABLE … SET TABLESPACE`, and
`pg_class.reltablespace` are identical for regular tables, partitioned parents,
and partition children, the feature is implemented **uniformly across all
tables**; the per-partition override falls out naturally (a partition child is a
table with its own `tablespace`).

This is **net-new, end-to-end** work: `Table` currently has no `tablespace`
field. (The existing `Index.tablespace` field is vestigial — the reader never
populates it and the diff never acts on it — so there is no complete in-repo
template; it stays untouched, out of scope.)

Brainstorming decisions:
- **General table tablespace, not partition-only.** Restricting to
  `PARTITION OF` tables would require artificial special-casing and would
  silently ignore a regular table's `TABLESPACE`. One uniform mechanism instead.
- **Move destructiveness is split by table kind.** `ALTER TABLE … SET
  TABLESPACE` on a **leaf** table (regular table or partition child) rewrites all
  rows under an `ACCESS EXCLUSIVE` lock → `RequiresApproval`. On a **partitioned
  parent** (`partition_by.is_some()`) it only changes the default for *future*
  partitions (no data moves) → `Safe`. `CREATE TABLE … TABLESPACE` (new, empty
  table) is always `Safe`.
- **Tablespace is an environment reference, not a managed dependency.** A table
  names a tablespace that must already exist in the cluster (tablespaces are
  cluster infra, like roles). We render the name; there is **no closed-world
  managed-tablespace constraint** and no new lint. If the tablespace is absent,
  Postgres errors at apply.

---

## §1. IR

`crates/pgevolve-core/src/ir/table.rs`: add

```rust
/// Tablespace placement (`TABLESPACE <name>`). `None` = the database
/// default (`pg_default`). Applies to regular tables, partitioned parents
/// (sets the default for future partitions), and partition children
/// (overrides the parent default).
pub tablespace: Option<Identifier>,
```

to `Table`, defaulting to `None` in every `Table` literal / `Table::empty()`
site. Include `tablespace` in the structural-diff field comparison (the `eq.rs`
/ `difference` field list that drives `Table` change detection) and in the
`Table` `Debug`-field diff helper if it enumerates fields.

## §2. Parser

`crates/pgevolve-core/src/parse/builder/create_stmt.rs`:
- `CreateStmt.tablespacename` (a `String`, empty when absent) → `Table.tablespace`
  (`Some(Identifier)` when non-empty). Covers `CREATE TABLE … TABLESPACE x` and
  `CREATE TABLE … PARTITION OF … TABLESPACE x` (same field).
- `ALTER TABLE x SET TABLESPACE y`: the `AlterTableStmt` command list contains an
  `AlterTableCmd` with subtype `AT_SetTableSpace` and `name = y`. Apply it to the
  table identity in the parse accumulator (mirror how other `ALTER TABLE`
  subcommands mutate the accumulated `Table`). Reject nothing new here — a
  source `ALTER TABLE … SET TABLESPACE` simply sets the field.

## §3. Catalog reader

`crates/pgevolve-core/src/catalog/queries/shared.rs` (the table/`pg_class`
query): add `reltablespace` and resolve it —
`CASE WHEN c.reltablespace = 0 THEN NULL ELSE (SELECT spcname FROM
pg_tablespace WHERE oid = c.reltablespace) END AS tablespace` (or a `LEFT JOIN
pg_tablespace ts ON ts.oid = c.reltablespace`). `reltablespace = 0` →
`Table.tablespace = None` (database default). `assemble/tables.rs` decodes the
column into the field.

## §4. Canon

`crates/pgevolve-core/src/ir/canon/…` `filter_pg_defaults` (the existing
PG-default-stripping pass): normalize `Table.tablespace == Some("pg_default")`
→ `None`, so an explicit `TABLESPACE pg_default` in source round-trips equal to
the reader's `None`. Mirrors how other implicit PG defaults are stripped.

## §5. Diff + destructiveness

In the table differ (`diff/tables.rs` / `diff/columns.rs` sibling that walks
table-level attributes), a changed `tablespace` emits a new
`TableOp::SetTableSpace { name: Option<Identifier> }` (where `None` renders as
`pg_default`). Destructiveness:
- target is a **partitioned parent** (`source.partition_by.is_some()`) →
  `Destructiveness::Safe` (metadata-only; PG does not move existing partitions).
- otherwise (**leaf**: regular table or partition child) →
  `Destructiveness::RequiresApproval { reason: "SET TABLESPACE rewrites the table
  and takes an ACCESS EXCLUSIVE lock" }`.

`CREATE TABLE … TABLESPACE` (a brand-new table) carries the clause inline and is
always `Safe` (the table is empty).

## §6. Render

`crates/pgevolve-core/src/plan/rewrite/`:
- `CREATE TABLE` gains a trailing ` TABLESPACE <name>` clause when
  `tablespace.is_some()` (rendered via `Identifier::render_sql`).
- `TableOp::SetTableSpace { name }` → `ALTER TABLE <qname> SET TABLESPACE
  <name | pg_default>;`, a new `StepKind::SetTableSpace`, `InTransaction`.
  (A move to the default renders `SET TABLESPACE pg_default`.)

## §7. Tablespace reference (no managed constraint)

The table names a tablespace that must pre-exist in the cluster. We render the
name verbatim; there is no closed-world check that the tablespace is managed and
no new lint (consistent with how role references are treated — environment
infra). No dependency-graph edge to a tablespace node.

## §8. Tests

- **Conformance** under `objects/tables/` (or `partitions/`):
  - `create-with-tablespace` — `CREATE TABLE … TABLESPACE ts`.
  - `partition-child-overrides-parent-tablespace` — parent default `ts_a`, one
    child `TABLESPACE ts_b`.
  - `alter-set-tablespace` — a leaf table moves tablespace → `RequiresApproval`.
  - `parent-default-tablespace-change` — a partitioned parent's tablespace
    changes → `Safe`.
  - `tablespace-pg_default-is-noop` — source `TABLESPACE pg_default` vs live
    `None` → no change (canon strip).
  Fixtures that apply must `CREATE TABLESPACE` the referenced tablespace(s) in
  their setup SQL (or use the conformance harness's tablespace provisioning;
  check how the cluster-tablespace fixtures provide one).
- **E2E** (real PG): provision a second tablespace, create a table in it,
  introspect, assert round-trip convergence. Docker-guarded.
- **Unit:** parser (`CREATE … TABLESPACE`; `ALTER … SET TABLESPACE`); reader
  decode (`reltablespace = 0` → `None`; non-zero → name); canon
  (`pg_default` → `None`); diff (Safe on parent, RequiresApproval on leaf,
  Safe on create); render strings (CREATE clause + ALTER SET, incl. `pg_default`).

## §9. Out of scope / non-goals

- Cluster-level `CREATE TABLESPACE` (shipped v0.4.0) and tablespace filesystem
  layout.
- Index tablespace — the vestigial `Index.tablespace` field stays unpopulated;
  wiring it is a separate future cleanup.
- Moving existing partitions when a partitioned parent's default tablespace
  changes (Postgres itself does not do this).
