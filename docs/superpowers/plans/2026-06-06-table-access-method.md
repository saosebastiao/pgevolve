# TABLE … USING access method Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Model a table's access method (`CREATE TABLE … USING <am>`) as `Table::access_method: Option<Identifier>`, read it back, render it on create, normalize `heap`→`None`, and surface an advisory when an existing table's AM differs.

**Architecture:** One field on `Table`. Parser reads `CreateStmt.access_method`; reader joins `pg_class.relam`→`pg_am.amname`; canon strips `heap`→`None`; create-renderer appends `USING <am>`; an existing-table AM change emits NO step (planner is version-agnostic; `ALTER TABLE … SET ACCESS METHOD` is PG15+) — a `table-access-method-change` advisory lint surfaces it instead.

**Tech Stack:** Rust, `pg_query` (libpg_query), `pg_catalog` introspection, conformance harness.

**Design:** [`docs/superpowers/specs/2026-06-06-table-access-method-design.md`](../specs/2026-06-06-table-access-method-design.md)

---

## Verified integration sites
- `Table` struct: `crates/pgevolve-core/src/ir/table.rs:14-42` (no `Default` derive; literals are manual — many test/lint literals need the new field).
- Parser: `crates/pgevolve-core/src/parse/builder/create_stmt.rs` (~line 133 Table literal). pg_query `CreateStmt.access_method: String` (tag 11), empty when no `USING`.
- Reader: `crates/pgevolve-core/src/catalog/queries/shared.rs:36-64` (TABLES_QUERY) + `catalog/assemble/tables.rs:139-155`. Index AM join (`pg_am`) precedent at INDEXES_QUERY ~line 191.
- Canon: `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs` (`run()` ~line 29 table loop).
- Diff: `crates/pgevolve-core/src/diff/tables.rs` — `emit_table_attribute_changes` (~line 377) and the synthesized empty target (~line 47-60).
- Render: `crates/pgevolve-core/src/plan/rewrite/sql.rs` `create_table` (~lines 54-99).
- Lint: per-DB lint rules dir `crates/pgevolve-core/src/lint/rules/`; registered in `lint/rules/mod.rs` + invoked from the drift-lint runner in `lint/universal.rs` (find `run_drift_lints` / the per-DB lint entry — mirror an existing per-DB advisory rule like `unmanaged_publication`/a table-level rule).
- Conformance: `crates/pgevolve-conformance/tests/cases/objects/tables/<case>/` (`fixture.toml`+`before.sql`+`after.sql`+`expected/`).

Project rules: no `unwrap`/`expect` in non-test code; `cargo clippy --workspace --all-targets` must be ZERO warnings (`[workspace.lints]`, pedantic+nursery); `cargo fmt` before commit. Co-author trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. **Build/clippy at WORKSPACE level each task** (CLI/conformance crates have exhaustive matches).

---

## Task 1: IR field + struct literals

**Files:** `crates/pgevolve-core/src/ir/table.rs`; all non-`..Default` `Table { … }` literals.

- [ ] **Step 1** — In `Table` (after `storage`):
```rust
    /// Table access method (`CREATE TABLE … USING <am>`). `None` = inherit the
    /// cluster default (`heap`). Canon normalizes `Some("heap")` → `None`.
    pub access_method: Option<Identifier>,
```
- [ ] **Step 2** — `cargo build --workspace` → it will error at every `Table { … }` literal missing the field. Add `access_method: None,` to each (the parser/reader literals get real values in Tasks 2/3 — for now `None` keeps them compiling; the test `base()` helper in `table.rs` and all lint/profile literals get `None`). Grep `grep -rn "Table {" crates/pgevolve-core/src crates/pgevolve-testkit/src` to find them all.
- [ ] **Step 3** — Add a unit test in `table.rs` tests: build a `Table` with `access_method: Some(id("columnar"))`, assert the field round-trips through serde (mirror an existing serde test if present; else just `assert_eq!`).
- [ ] **Step 4** — `cargo test -p pgevolve-core --lib ir::table` pass; `cargo build --workspace` clean; `cargo clippy --workspace --all-targets` 0 warnings.
- [ ] **Step 5** — `cargo fmt && git add -A && git commit -m "feat(ir): Table::access_method field

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"`

---

## Task 2: Canon heap→None

**Files:** `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs`.

- [ ] **Step 1** — Add a failing test (in that file's tests): a `Catalog` with one table `access_method: Some(id("heap"))` → after `run`, `access_method == None`; a table with `Some(id("columnar"))` → unchanged.
- [ ] **Step 2** — Run it → fails.
- [ ] **Step 3** — Add a helper + call it in the table loop of `run()`:
```rust
fn normalize_table_access_method(table: &mut Table) {
    if table.access_method.as_ref().is_some_and(|am| am.as_str() == "heap") {
        table.access_method = None;
    }
}
```
Call `normalize_table_access_method(table);` inside the existing `for table in &mut cat.tables { … }` loop (mirror how `normalize_column_*` are called). Add the `Table` import if needed.
- [ ] **Step 4** — test passes; `cargo clippy --workspace --all-targets` 0; `cargo build --workspace` clean.
- [ ] **Step 5** — `cargo fmt && git commit -am "feat(canon): normalize table access_method heap→None …"` (with trailer).

---

## Task 3: Parser

**Files:** `crates/pgevolve-core/src/parse/builder/create_stmt.rs`.

- [ ] **Step 1** — Failing parser test (mirror existing create_stmt tests that parse SQL and assert the `Table`): parse `CREATE TABLE app.t (id bigint) USING columnar;` → `access_method == Some("columnar")`; parse `CREATE TABLE app.t (id bigint);` → `None`; parse `… USING heap;` → `Some("heap")` (canon normalizes later, parser stores verbatim).
- [ ] **Step 2** — Run → fails.
- [ ] **Step 3** — Before the `Table { … }` construction, read the field:
```rust
let access_method = if create.access_method.is_empty() {
    None
} else {
    Some(Identifier::from_unquoted(&create.access_method).map_err(|e| ParseError::Structural {
        location: location.clone(),
        message: format!("invalid access method {:?}: {e}", create.access_method),
    })?)
};
```
(Match the exact `ParseError` variant + `location` handling used by neighboring fields in this builder.) Add `access_method,` to the `Table` literal.
- [ ] **Step 4** — tests pass; clippy 0; build clean.
- [ ] **Step 5** — commit `feat(parse): CREATE TABLE … USING <access method>` (trailer).

---

## Task 4: Catalog reader

**Files:** `crates/pgevolve-core/src/catalog/queries/shared.rs` (TABLES_QUERY); `crates/pgevolve-core/src/catalog/assemble/tables.rs`.

- [ ] **Step 1** — Failing assemble test (mirror existing `assemble/tables.rs` tests using `Row::new().with(...)`): a row with `access_method = "columnar"` → table `access_method == Some("columnar")`; NULL/absent → `None`.
- [ ] **Step 2** — Run → fails.
- [ ] **Step 3** — TABLES_QUERY: add `LEFT JOIN pg_catalog.pg_am am ON am.oid = c.relam` and `am.amname AS access_method` to the SELECT (mirror the INDEXES_QUERY `pg_am` join). In `assemble/tables.rs`, decode after `storage`:
```rust
let access_method = r
    .get_opt_text(q, "access_method")?
    .filter(|s| !s.is_empty())
    .map(|s| Identifier::from_unquoted(&s))
    .transpose()
    .map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "access_method".to_string(),
        message: format!("invalid identifier: {e}"),
    })?;
```
Add `access_method,` to the `Table` literal. (Confirm `get_opt_text` is the real nullable-text accessor by reading neighboring decodes.)
- [ ] **Step 4** — tests pass; clippy 0; build clean.
- [ ] **Step 5** — commit `feat(catalog): read table access method (pg_class.relam → pg_am)` (trailer).

---

## Task 5: Render `USING` on create (+ confirm no diff on change)

**Files:** `crates/pgevolve-core/src/plan/rewrite/sql.rs` (`create_table`); `crates/pgevolve-core/src/diff/tables.rs` (synthesized empty target only).

- [ ] **Step 1** — Failing render test in `sql.rs`: build a `Table` with `access_method: Some(id("columnar"))`, call `create_table`, assert the output contains `) USING columnar` in the right place (after the closing paren of the column/constraint list). Also a test that `access_method: None` renders NO `USING` clause.
- [ ] **Step 2** — Run → fails.
- [ ] **Step 3** — In `create_table`, after the column/constraint list close `)` and any partition clause, before `WITH (...)`/`TABLESPACE`/the final `;`, append:
```rust
if let Some(am) = &t.access_method {
    s.push_str(" USING ");
    s.push_str(&am.render_sql());
}
```
(Verify the exact position against PG's `CREATE TABLE` grammar and the existing clause ordering in `create_table`; `USING` comes after the table element list and before `WITH`/`TABLESPACE`.)
- [ ] **Step 4** — In `diff/tables.rs`, the synthesized empty-target `Table` (~line 47-60) needs `access_method: None,` (compiler-forced from Task 1; confirm). Add a diff test: target table `access_method: Some("columnar")`, source `Some("heap"→None after canon)` or a different AM → `emit_table_attribute_changes` emits **no** access-method change (the field is intentionally not diffed here; the lint handles it). Assert no `Change` mentions access method. (There is NO new Change/StepKind — the create path carries it inline, the change path is lint-only.)
- [ ] **Step 5** — `cargo test -p pgevolve-core --lib plan::rewrite diff::tables` pass; clippy 0; build clean.
- [ ] **Step 6** — commit `feat(render): CREATE TABLE … USING <access method>` (trailer).

---

## Task 6: `table-access-method-change` advisory lint

**Files:** Create `crates/pgevolve-core/src/lint/rules/table_access_method_change.rs`; register in `lint/rules/mod.rs`; invoke from the per-DB drift-lint runner in `lint/universal.rs`.

- [ ] **Step 1** — Read an existing per-DB advisory lint that compares source vs live tables (e.g. find one that takes `(source, target)` catalogs and emits `Finding`s — `grep -rn "fn check" crates/pgevolve-core/src/lint/rules/ | head`, and read `lint/universal.rs` to see the per-DB lint entry point + how rules are invoked with source+target). Mirror its shape + `Finding` advisory API.
- [ ] **Step 2** — Write the rule + tests:
```rust
//! Advisory: an existing table's access method differs between source and live.
//! pgevolve does not rewrite a table's access method (heavy full-table rewrite;
//! `ALTER TABLE … SET ACCESS METHOD` is PG 15+). The operator runs it manually.

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "table-access-method-change";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut out = Vec::new();
    for s in &source.tables {
        if let Some(t) = target.tables.iter().find(|t| t.qname == s.qname) {
            if s.access_method.is_some() && s.access_method != t.access_method {
                out.push(/* advisory Finding RULE_ID, message:
                    "table {qname} access method differs: live={t.access_method:?}, \
                     source={s.access_method:?} — pgevolve does not rewrite a table's \
                     access method; run ALTER TABLE … SET ACCESS METHOD manually (PG 15+)" */);
            }
        }
    }
    out
}
```
Use the EXACT `Finding` advisory constructor a sibling rule uses. Note: `access_method` is post-canon (`heap`→`None`), so `Some(columnar)` vs `None`(heap) correctly fires; `None` source never fires (lenient). Tests: differing AM on existing table → 1 finding; same AM → none; table only in source (new) → none; source `None` → none.
- [ ] **Step 3** — Register `pub mod table_access_method_change;` and invoke `out.extend(rules::table_access_method_change::check(source, target));` in the per-DB drift-lint runner (mirror an existing per-DB rule invocation — match the exact arg order/variable names).
- [ ] **Step 4** — `cargo test -p pgevolve-core --lib lint::rules::table_access_method_change` + `cargo test -p pgevolve --lib` pass; clippy 0; build clean.
- [ ] **Step 5** — commit `feat(lint): table-access-method-change advisory` (trailer).

---

## Task 7: Conformance + docs + full gate

**Files:** new `crates/pgevolve-conformance/tests/cases/objects/tables/using-heap-is-noop/`; `docs/spec/objects.md`; `docs/spec/roadmap.md`; `CHANGELOG.md`; `git rm docs/superpowers/plans/_skeleton/table-access-method.md`.

- [ ] **Step 1** — Read a real `objects/tables/` fixture (`for f in fixture.toml before.sql after.sql expected/plan.sql; do echo "== $f =="; cat crates/pgevolve-conformance/tests/cases/objects/tables/create-simple/$f; done`) to match the format + `[pg] min/max` convention (use min=14, max=18).
- [ ] **Step 2** — Create `using-heap-is-noop/`:
  - `before.sql`: schema + `CREATE TABLE app.t (id bigint NOT NULL, CONSTRAINT t_pkey PRIMARY KEY (id));`
  - `after.sql`: the same but `… ) USING heap;`
  - `fixture.toml`: `[expect.plan] steps = 0` (canon normalizes `USING heap`→None on both sides → no diff). Title "CREATE TABLE USING heap is a no-op".
- [ ] **Step 3** — `cargo run -p xtask -- bless --conformance`; inspect the generated `expected/` (empty plan). Run `cargo test -p pgevolve-conformance` (Docker) → all pass, no regressions. If `using-heap-is-noop` produces ANY step, STOP — canon normalization (Task 2) is broken.
- [ ] **Step 4** — Docs: `objects.md` flip the `TABLE … USING <access method>` row to `✅ Supported` (mirror EVENT TRIGGER/TABLESPACE recently-shipped format); `roadmap.md` move it from Active matrix to Shipped (v0.4.0), plan link `2026-06-06-table-access-method.md` — **this empties the v0.4.0 Active matrix** (note it); `CHANGELOG.md` `[Unreleased]→Added` a bullet (table access method: `USING` on create, read-back, `heap` normalized, AM change is advisory-only via `table-access-method-change`). `git rm` the skeleton.
- [ ] **Step 5** — Full gate: `cargo test --workspace` all pass; `cargo clippy --workspace --all-targets` 0; `cargo fmt --check` clean; `cargo deny check` ok. If tier-3 `catalog_round_trip` snapshots re-blessed, verify diffs are only the additive `access_method` line (per-DB `Catalog` changed, so some may shift — confirm no content regression, like the EVENT TRIGGER snapshot check).
- [ ] **Step 6** — commit `feat(table-access-method): mark shipped — conformance, objects.md, roadmap, CHANGELOG` (trailer).

---

## Self-review notes (coverage vs spec)
- §1 IR → T1. §2 parser → T3. §3 reader → T4. §4 canon heap→None → T2. §5 diff (new-table inline, change→nothing) → T5. §6 render `USING` → T5. §7 lint → T6. §8 tests → unit across T1-T6 + conformance T7. §9 non-goals: no Change/StepKind/ALTER (T5 confirms diff emits nothing), advisory-only change (T6), heap normalized (T2).
- **Type consistency:** `Table::access_method: Option<Identifier>` used identically in parser (T3), reader (T4), canon (T2), render (T5), lint (T6). No new `Change`/`StepKind` — deliberately.
- **Watch:** tier-3 snapshots will re-bless on the new `Catalog` field (T7 Step 5) — verify additive-only.
