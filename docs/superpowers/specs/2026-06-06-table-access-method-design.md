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
- **An AM change is advisory-only — pgevolve never auto-rewrites a table's
  access method.** When an existing table's access method differs from source,
  the diff emits no change and a `table-access-method-change` advisory lint.
  Rationale: changing a table's AM is a heavy full-table rewrite, and the
  planner is deliberately version-agnostic (it emits plans valid across the
  whole PG 14–18 support window). The statement that performs the change,
  `ALTER TABLE … SET ACCESS METHOD`, is PG 15+ with no PG-14-compatible form,
  so there is no support-window-wide step to emit — and auto-rewriting a table
  is exactly the kind of heavy operation pgevolve leaves to the operator
  (cf. tablespace location-drift). The advisory points the user at the manual
  `ALTER TABLE … SET ACCESS METHOD` (PG 15+).

`CREATE TABLE … USING <am>` *is* valid across all of PG 14–18, so a new table
carries its access method inline. Because `heap` is the only access method
that ships with Postgres, a non-`heap` AM cannot be applied in CI (no
extension provides one); the parser/reader/canon/diff/lint paths are
unit-tested, and end-to-end coverage is limited to proving `USING heap` is a
no-op.

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
  `CreateTable` change → rendered as `USING <am>` (§6). No separate change.
- **Existing table**, `source.access_method` is `Some(am)` and differs from
  `target.access_method`: emit **no change**. The `table-access-method-change`
  lint (§7) surfaces it. (Add a code comment: pgevolve does not auto-rewrite a
  table's access method.)
- **Lenient**: `source.access_method == None` → no change, regardless of live.

No new `Change` variant, `StepKind`, or `ALTER TABLE` step is introduced — the
only emitted SQL is the inline `USING` on `CreateTable`.

## §6. Render

`CREATE TABLE … (cols) USING <am>` — the create-table renderer appends
`USING <am>` when `access_method` is `Some` (after the column/constraint list,
in the position PG's grammar accepts: after `)` and before `WITH (...)` /
`TABLESPACE`). No other rendering changes.

## §7. Lint

`table-access-method-change` (per-DB lint): when a table exists in both source
and live with differing access methods, emit an advisory ("table `<name>`
access method differs: live=`<a>`, source=`<b>` — pgevolve does not rewrite a
table's access method; run `ALTER TABLE … SET ACCESS METHOD` manually (PG 15+)
if intended"). Informational; never blocks. (Does not fire for a brand-new
table — that's handled by the inline `USING` on create.)

## §8. Tests

- **Conformance** `objects/tables/using-heap-is-noop`: a table declared `USING
  heap` in source against a live heap table → **0 plan steps** (proves canon
  normalization; this is the only AM exercisable against real PG).
- **Unit tests**: parser (`USING foo` → `Some("foo")`, no clause → `None`);
  reader (`relam`/`amname` decode); canon (`Some("heap")` → `None`, `Some("foo")`
  preserved); diff (new table renders inline `USING`; AM change on an existing
  table → no change; source `None` → nothing); lint (`table-access-method-change`
  fires on a differing-AM existing table, silent for new table / matching AM);
  render (`CREATE … USING foo`).

## §9. Out of scope / non-goals

- `CREATE ACCESS METHOD` (AM definitions — extension territory).
- Real-PG apply of a non-`heap` AM (none ships with Postgres; unit-tested only).
- **Auto-changing a table's access method** — advisory only (heavy rewrite;
  `ALTER TABLE … SET ACCESS METHOD` is PG 15+ with no support-window-wide form,
  and the planner is version-agnostic by design). The operator runs it manually.
- Storing `heap` explicitly (normalized to `None`).
