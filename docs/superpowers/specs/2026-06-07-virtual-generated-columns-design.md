---
status: blocked-upstream
target: deferred
sub_spec: virtual-generated-columns
---

# `VIRTUAL` generated columns (PG 18) — design

> **BLOCKED (2026-06-07):** This feature cannot be implemented yet. The Rust
> `pg_query` crate (the binding pgevolve uses, mandated by the constitution)
> wraps libpg_query **17** and rejects `GENERATED ALWAYS AS (expr) VIRTUAL` with
> `syntax error at or near "VIRTUAL"`. The libpg_query **C** library has an
> `18.0.0` tag (2026-05-21), but the gap is the Rust binding (`pganalyze/pg_query.rs`):
> as of 2026-06-07 its latest crates.io release **and its `main` branch** are
> both still `6.1.1` (libpg_query 17), with no PG-18 integration even merged
> (latest commit 2026-03-18 is unrelated build-caching work). So there is no
> workaround: no crates.io release to bump to, no `main` to git-depend on (and
> `cargo publish` forbids git deps anyway), and the constitution rules out
> hand-rolled SQL parsing. The design below is complete and ready; unblock by
> bumping `pg_query` once it ships a PG-18 release, then proceed to writing-plans.

Adds support for PostgreSQL 18 *virtual* generated columns
(`GENERATED ALWAYS AS (expr) VIRTUAL`), a v0.4.1 roadmap row. A virtual
generated column computes its value on read rather than materializing it on
write (the existing `STORED` form). This is an **extension of existing
infrastructure**, not a new object kind: the IR already models generated
columns as `Generated { kind: GeneratedKind, expression }`, where
`GeneratedKind` today has the single variant `Stored`.

Brainstorming decisions:
- **Kind flips are column-recreates.** Postgres has no in-place `ALTER` to
  convert a column between `STORED` and `VIRTUAL`; that requires dropping and
  re-adding the column. A `Stored ↔ Virtual` flip is therefore planned as
  `DROP COLUMN` + `ADD COLUMN` (`RequiresApproval`). This is data-safe: the
  values of a generated column are a pure function of other columns, so
  drop+add recomputes them identically.
- **`None ↔ Some` (plain ↔ generated) is intentionally NOT auto-recreated.**
  Converting a *plain* column (arbitrary user data) into a generated column via
  drop+add would permanently destroy that column's data. The existing behavior
  — `SetColumnGenerated` emits `SET EXPRESSION`, which Postgres rejects on a
  plain column — fails **loudly at apply** rather than silently losing data.
  We preserve that. Extending recreate to this case would convert a safe loud
  failure into silent data loss, which is strictly worse. (`Some → None` is
  already handled non-destructively by the existing `DROP EXPRESSION` render.)
- **Version gate is a plan-time lint, Error severity.** The planner is
  deliberately version-agnostic (no target-PG-major threading). VIRTUAL is
  PG 18+ only, so a new plan-time lint blocks the plan when any column is
  VIRTUAL and `[managed].min_pg_version < 18`, mirroring the existing
  `builtin_provider_requires_pg_17` / `*_feature_requires_pg_version` rules.

---

## §1. IR

`crates/pgevolve-core/src/ir/column.rs`: add a variant to the existing enum.

```rust
/// Generated-column kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedKind {
    /// `STORED` — value materialized on write (PG 12+).
    Stored,
    /// `VIRTUAL` — value computed on read (PG 18+).
    Virtual,
}
```

No change to the `Generated` struct shape — `expression` is shared across both
kinds. `Generated` already derives `PartialEq`/`Eq`, so a kind flip naturally
registers as a difference in the diff. Update the stale doc comment on
`Generated` (currently "PG only supports stored as of v17").

## §2. Parser

`crates/pgevolve-core/src/parse/builder/create_stmt.rs` handles
`ConstrType::ConstrGenerated` and currently hardcodes `GeneratedKind::Stored`.
pg_query's `Constraint` node carries the stored/virtual distinction. Read it and
map `STORED → Stored`, `VIRTUAL → Virtual`. pg_query 6.1.1 ships the PG 18
grammar, so `VIRTUAL` parses. (If pg_query does not expose a dedicated field,
inspect the `generated_when` / location and the constraint's stored flag; verify
against the parsed AST during implementation and decode accordingly.)

## §3. Catalog reader

`crates/pgevolve-core/src/catalog/assemble/tables.rs` reads `attgenerated`
(query `catalog/queries/shared.rs` already selects it) and currently matches
only `"s"`. Extend so `"v"` decodes to `Generated { kind: Virtual, .. }`:

- exclude `"v"` from `default` materialization exactly as `"s"` is (both stored
  and virtual generated columns carry their expression in `pg_attrdef`, surfaced
  via `default_expr`);
- in the `generated` branch, accept `"s" | "v"`, set the kind accordingly, and
  re-parse the expression text through the existing `reparse_expression_text`
  path so source and catalog expressions compare equal.

## §4. Diff

`crates/pgevolve-core/src/diff/columns.rs`, the `target.generated !=
source.generated` block. Refine the single current `SetColumnGenerated` emission
into three cases:

- **both `Some`, same `kind`, expression differs** → keep `SetColumnGenerated`
  (renders `SET EXPRESSION AS (…) {STORED|VIRTUAL}`, valid on PG 17+/18).
  `RequiresApproval`.
- **both `Some`, `kind` differs (`Stored ↔ Virtual`)** → emit a **column
  recreate**: `DROP COLUMN name` + `ADD COLUMN name … GENERATED ALWAYS AS (expr)
  {kind}`. `RequiresApproval` / destructive (table rewrite, column moves to end).
  No `CASCADE` on the drop, so a dependent index/view blocks it loudly rather
  than being silently dropped.
- **`None ↔ Some`** → unchanged from today (no recreate; the existing
  `SetColumnGenerated` path applies, which loud-fails for `None → Some`). This is
  the data-safety decision above.

Implementation note: check whether a column-recreate primitive already exists
(e.g. for incompatible type changes) and reuse it; if not, emit the
`DROP COLUMN` + `ADD COLUMN` table-op pair directly. The recreate must order
drop-before-add on the same column.

## §5. Render

`crates/pgevolve-core/src/plan/rewrite/sql.rs`:

- the `generated_kind()` helper and the column-definition renderer emit
  `VIRTUAL` when `kind == Virtual` (they already emit `STORED`). Used by both the
  `CREATE TABLE` column fragment and `ADD COLUMN`.
- the recreate step reuses the existing `DROP COLUMN` and `ADD COLUMN` SQL
  builders (no new StepKind required if recreate is expressed as the existing
  drop+add table ops).

## §6. Version gate (lint)

New rule `crates/pgevolve-core/src/lint/rules/column_virtual_generated_requires_pg_18.rs`,
wired into `lint/universal.rs::check_plan_time_catalog` alongside the other
`min_pg_version` rules. For every column with `generated.kind == Virtual` while
`min_pg_version < 18`, emit `Severity::Error` with id
`column-virtual-generated-requires-pg-18`. Message names `schema.table.column`,
states VIRTUAL requires PG 18 (`min_pg_version = N`), and suggests raising
`[managed].min_pg_version` to 18 or using `STORED`. Mirror the structure of
`builtin_provider_requires_pg_17.rs`.

## §7. Tests

- **Unit:**
  - parser: `… VIRTUAL` → `Virtual`; `… STORED` still `Stored`.
  - reader: `attgenerated='v'` → `Virtual`, expression decoded, excluded from
    `default`.
  - diff: kind flip → recreate (drop+add, `RequiresApproval`); same-kind
    expression change → `SetColumnGenerated`; `None ↔ Some` unchanged.
  - render: `VIRTUAL` column fragment / `ADD COLUMN` strings.
  - lint: fires at `min_pg_version < 18`, silent at `>= 18`.
- **Conformance** `objects/columns/virtual-generated/`: `create-virtual`,
  `stored-to-virtual` (recreate plan), `virtual-expression-change`, and
  `failure/virtual-requires-pg-18` (lint at `min_pg_version = 16`).
- **E2E** (PG 18 only): create a table with a VIRTUAL generated column, apply to
  an ephemeral PG 18, introspect, assert round-trip convergence. Gate the test
  to the PG 18 container (skip on 14–17).

## §8. Out of scope / non-goals

- Indexes / primary keys / foreign keys / partition keys over virtual columns —
  Postgres enforces its own PG 18 restrictions; pgevolve renders what the source
  declares and lets PG reject the unsupported combinations.
- Automatic plain ↔ generated conversion (`None ↔ Some`) — kept as a loud
  failure to avoid silent data loss (see brainstorming decisions).
- Moving / `MSFUNC`-style or other exotic generated forms; only the
  `GENERATED ALWAYS AS (expr) {STORED|VIRTUAL}` forms are supported.
