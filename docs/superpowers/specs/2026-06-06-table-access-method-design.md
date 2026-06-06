---
status: design
target: v0.4.0
sub_spec: table-access-method
---

# `TABLE … USING <access method>` — design

Adds support for a table's access method (the last v0.4.0 roadmap row).
Postgres tables can specify a non-default table access method
(`CREATE TABLE … USING <am>`); the default is `heap`, and extensions can
provide others (columnar AMs, `zheap`, etc.). pgevolve currently assumes
`heap` implicitly and silently ignores any mismatch. This feature models the
attribute, reads it back, and converges it where Postgres allows.

Scope is one field on `Table`. `CREATE ACCESS METHOD` itself stays out of
scope — pgevolve manages tables' use of an AM, not AM definitions (which come
from extensions).

Brainstorming decisions:
- **`heap` normalizes to `None`.** Canon strips `access_method = Some("heap")`
  to `None` on both the source and the catalog-reader sides, so `USING heap`,
  an omitted clause, and a live default-heap table are all equivalent — no
  spurious diff. (`None` = "inherit the cluster default", which is `heap` in
  practice.)
- **Lenient.** A source that does not declare an access method
  (`access_method = None`) is unmanaged: the diff emits nothing, never
  changing a live table's AM the user didn't ask about.
- **Change is version-gated.** `ALTER TABLE … SET ACCESS METHOD` is **PG 15+**
  and performs a full-table rewrite. On PG 15+ an AM change emits that ALTER,
  destructive and intent-gated. On PG 14 (no ALTER form) the diff emits no
  change and a `table-access-method-change-unsupported` advisory lint.

A consequence of `heap` being the only access method that ships with
Postgres: a non-`heap` AM cannot be applied in CI (no extension provides
one), so the ALTER/rewrite path is unit-tested only; end-to-end coverage is
limited to proving `USING heap` is a no-op.

---

## §1. IR

`crates/pgevolve-core/src/ir/table.rs` — `Table` gains:
```rust
    /// Table access method (`CREATE TABLE … USING <am>`). `None` = inherit the
    /// cluster default (`heap`). Canon normalizes `Some("heap")` → `None`.
    pub access_method: Option<Identifier>,
```
Open-ended `Identifier` (not a closed enum like `IndexMethod`), because table
AMs are extension-provided. Default `None`. Update `Table` constructors/tests
that build struct literals.

## §2. Parser

`crates/pgevolve-core/src/parse/builder/` (the `CreateStmt`/table builder):
read `CreateStmt.access_method` (a `String`, empty when no `USING` clause) →
`access_method = (!s.is_empty()).then(|| Identifier::from_unquoted(s))`. The
canon pass (`§4`) normalizes `heap` afterward, so the parser stores it
verbatim.

## §3. Catalog reader

`crates/pgevolve-core/src/catalog/assemble/tables.rs` + the table query: join
`pg_class.relam` → `pg_am.amname` and set `access_method = Some(amname)`. Canon
normalizes `heap` → `None`, so the reader can store it verbatim (no special
heap handling at read time).

## §4. Canon

`crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs` (the pass that maps
PG documented defaults to `None`): for each table, if
`access_method == Some("heap")` set it to `None`. Runs on both source and
live IR, so the two normalize identically.

## §5. Diff

`crates/pgevolve-core/src/diff/tables.rs`:
- **New table** (not in target): `access_method` rides inline in the
  `CreateTable` change → rendered as `USING <am>`.
- **Existing table**, `source.access_method` is `Some(am)` and differs from
  `target.access_method`:
  - **PG 15+**: emit `AlterTableAccessMethod { qname, method }`, destructiveness
    `RequiresApprovalAndDataLossWarning` (full-table rewrite), intent-gated.
  - **PG 14**: emit no change; record a `table-access-method-change-unsupported`
    advisory (§7).
- **Lenient**: `source.access_method == None` → no change, regardless of live.

The PG-major gate uses the planner's existing target-version mechanism (the
same one that gates column `STORAGE` inline-vs-ALTER and subscription
`streaming`). The diff/plan layer that has the version decides which arm to
take; mirror the existing version-gated site.

## §6. Render + plan

- `CREATE TABLE … (cols) USING <am>` — the create renderer appends
  `USING <am>` when `access_method` is `Some` (after the column/constraint
  list, before `WITH (...)`/`TABLESPACE`, per PG grammar).
- `ALTER TABLE <qname> SET ACCESS METHOD <am>;` — new `StepKind`
  `AlterTableAccessMethod`, destructive (rewrite). Standard in-transaction
  step (PG allows `SET ACCESS METHOD` inside a transaction).

## §7. Lint

`table-access-method-change-unsupported` (per-DB lint): when the live and
source tables have differing access methods AND the target is PG 14, emit an
advisory ("cannot change access method of `<table>` on PG 14 — `ALTER TABLE …
SET ACCESS METHOD` requires PG 15+; rebuild the table manually if intended").
Informational; never blocks. (On PG 15+ the change is handled by the ALTER, so
no lint fires.)

## §8. Tests

- **Conformance** `objects/tables/using-heap-is-noop`: a table declared `USING
  heap` in source against a live heap table → **0 plan steps** (proves canon
  normalization; this is the only AM exercisable against real PG).
- **Unit tests**: parser (`USING foo` → `Some("foo")`, no clause → `None`);
  reader (`relam`/`amname` decode); canon (`Some("heap")` → `None`, `Some("foo")`
  preserved); diff (new table inline; AM change on PG 15+ → AlterTableAccessMethod
  intent-gated; on PG 14 → no change + lint; source `None` → nothing); render
  (`CREATE … USING foo`, `ALTER TABLE … SET ACCESS METHOD foo;`).

## §9. Out of scope / non-goals

- `CREATE ACCESS METHOD` (AM definitions — extension territory).
- Real-PG apply of a non-`heap` AM (none ships with Postgres; unit-tested only).
- Changing AM on PG 14 (advisory only — no Postgres mechanism short of a manual
  rebuild).
- Storing `heap` explicitly (normalized to `None`).
