# AGGREGATE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the schema-scoped `AGGREGATE` object kind (ordinary aggregates: `sfunc`+`stype`+optional `finalfunc`/`initcond`), with state/final functions constrained to managed SQL/plpgsql functions.

**Architecture:** New `Aggregate` IR on `Catalog`, parsed from `CREATE AGGREGATE` (a `DefineStmt`), read from `pg_aggregate`⋈`pg_proc(prokind='a')`, diffed as a managed schema-scoped object (Create/Drop/Replace), rendered with new `StepKind`s, with a closed-world constraint that `sfunc`/`finalfunc` resolve to managed functions and dep-graph edges to those functions.

**Tech Stack:** Rust, `pg_query`, `pg_catalog` introspection, conformance harness.

**Design:** [`docs/superpowers/specs/2026-06-06-aggregate-design.md`](../specs/2026-06-06-aggregate-design.md)

**Closest templates:** EVENT TRIGGER (object-kind threading: `2026-06-04-event-trigger.md`) and the cluster/per-DB function dependency edges. `CREATE COLLATION` (`parse/builder/` — it's also a `DefineStmt`) is the parser template.

---

## Verified facts
- `CREATE AGGREGATE` is a pg_query `DefineStmt` (same node as `CREATE COLLATION`, `statement.rs:71/122`); distinguish by `DefineStmt.kind == ObjectType::ObjectAggregate`. `DefineStmt.defnames` = the name; `DefineStmt.args` = arg type list; `DefineStmt.definition` = `Vec<DefElem>` (`sfunc`/`stype`/`finalfunc`/`initcond`/…).
- The **function reader already excludes `prokind='a'`** (`catalog/queries/functions.rs:34 AND p.prokind IN ('f','p')`) — no change needed; aggregates won't surface as functions.
- Dep edges to functions: `NodeId::Function(qname, func.arg_types_normalized)` — look up the function in `catalog.functions` and use its `arg_types_normalized` (see `edges.rs:172-176` for the trigger→function edge — mirror it).
- Closed-world reference checks live in `lint/rules/closed_world_references.rs` + `parse/ast_resolution.rs`.

Project rules: no `unwrap`/`expect` in non-test code; `cargo clippy --workspace --all-targets` ZERO warnings; `cargo fmt`; **build/clippy at WORKSPACE level each task**. Co-author trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 1: IR — `Aggregate` + `Catalog::aggregates` + canon

**Files:** Create `crates/pgevolve-core/src/ir/aggregate.rs`; modify `ir/mod.rs`, `ir/catalog.rs`, `ir/canon/mod.rs`; create `ir/canon/aggregates.rs`; add `IrError::DuplicateAggregate`.

- [ ] **Step 1** — `ir/aggregate.rs` with the `Aggregate` struct (fields: `qname: QualifiedName`, `arg_types: Vec<ColumnType>`, `state_type: ColumnType`, `sfunc: QualifiedName`, `finalfunc: Option<QualifiedName>`, `initcond: Option<String>`, `owner: Option<Identifier>`, `comment: Option<String>`), derives `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`. Use `ColumnType` from `crate::ir::column_type`. Add a serde round-trip unit test.
- [ ] **Step 2** — `ir/mod.rs`: `pub mod aggregate;`. `ir/catalog.rs`: add `pub aggregates: Vec<crate::ir::aggregate::Aggregate>,` next to `event_triggers`; update `Catalog::empty()` if it lists fields. Grep `grep -rn "Catalog {" crates/pgevolve-core/src` for non-test literals; fix.
- [ ] **Step 3** — `IrError::DuplicateAggregate(QualifiedName)` (mirror `DuplicateEventTrigger`; identity is qname+arg_types but the message can use the qname — if you want arg-type precision, format the full identity).
- [ ] **Step 4** — `ir/canon/aggregates.rs` `run(cat) -> Result<(), IrError>`: sort `cat.aggregates` by `(qname.schema, qname.name, arg_types)` (derive an Ord on the tuple — `ColumnType` is `Ord`? if not, compare via `format!`/a stable key); reject duplicate `(qname, arg_types)`. Wire into `ir/canon/mod.rs canonicalize()` after `event_triggers::run`. Add canon tests (sort + dup).
- [ ] **Step 5** — Verify: `cargo test -p pgevolve-core --lib ir::aggregate ir::canon::aggregates`; `cargo build --workspace`; `cargo clippy --workspace --all-targets` 0. Commit `feat(ir): Aggregate type + Catalog::aggregates + canon`.

---

## Task 2: Change enum

**Files:** `crates/pgevolve-core/src/diff/change.rs` (+ `diff/mod.rs` re-export).

- [ ] **Step 1** — Add `AggregateChange` (mirror `EventTriggerChange`): `Create(Aggregate)`, `Replace { from: Aggregate, to: Aggregate }`, `Drop { qname: QualifiedName, arg_types: Vec<ColumnType> }`, `AlterOwner { qname, arg_types, owner: Identifier }`, `CommentOn { qname, arg_types, comment: Option<String> }`. Add `Change::Aggregate(AggregateChange)`. Re-export `AggregateChange` if `diff/mod.rs` lists change types.
- [ ] **Step 2** — `cargo build -p pgevolve-core` → exhaustive-match breaks. For ordering/rewrite/CLI matches, add temporary arms marked `// TODO(aggregate Task 4/5/6)` (no `todo!()`); for Display/destructiveness, add real arms mirroring `EventTrigger`/`Trigger`. List the sites touched.
- [ ] **Step 3** — Commit `feat(diff): AggregateChange + Change::Aggregate`.

---

## Task 3: Diff (managed schema-scoped)

**Files:** Create `crates/pgevolve-core/src/diff/aggregates.rs`; wire into `diff/mod.rs diff()`.

- [ ] **Step 1** — `diff_aggregates(target, source, out)` paired by identity `(qname, arg_types)` (use a `BTreeMap` keyed by a clonable identity tuple). **Managed, not lenient:**
  - source-only → `Create` (Safe).
  - target-only → `Drop` (Safe — aggregates carry no data).
  - both: structural diff (`state_type`/`sfunc`/`finalfunc`/`initcond` differ) → `Replace { from, to }` (Safe). Else: `owner` differs **and source `Some`** → `AlterOwner` (lenient owner); `comment` differs → `CommentOn`.
  Tests: create; drop (managed — DOES drop, unlike event triggers); replace on each structural field; lenient owner (source None → nothing); comment.
- [ ] **Step 2** — `diff/mod.rs`: `pub mod aggregates;` + call `aggregates::diff_aggregates(target, source, &mut out)` after `event_triggers`.
- [ ] **Step 3** — Verify `cargo test -p pgevolve-core --lib diff::aggregates`; clippy 0; build (temp emit arm OK). Commit `feat(diff): aggregate differ (managed schema-scoped)`.

---

## Task 4: Dep-graph + ordering

**Files:** `crates/pgevolve-core/src/plan/edges.rs`, `plan/ordering.rs`.

- [ ] **Step 1** — `NodeId::Aggregate(QualifiedName, NormalizedArgTypes)` variant (mirror `NodeId::Function`'s `(QualifiedName, NormalizedArgTypes)` shape — build the aggregate's `NormalizedArgTypes` from its `arg_types` via `NormalizedArgTypes::from_*`; check the constructor that takes a `&[ColumnType]` or build `FunctionArg`s). Handle any exhaustive `match NodeId` (Display/render_node/lint `continue` arms — mirror how `Function` is handled there).
- [ ] **Step 2** — In the graph builder, register each aggregate node and add edges to its `sfunc` and `finalfunc` functions: find the managed function in `catalog.functions` whose `qname == agg.sfunc` and whose `arg_types_normalized` matches the **implied sfunc signature** `(state_type, arg_types…)`; add `add_edge(NodeId::Aggregate(...), NodeId::Function(sfunc_qname, that_func.arg_types_normalized.clone()))`. Same for `finalfunc` with implied signature `(state_type)`. (If the function isn't found — shouldn't happen post-closed-world-check — skip the edge, like the trigger edge does when the function is unmanaged.) Mirror `edges.rs:172-176`.
- [ ] **Step 3** — `plan/ordering.rs change_node`: map each `AggregateChange` to `NodeId::Aggregate(...)` (Create→to.qname/arg_types; Replace→to; Drop/AlterOwner/CommentOn→the carried qname+arg_types). Clear the Task-2 ordering TODO. Add the `partition()` arm (Create→creates, Drop/Replace→drops, AlterOwner/CommentOn→modifies — mirror EventTrigger).
- [ ] **Step 4** — Tests (edges.rs): aggregate node registered; edge to a managed sfunc function exists. Verify build/clippy. Commit `feat(plan): NodeId::Aggregate + edges to sfunc/finalfunc`.

---

## Task 5: Render + StepKind

**Files:** Create `crates/pgevolve-core/src/plan/rewrite/emit/aggregate.rs`; modify `plan/rewrite/emit/mod.rs`, `plan/rewrite/mod.rs`, `plan/raw_step.rs`, `plan/plan.rs`.

- [ ] **Step 1** — `StepKind`: `CreateAggregate`, `DropAggregate`, `AlterAggregateOwner`, `CommentOnAggregate` (+ serde round-trip test update + `kind_name`/`parse_kind_name` in `plan.rs`). (No `ReplaceAggregate` kind — Replace emits a Drop step + a Create step.)
- [ ] **Step 2** — `emit/aggregate.rs` (mirror `emit/event_trigger.rs`): SQL builders rendering an arg-type list `(t1, t2)` and the `(SFUNC = …, STYPE = …[, FINALFUNC = …][, INITCOND = '…'])` definition:
  - `CREATE AGGREGATE <qname> (<argtypes>) (SFUNC = <sfunc>, STYPE = <state_type>[, FINALFUNC = <finalfunc>][, INITCOND = '<initcond escaped>']);`
  - `DROP AGGREGATE <qname> (<argtypes>);`
  - `ALTER AGGREGATE <qname> (<argtypes>) OWNER TO <owner>;`
  - `COMMENT ON AGGREGATE <qname> (<argtypes>) IS '…' | IS NULL;`
  Render arg types / state type via the existing `ColumnType` SQL renderer (find how columns render their type — reuse it). `Create` with `owner`/`comment` → follow-up `ALTER OWNER`/`COMMENT` steps (CREATE AGGREGATE has no inline OWNER/COMMENT). `Replace` → `Drop` step then the `Create` sequence. All `InTransaction`, Safe (Drop too — no data). Map the emit signature exactly to the trigger/event-trigger emitter.
- [ ] **Step 3** — Register `pub mod aggregate;` in `emit/mod.rs`; dispatch `Change::Aggregate(ac) => emit::aggregate::emit(ac, …)` in `rewrite/mod.rs` (clear the Task-2 emit TODO).
- [ ] **Step 4** — Unit-test each rendered SQL string (incl. with/without finalfunc/initcond, owner/comment follow-ups, replace=drop-then-create). Verify `grep -rn "TODO(aggregate" crates` → empty; clippy 0; build clean. Commit `feat(render): emit aggregate DDL`.

---

## Task 6: Parser

**Files:** Create `crates/pgevolve-core/src/parse/builder/aggregate_stmt.rs`; modify `parse/statement.rs`, `parse/builder/mod.rs`, `parse/mod.rs`.

- [ ] **Step 1** — `statement.rs`: the `DefineStmt` arm (line ~122) currently routes `CREATE COLLATION`. Branch on `DefineStmt.kind`: `ObjectType::ObjectAggregate` → a new `Statement::CreateAggregate(DefineStmt)` variant; keep collation for `ObjectCollation`. Also route `AlterOwnerStmt`/`CommentStmt`/`RenameStmt`/`DropStmt` with `ObjectType::ObjectAggregate`. Read how the collation `DefineStmt` builder is invoked and mirror.
- [ ] **Step 2** — `aggregate_stmt.rs`: parse the `DefineStmt` into an `Aggregate`:
  - name from `defnames` (qualified-name list → `QualifiedName`); `arg_types` from `args` (a list of `FunctionParameter`/type nodes → `Vec<ColumnType>` — reuse the function-arg type parser; ordinary aggregates' args are plain types).
  - `definition` `Vec<DefElem>`: read `sfunc`→`QualifiedName`, `stype`→`ColumnType`, `finalfunc`→`Option<QualifiedName>`, `initcond`→`Option<String>`. **Reject** any DefElem outside this set (`combinefunc`/`serialfunc`/`deserialfunc`/`msfunc`/`sortop`/`hypothetical`/…) and any `ORDER BY` (ordered-set) with a structured `ParseError` ("unsupported aggregate feature `<x>` — v0.4.1 supports ordinary aggregates only").
  - push; reject duplicate identity in source.
  - `ALTER AGGREGATE … OWNER TO` / `COMMENT ON AGGREGATE` apply by identity; `DROP`/`RENAME` in source → `ParseError` (mirror event-trigger/tablespace rejections).
- [ ] **Step 3** — Wire builder module + dispatch + (aggregates flow into `catalog.aggregates` via the parse accumulator — mirror how collations/event-triggers flow). Tests: simple create (sfunc+stype); with finalfunc; with initcond; ALTER OWNER; COMMENT; reject ordered-set/combinefunc; reject DROP/RENAME in source; duplicate identity.
- [ ] **Step 4** — Verify `cargo test -p pgevolve-core --lib parse`; clippy 0; build. Commit `feat(parse): CREATE/ALTER/COMMENT AGGREGATE`.

---

## Task 7: Catalog reader

**Files:** Create `crates/pgevolve-core/src/catalog/assemble/aggregates.rs` + a query; modify `catalog/mod.rs` (CatalogQuery), `catalog/assemble/mod.rs` (RawRows + call), `catalog/drift.rs` (or wherever `DriftReport` is — add `unmanaged_aggregates`).

- [ ] **Step 1** — `AGGREGATES_QUERY` over `pg_aggregate a JOIN pg_proc p ON p.oid = a.aggfnoid` (the wrapper proc, `p.prokind='a'`), schema-filtered by `$1` like the functions query: select `p.proname`, `p.pronamespace`→schema, `p.proargtypes`→arg type oids, `pg_get_userbyid(p.proowner)`→owner, comment; `a.aggtransfn`→sfunc oid, `a.aggtranstype`→state type oid, `a.aggfinalfn`→finalfunc oid (0=none), `a.agginitval`→initcond, `a.aggkind`→kind ('n'=normal). Join `pg_proc` again for sfunc/finalfunc names+langs and `pg_type` for type names (or resolve OIDs in the assembler via helper functions the type/function readers already use — check how `assemble/functions.rs` resolves arg type OIDs to `ColumnType` and reuse it).
- [ ] **Step 2** — Register `CatalogQuery::Aggregates` (param group like functions — takes `$1` schema array); RawRows field; assemble call. Build errors guide the registration sites (mirror the EVENT TRIGGER reader task's registration).
- [ ] **Step 3** — `assemble_aggregates`: decode each row; **skip** (push to `DriftReport.unmanaged_aggregates`) when `aggkind <> 'n'` (ordered-set/hypothetical) OR the sfunc/finalfunc language is unreadable (not 'sql'/'plpgsql') OR the sfunc/finalfunc isn't otherwise representable. Otherwise build the `Aggregate`. Resolve type OIDs → `ColumnType` and func OIDs → `QualifiedName` using the existing helpers. Add the `unmanaged_aggregates: Vec<...>` field to `DriftReport` (mirror `unmanaged_language_routines`).
- [ ] **Step 4** — Tests (mirror `assemble/functions.rs` tests with `Row::new().with(...)`): decode simple; with finalfunc/initcond; skip ordered-set; skip unreadable-sfunc-language. Verify; clippy 0; build. Commit `feat(catalog): read pg_aggregate (skips ordered-set + unmanaged-fn aggregates)`.

---

## Task 8: Closed-world constraint (source rejects unmanaged sfunc)

**Files:** `crates/pgevolve-core/src/lint/rules/closed_world_references.rs` (or a new IR-build check); wherever source closed-world checks run.

- [ ] **Step 1** — Read `closed_world_references.rs` + how it's invoked (it validates that source references resolve to managed objects, erroring otherwise). Add aggregate handling: for each source aggregate, its `sfunc` (and `finalfunc` if present) must match a managed function in `catalog.functions` by `qname` + the implied signature (sfunc: `(state_type, arg_types…)`; finalfunc: `(state_type)`). If not → a structured error/finding `AggregateUnmanagedStateFunction { aggregate, function }` (add the variant; mirror an existing closed-world error variant). Whether this is a hard `IrError` at canon/build time or a lint-stage error: match how closed-world function references for triggers/views are enforced (they error the plan). Use the same mechanism.
- [ ] **Step 2** — Tests: source aggregate over a declared managed plpgsql function → OK; over an undeclared/unmanaged function → the error. Verify; clippy 0; build clean. Commit `feat(lint): aggregate state/final functions must be managed`.

---

## Task 9: CLI exhaustive matches

**Files:** `crates/pgevolve/src/commands/diff.rs` (Change describe ×2), `crates/pgevolve/src/commands/graph.rs` (NodeId label), `crates/pgevolve-conformance/src/assertions/dep_graph.rs` (NodeId label), and any `cargo build --workspace` flags.

- [ ] **Step 1** — Build the whole workspace; add the `Change::Aggregate(_)` and `NodeId::Aggregate(_)` arms in the CLI + conformance matches, mirroring how `EventTrigger`/`Function` are handled there (human-readable describe strings; graph node label `aggregate:<schema.name>`). Verify `cargo build --workspace` + clippy 0. Commit `feat(cli): handle Aggregate Change/NodeId variants`.

---

## Task 10: Conformance + e2e

**Files:** `crates/pgevolve-conformance/tests/cases/objects/aggregates/**`; `crates/pgevolve/tests/aggregate_e2e.rs`.

- [ ] **Step 1** — Read a real `objects/` fixture for format. Create under `objects/aggregates/` (each declares its managed plpgsql sfunc/finalfunc in SQL):
  - `create-simple` — a plpgsql `sfunc(state, val)` + `CREATE AGGREGATE agg(<type>) (SFUNC=sfunc, STYPE=<type>)`.
  - `create-with-finalfunc`, `create-with-initcond`, `drop`, `comment-on`.
  - `failure/reject-unmanaged-state-fn` — `CREATE AGGREGATE` over a built-in sfunc (e.g. `int4pl`) → source rejected (encode the expected error, mirror an existing `failure/` fixture).
- [ ] **Step 2** — `cargo run -p xtask -- bless --conformance`; inspect `expected/plan.sql` (create cases show the CREATE AGGREGATE + dep-graph edge to the sfunc function). `cargo test -p pgevolve-conformance` (Docker) → pass. If a generated plan is wrong (e.g. aggregate created before its sfunc), STOP — Task 4 dep bug.
- [ ] **Step 3** — e2e (`crates/pgevolve/tests/aggregate_e2e.rs`, mirror `event_trigger_e2e.rs`): parse SQL declaring a plpgsql sfunc + an aggregate over it, apply to ephemeral PG, introspect, `assert_convergent`. `#[ignore]`/docker-guarded. Run with `--ignored`; MUST converge. If it diverges (sfunc/argtype/initcond), STOP and report.
- [ ] **Step 4** — Commit `test: AGGREGATE conformance + e2e`.

---

## Task 11: docs + full gate

**Files:** `docs/spec/objects.md`, `docs/spec/roadmap.md`, `CHANGELOG.md`, `git rm docs/superpowers/plans/_skeleton/aggregate.md`.

- [ ] **Step 1** — `objects.md`: flip `AGGREGATE` to ✅ Supported (note ordinary-only + managed-fn constraint). `roadmap.md`: move to Shipped (v0.4.1), plan link `2026-06-06-aggregate.md`. `CHANGELOG.md` `[Unreleased]→Added`: AGGREGATE bullet (ordinary aggregates, managed SQL/plpgsql state functions, drop+create rename, ordered-set/moving out). `git rm` the skeleton. (Note: a fresh `[Unreleased]` section may need re-adding above the released sections since v0.4.0 consumed it — add `## [Unreleased]` with the Added bullet.)
- [ ] **Step 2** — Full gate: `cargo test --workspace`; `cargo clippy --workspace --all-targets` 0; `cargo fmt --check`; `cargo deny check`. Tier-3 `catalog_round_trip` snapshots re-bless on the new `Catalog` field — verify additive-only (`"aggregates": []`). Commit `feat(aggregate): mark shipped`.

---

## Self-review notes
- §1 IR → T1. §2 constraint → T8 (source) + T7 (reader skip). §3 parser → T6. §4 reader → T7. §5 canon → T1. §6 diff → T3. §7 render+depgraph → T4/T5. §8 tests → T10 + unit across tasks. §9 non-goals: ordered-set/moving rejected (T6 parser, T7 reader), no rename (T6), managed-fn-only (T8).
- **Type consistency:** `Aggregate { qname, arg_types: Vec<ColumnType>, state_type: ColumnType, sfunc, finalfunc, initcond, owner, comment }` and `AggregateChange`/`StepKind` names used identically across T2-T7. Identity = `(qname, arg_types)` everywhere.
- **Watch:** the dep-edge implied-signature lookup (T4 Step 2) — the sfunc's managed-function `arg_types_normalized` must equal `NormalizedArgTypes::from((state_type, arg_types…))`. If the e2e (T10) shows the aggregate ordered before its sfunc, that lookup is mismatched.
