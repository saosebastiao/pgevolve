# Table-level `TABLESPACE` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `TABLESPACE` placement clause to managed tables (regular tables, partitioned parents, partition children) — IR field, parser, reader, canon normalization, a `SET TABLESPACE` diff/render path with kind-aware destructiveness.

**Architecture:** `Table` gains `tablespace: Option<Identifier>` (mirrors the existing `access_method` field). Parsed from `CreateStmt.tablespacename` and `ALTER TABLE … SET TABLESPACE`, read from `pg_class.reltablespace`, `pg_default`-normalized to `None` in canon, diffed as a new `TableOp::SetTableSpace` that is **Safe on a partitioned parent** (metadata-only) and **RequiresApproval on a leaf** (rewrite + ACCESS EXCLUSIVE), rendered inline on `CREATE TABLE` and via `ALTER TABLE … SET TABLESPACE`.

**Tech Stack:** Rust, `pg_query`, `pg_catalog` introspection, conformance harness.

**Design:** [`docs/superpowers/specs/2026-06-08-table-tablespace-design.md`](../specs/2026-06-08-table-tablespace-design.md)

**Closest template — the `access_method` field (v0.4.0 table access method).** It is a `Table`-level `Option<Identifier>` handled end-to-end EXCEPT that access-method changes are advisory (no ALTER). For tablespace we reuse its IR/parser/reader/canon shape and ADD a real ALTER op. Read these exact sites:
- IR: `ir/table.rs:44` (`access_method` field) + its `Table::empty`/literal defaults + `eq.rs`/`difference` field list.
- Reader: `catalog/queries/shared.rs:47` + `:55` (`LEFT JOIN pg_am … AS access_method`); `catalog/assemble/tables.rs:139` (decode).
- Canon: `ir/canon/filter_pg_defaults.rs:169` (`normalize_table_access_method`, heap→None) + wiring at `:31`.
- ALTER op template: `TableOp::SetTableComment` — diff `diff/tables.rs:110`, render `plan/rewrite/emit/table.rs:319`.
- Destructiveness template: `TableOp::SetColumnGenerated` uses `Destructiveness::RequiresApproval { reason }` (`diff/columns.rs:155`).

## Verified facts
- pg_query: `CreateStmt.tablespacename: String` (empty when absent). `ALTER TABLE … SET TABLESPACE` is an `AlterTableCmd` with `subtype == AlterTableType::AtSetTableSpace` (=35) and `name` = the tablespace.
- `pg_class.reltablespace = 0` means "database default" (`pg_default`).
- A partitioned parent is `table.partition_by.is_some()`; a partition child is `table.partition_of.is_some()`. Leaf = neither a parent (regular table) OR a child of a parent. For destructiveness, the rule is simply: **`partition_by.is_some()` → Safe; else → RequiresApproval.**

Project rules: no `unwrap`/`expect`/`panic!`/`todo!` in non-test code; `cargo clippy --workspace --all-targets` ZERO warnings; `cargo fmt`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` clean; **build/clippy/doc at WORKSPACE level each task**. Co-author trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Commit directly to `main`.

---

## Task 1: IR field + canon normalization

**Files:** `crates/pgevolve-core/src/ir/table.rs`; `ir/eq.rs` or wherever `Table` structural diff fields are listed; `ir/canon/filter_pg_defaults.rs`; grep for `Table {` / `access_method:` literal sites.

- [ ] **Step 1** — `ir/table.rs`: add `pub tablespace: Option<Identifier>,` to `Table` immediately after `access_method` (line ~44), with a doc comment per the spec §1. Add `tablespace: None` to every `Table` literal and `Table::empty()` — run `grep -rn "access_method: None" crates/pgevolve-core/src` and add a sibling `tablespace: None` at each (these are the same literal sites). Add a `tablespace_field_roundtrips` unit test mirroring `access_method_field_roundtrips` (`ir/table.rs:343`).
- [ ] **Step 2** — Wherever `Table`'s structural-diff field comparison lives (the field that makes `access_method` participate in change detection — check `ir/eq.rs` and the `Table` `Debug`-field-diff helper at `ir/table.rs:52`): add `tablespace` to that list/helper exactly as `access_method`/`partition_by` appear (`ir/table.rs:52-59` shows the field-diff helper pattern).
- [ ] **Step 3** — `ir/canon/filter_pg_defaults.rs`: add `fn normalize_table_tablespace(table: &mut Table)` mirroring `normalize_table_access_method` (`:169`): if `table.tablespace.as_ref().is_some_and(|ts| ts.as_str() == "pg_default")` → set `None`. Call it from the per-table normalization fn right after `normalize_table_access_method(table);` (`:31`). Add tests mirroring `strips_heap_access_method` (`:785`): `pg_default` stripped to `None`; a real tablespace kept. Update the `bare_table` test helper (`:766`) / any full `Table` literal in this file's tests with `tablespace: None`.
- [ ] **Step 4** — Verify: `cargo test -p pgevolve-core --lib ir::table ir::canon::filter_pg_defaults`; `cargo build --workspace`; `cargo clippy --workspace --all-targets` 0; `cargo fmt`. Commit `feat(ir): Table::tablespace + pg_default canon strip`.

---

## Task 2: `TableOp::SetTableSpace` + StepKind + render

**Files:** `crates/pgevolve-core/src/diff/table_op.rs`; `plan/raw_step.rs`; `plan/plan.rs`; `plan/rewrite/emit/table.rs`; `plan/rewrite/sql.rs`.

- [ ] **Step 1** — `diff/table_op.rs`: add a variant
  ```rust
  /// `ALTER TABLE … SET TABLESPACE …` (None = pg_default).
  SetTableSpace { name: Option<Identifier> },
  ```
  next to `SetTableComment` (`:122`). `cargo build -p pgevolve-core` → fix the exhaustive `match TableOp` sites (render dispatch, any Display, any op-name mapping) — mirror `SetTableComment` at each.
- [ ] **Step 2** — `plan/raw_step.rs` + `plan/plan.rs`: add `StepKind::SetTableSpace` (+ serde round-trip test + `kind_name`/`parse_kind_name` entries) mirroring an existing `Set*` table StepKind (e.g. `SetColumnStorage` / the kind used by `SetTableComment`).
- [ ] **Step 3** — `plan/rewrite/sql.rs`: add `pub fn alter_table_set_tablespace(qname: &QualifiedName, name: Option<&Identifier>) -> String` returning `ALTER TABLE <qname> SET TABLESPACE <name|pg_default>;` (when `None`, literal `pg_default`). Render names via `Identifier::render_sql` / `QualifiedName::render_sql`.
- [ ] **Step 4** — `plan/rewrite/emit/table.rs`: add the `TableOp::SetTableSpace { name } => out.push(RawStep { … })` arm mirroring `SetTableComment` (`:319`), `kind: StepKind::SetTableSpace`, `transactional: TransactionConstraint::InTransaction`, `sql: sql::alter_table_set_tablespace(qname, name.as_ref())`. **Destructiveness flows from the diff** (the `destructive`/`destructive_reason` the emit fn receives) — do NOT hardcode it here; just pass through like the other ops that receive it. (Check the emit signature: ops like `SetColumnGenerated` at `:226` receive `destructive`/`destructive_reason` — follow that.)
- [ ] **Step 5** — Also extend the `CREATE TABLE` renderer to append ` TABLESPACE <name>` when `table.tablespace.is_some()`. Find the CREATE TABLE body builder (`grep -n "CREATE TABLE" plan/rewrite/sql.rs` or `emit/table.rs`) and add the clause after the column/partition clause, before the closing `;`, rendered via `Identifier::render_sql`. (None → omit the clause entirely.)
- [ ] **Step 6** — Unit-test the SQL builders: `alter_table_set_tablespace` for `Some("fast")` and `None`→`pg_default`; CREATE TABLE with a tablespace. Verify clippy 0; build (the diff doesn't emit SetTableSpace yet — Task 3 — but render is testable directly). Commit `feat(render): SET TABLESPACE + CREATE TABLE tablespace clause`.

---

## Task 3: Diff + destructiveness split

**Files:** `crates/pgevolve-core/src/diff/tables.rs`.

- [ ] **Step 1** — In `diff_tables` where table-level attributes are compared (the block that emits `SetTableComment` at `:110`), add a `tablespace` comparison: when `source.tablespace != target.tablespace`, push a `TableOpEntry { op: TableOp::SetTableSpace { name: source.tablespace.clone() }, destructiveness }` where:
  ```rust
  let destructiveness = if source.partition_by.is_some() {
      Destructiveness::Safe
  } else {
      Destructiveness::RequiresApproval {
          reason: "SET TABLESPACE rewrites the table and takes an ACCESS EXCLUSIVE lock".into(),
      }
  };
  ```
  (Match the exact `TableOpEntry`/`Destructiveness` construction the `SetTableComment` and `SetColumnGenerated` (`diff/columns.rs:149`) arms use.)
- [ ] **Step 2** — Confirm a brand-new table (source-only) renders its tablespace inline via the Task-2 CREATE TABLE clause (no SetTableSpace op needed for creation — verify the create path uses the full `Table` incl. `tablespace`). No diff change needed for create beyond Task 2's renderer.
- [ ] **Step 3** — Tests in `diff/tables.rs` (mirror the access-method and comment tests):
  - leaf regular table tablespace change → one `SetTableSpace` op, `RequiresApproval`.
  - partition child (`partition_of: Some`, `partition_by: None`) tablespace change → `SetTableSpace`, `RequiresApproval`.
  - partitioned parent (`partition_by: Some`) tablespace change → `SetTableSpace`, `Safe`.
  - `source.tablespace == target.tablespace` → no op.
  - source `Some("pg_default")` is irrelevant here (canon strips it before diff) — but add a test that equal `None`/`None` yields nothing.
- [ ] **Step 4** — Verify `cargo test -p pgevolve-core --lib diff::tables`; clippy 0; build; fmt. Commit `feat(diff): SET TABLESPACE (Safe on parent, RequiresApproval on leaf)`.

---

## Task 4: Parser

**Files:** `crates/pgevolve-core/src/parse/builder/create_stmt.rs`; the `ALTER TABLE` command handler (`grep -rln "AlterTableCmd\|AtSet" crates/pgevolve-core/src/parse`).

- [ ] **Step 1** — `create_stmt.rs`: read `CreateStmt.tablespacename` (a `String`) → `Table.tablespace = (!s.is_empty()).then(|| Identifier::from_unquoted(&s))…` (handle the `Result` from `Identifier::from_unquoted` per the codebase's convention — see how `access_method` reads `CreateStmt.access_method`). This covers CREATE TABLE and CREATE TABLE … PARTITION OF (same field).
- [ ] **Step 2** — In the `ALTER TABLE` subcommand handler, add a case for `subtype == AlterTableType::AtSetTableSpace`: set the accumulated table's `tablespace = Some(Identifier::from_unquoted(&cmd.name)…)`. Mirror how an existing `AlterTableCmd` subtype (e.g. SET/RESET, or the access-method ALTER if present) mutates the accumulated `Table`. If `ALTER TABLE` handling routes through a match on `AlterTableType`, add the arm; if a subtype is currently unhandled-and-ignored, make this one handled.
- [ ] **Step 3** — Tests (mirror access_method parser tests): `CREATE TABLE t (...) TABLESPACE ts` → `tablespace == Some("ts")`; `CREATE TABLE p PARTITION OF par … TABLESPACE ts`; `ALTER TABLE t SET TABLESPACE ts` → field set; no clause → `None`.
- [ ] **Step 4** — Verify `cargo test -p pgevolve-core --lib parse`; clippy 0; build; fmt. Commit `feat(parse): TABLESPACE on CREATE TABLE + ALTER TABLE SET TABLESPACE`.

---

## Task 5: Catalog reader

**Files:** `crates/pgevolve-core/src/catalog/queries/shared.rs`; `catalog/assemble/tables.rs`.

- [ ] **Step 1** — `queries/shared.rs`: in the table query (the one selecting `access_method` at `:47`), add a `tablespace` column resolving `reltablespace`:
  `LEFT JOIN pg_catalog.pg_tablespace ts ON ts.oid = c.reltablespace` and select `ts.spcname AS tablespace`. (`reltablespace = 0` → the LEFT JOIN yields NULL → `None`.) Add it to BOTH table-query sites if the file has two (`:47` and the `:193`-area join block — check whether both need it; the access_method join appears at `:55` and `:193`).
- [ ] **Step 2** — `assemble/tables.rs`: decode `tablespace` mirroring `access_method` (`:139-146`): `r.get_opt_text(q, "tablespace")?` → `Option<Identifier>` (via `Identifier::from_unquoted`, mapping errors like access_method does), assign into the built `Table` (`:164` area).
- [ ] **Step 3** — Tests (mirror `build_tables_access_method_columnar` at `:745`): a row with `tablespace = Text("fast")` → `Table.tablespace == Some("fast")`; a row with no tablespace (NULL) → `None`.
- [ ] **Step 4** — Verify `cargo test -p pgevolve-core --lib catalog::assemble::tables`; clippy 0; build; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p pgevolve-core` clean; fmt. Commit `feat(catalog): read pg_class.reltablespace into Table::tablespace`.

---

## Task 6: CLI/testkit exhaustive matches + conformance + e2e

**Files:** any `cargo build --workspace` breakages (`crates/pgevolve/src/commands/diff.rs` TableOp describe, `pgevolve-testkit/src/ir_mutator.rs` Table fields); `crates/pgevolve-conformance/tests/cases/objects/**`; `crates/pgevolve/tests/`.

- [ ] **Step 1** — `cargo build --workspace`; add `TableOp::SetTableSpace` arms to any CLI describe / kind-name match (mirror `SetTableComment`), and a `tablespace: None` (or a mutation) to any full `Table` literal in `ir_mutator.rs`. Build + clippy 0.
- [ ] **Step 2** — Conformance fixtures. **First read how the cluster-tablespace conformance/e2e fixtures provision a real tablespace** (`grep -rln "CREATE TABLESPACE\|tablespace" crates/pgevolve-conformance crates/pgevolve/tests`) — a tablespace needs a filesystem `LOCATION`, so reuse whatever directory/setup the existing tablespace tests use. Create under `objects/tables/` (or `partitions/`):
  - `create-with-tablespace` — `CREATE TABLE t (...) TABLESPACE <ts>`.
  - `partition-child-overrides-parent-tablespace` — parent + a child `TABLESPACE <ts>`.
  - `alter-set-tablespace` — leaf table moves → plan shows `ALTER TABLE … SET TABLESPACE`, RequiresApproval.
  - `parent-default-tablespace-change` — partitioned parent default change → Safe.
  - `tablespace-pg_default-is-noop` — source `TABLESPACE pg_default`, live default → empty plan (canon strip).
  If provisioning a real tablespace in conformance is infeasible (no writable LOCATION dir in the harness), mark the apply-requiring fixtures `apply=false` (plan-only) and document why, and rely on the e2e for real-PG coverage — but check the existing tablespace tests first; they solved this.
- [ ] **Step 3** — `bless --conformance`; inspect plans (create shows inline `TABLESPACE`; alter shows `ALTER TABLE … SET TABLESPACE` with the destructive marker on the leaf case, none on the parent case). `cargo test -p pgevolve-conformance`. If the parent case shows RequiresApproval or the leaf shows Safe, STOP — Task 3 destructiveness bug.
- [ ] **Step 4** — E2E (`crates/pgevolve/tests/`, mirror an existing real-PG table test + the cluster-tablespace e2e for tablespace provisioning): `CREATE TABLESPACE` a second tablespace at a temp `LOCATION`, create a table in it, introspect, `assert_convergent`. Docker-guarded. If it diverges (name qualification, pg_default handling), STOP and report. Commit `test: table TABLESPACE conformance + e2e`.

---

## Task 7: docs + full gate

**Files:** `docs/spec/objects.md`, `docs/spec/roadmap.md`, `CHANGELOG.md`, `git rm docs/superpowers/plans/_skeleton/per-partition-tablespace.md`.

- [ ] **Step 1** — `objects.md`: note table `TABLESPACE` placement is supported (regular + partitions; `SET TABLESPACE` move is intent-gated on leaves, metadata-only on parents). `roadmap.md`: move the per-partition `TABLESPACE` row from the Active matrix to the Shipped table with version `Unreleased`, plan link `2026-06-08-table-tablespace.md`. `CHANGELOG.md`: add a fresh `## [Unreleased]` section above `## [0.4.2]` with an `### Added` table-`TABLESPACE` bullet (CREATE clause + ALTER SET TABLESPACE; per-partition override; pg_default normalized; leaf rewrite RequiresApproval vs parent metadata Safe). `git rm` the skeleton.
- [ ] **Step 2** — Full gate: `cargo test --workspace`; `cargo clippy --workspace --all-targets` 0; `cargo fmt --check`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace` clean; `cargo deny check`. Tier-3 `catalog_round_trip` fixtures (`crates/pgevolve-core/tests/fixtures/catalog/pg*/*/expected.json`) gain a `"tablespace": null` field on every table — re-bless via `cargo run -p xtask -- bless` (Docker) and verify the diff is ADDITIVE ONLY (`"tablespace": null` added to table objects, nothing else changed); if non-additive, STOP and report. Commit `feat(table-tablespace): mark shipped`.

---

## Self-review notes
- §1 IR → T1. §2 parser → T4. §3 reader → T5. §4 canon → T1. §5 diff+destructiveness → T3 (+ TableOp T2). §6 render → T2. §7 reference (no constraint) → nothing to build (absence). §8 tests → T6 + unit across tasks. §9 non-goals: index tablespace untouched; cluster CREATE TABLESPACE already shipped.
- **Type consistency:** `Table.tablespace: Option<Identifier>`, `TableOp::SetTableSpace { name: Option<Identifier> }`, `StepKind::SetTableSpace`, `sql::alter_table_set_tablespace` used identically across T1–T6.
- **Watch:** (1) the destructiveness predicate is `partition_by.is_some()` → Safe, else RequiresApproval — a partition CHILD has `partition_of: Some` and `partition_by: None`, so it correctly lands in the RequiresApproval (leaf) branch (T3 tests must cover this). (2) The tier-3 re-bless (T7) adds `"tablespace": null` to EVERY table object across 27 snapshots — this is a larger additive diff than the aggregate/cast `[]` field; confirm additive-only carefully. (3) Reader: there may be TWO table query sites in `shared.rs` (`:47` and `:193`) — add the tablespace join/column to BOTH or the round-trip will be inconsistent.
