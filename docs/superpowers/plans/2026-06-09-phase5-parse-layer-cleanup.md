# Phase 5 — Parse-layer cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the common-case body-normalization gap that causes spurious view diffs (decision 5), and dedup the parse-layer (consolidate the `DefElem`/qname extractors into `builder::shared`; replace the 18-argument `process_file` with a `ParseContext`) — decisions 7a/9. The fifth and final pre-1.0 cleanup phase.

**Architecture:** `parse/normalize_body.rs` canonicalizes view/function bodies (parse → strip redundant qualifiers → `pg_query::deparse` → collapse whitespace → BLAKE3 hash); two bodies are equivalent iff their canonical text is byte-equal. The qualifier-stripper currently only handles a top-level single-relation `FROM`, so multi-table views and subqueries can produce spurious cross-PG-version diffs. The per-object builders also re-implement `DefElem → String` ~6× and open-code List-of-String→qname instead of using `builder::shared::qname_from_string_list`, and `process_file` threads 18 `&mut` params.

**Tech Stack:** Rust, `pg_query` (v6 protobuf AST). STRICT lints (clippy pedantic+nursery, `-D warnings`); no `unwrap`/`expect` in production. Per-commit gate INCLUDES `cargo fmt --check` AND `RUSTDOCFLAGS="-D warnings" cargo doc -p pgevolve-core --no-deps` (Phase 3 lesson). Commits go directly to `main`. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Prerequisite:** Phase 4 CI green across all 5 PG majors.

**Out of scope (deliberately, per CLAUDE.md directive 6 — not a chosen decision):** unifying the two SQL dep-walkers (`ast_canon.rs::walk_node` + `plpgsql.rs::walk_sql_node_for_deps`). The parse review flagged it as the clearest missing helper, but it wasn't a ratified decision — note it as a future cleanup, don't fold it in.

---

## Pre-flight
- [ ] **P1:** Confirm Phase 4 CI green across all 5 PG majors.
- [ ] **P2:** Green baseline: `cargo fmt --check && cargo test -p pgevolve-core && cargo clippy -p pgevolve-core --all-targets`.

---

### Task 1: Recurse qualifier-stripping into subqueries / CTEs / nested scopes (decision 5)

**Why:** `strip_redundant_qualifiers` (`parse/normalize_body.rs`) only strips the top-level single-relation `FROM`'s qualifier (+ set-op children). Subqueries (`RangeSubselect`), CTEs (`WITH`), join-nested subqueries, and `SubLink` subselects in expressions are NOT recursed, so a multi-table/subquery view can canonicalize differently across PG 14↔17 → a spurious diff. Extend the recursion so EVERY nested `SelectStmt` is stripped **in its own scope**.

**Safety invariant (critical — this is what keeps it correct):** each `SelectStmt` scope computes its OWN `unique_from_qualifier` from its OWN `from_clause`, and `strip_qualifier_in_node` only rewrites `[that_local_name, col] → [col]`. A correlated subquery's reference to an OUTER table uses a *different* qualifier (the outer relation's name), which does NOT match the inner scope's `unique_name`, so it is left untouched. Therefore recursing per-scope can only ever strip a qualifier that is genuinely redundant *within that scope* — it cannot collapse two semantically-different bodies (no false-negative). Do NOT strip based on any name not drawn from the local scope's single-relation FROM.

**Files:** `crates/pgevolve-core/src/parse/normalize_body.rs` (`strip_qualifiers_in_select` + a new recursion helper); tests in the same file.

- [ ] **Step 1: Read the current `strip_qualifiers_in_select`** (it handles `larg`/`rarg` set-op recursion + the local scope). Extend it to ALSO recurse into nested `SelectStmt`s, each via a recursive `strip_qualifiers_in_select` call (so each gets its own scope):
  - **`from_clause`**: walk each `Node` — a `RangeSubselect` carries `subquery: Option<Box<Node>>` whose node is a `SelectStmt` → recurse into it; a `JoinExpr` carries `larg`/`rarg` `Node`s that may be `RangeSubselect`/`JoinExpr` → recurse to find nested subqueries. (Plain `RangeVar`s contribute to `collect_from_qualifiers` as today.)
  - **`with_clause`**: `sel.with_clause` (Option<WithClause>) has `ctes: Vec<Node>`; each is a `CommonTableExpr` with `ctequery: Option<Box<Node>>` → a `SelectStmt`; recurse into each.
  - **`SubLink` subselects in expressions**: in `target_list`, `where_clause`, `having_clause` (and `from_clause` lateral), a `SubLink` node has `subselect: Option<Box<Node>>` → `SelectStmt`; recurse. Write one small `recurse_into_subselects_in_node(node)` helper that finds `SubLink.subselect` (and descends through boolean/expr nodes) and calls `strip_qualifiers_in_select` on the contained select. Reuse/extend the existing node-walk machinery (`strip_qualifier_in_node` already descends expression nodes — model the recursion-finder on it).

  Keep the ORDER: recurse into children first (so inner scopes are normalized), then strip the local scope (as the set-op recursion already does). Update the `Limitations` doc comment to reflect the new coverage.

- [ ] **Step 2: Tests — prove the fix AND the safety invariant.** Add to `normalize_body.rs` tests (pure, no DB — `NormalizedBody::from_sql`):
  - **Subquery stripped in its own scope:** `SELECT s.id FROM (SELECT t.id FROM app.t) s` — confirm the INNER `t.id` strips to `id` (inner FROM is single-relation `app.t`); the outer `s.id` keeps/strips per the outer single-relation rule. Assert the canonical text of this equals the canonical text of the already-unqualified form.
  - **CTE stripped:** `WITH c AS (SELECT u.x FROM app.u) SELECT * FROM c` — inner `u.x` → `x`.
  - **CORRELATED subquery NOT mis-stripped (the false-negative guard):** `SELECT a FROM app.t WHERE EXISTS (SELECT 1 FROM app.u WHERE u.fk = t.pk)` — the inner scope's single FROM is `app.u`, so `u.fk` → `fk`, but `t.pk` (outer ref, qualifier `t` ≠ inner `u`) MUST be preserved. Assert the canonical text still contains a `t`-qualified reference (i.e. the outer qualifier survives).
  - **Two genuinely-different bodies still differ:** e.g. `SELECT a FROM app.t` vs `SELECT b FROM app.t` → different canonical hashes (sanity that normalization didn't over-collapse).
  - **Multi-table top-level FROM unchanged:** a join view `SELECT t.a, u.b FROM app.t JOIN app.u ON …` keeps its qualifiers at the outer level (the outer FROM has 2 relations → no unique name → no strip), exactly as before.

- [ ] **Step 3: Verify + commit.** `cargo fmt --check && cargo clippy -p pgevolve-core --all-targets && cargo test -p pgevolve-core normalize_body && RUSTDOCFLAGS="-D warnings" cargo doc -p pgevolve-core --no-deps`. Then run the FULL `cargo test -p pgevolve-core` — body normalization feeds view-diff equality, so watch for any view round-trip test that shifts (it should only IMPROVE — fewer spurious diffs; if a test now reports two bodies as EQUAL that should differ, that's a false-negative — stop and fix). If conformance view fixtures change, that's a real (good) reduction in spurious recreations — re-bless and inspect to confirm only spurious view-recreations disappeared. Commit:
```
feat(parse): recurse qualifier-stripping into subqueries/CTEs (fewer spurious view diffs)
```
> Record in `v1.md` §8: the multi-table/subquery spurious-diff limitation is now resolved for the common cases; commutative-operand reordering + deep paren-folding remain the documented edge (already noted).

---

### Task 2: Consolidate the `DefElem → String` extractors into `builder::shared` (decision 7a)

**Why:** `DefElem → Option<String>` is reimplemented ~6×: `subscription_stmt.rs:325` (`extract_string_value`), `publication_stmt.rs:429` (`extract_string_value`), `reloptions.rs:143` (`extract_value`), `create_function_stmt.rs:414` + `create_extension_stmt.rs:97` (`string_from_def_elem`), `aggregate_stmt.rs:268` (`string_from_defelem`). One helper.

**Files:** `crates/pgevolve-core/src/parse/builder/shared.rs` (add the helper); the 6 builder files (replace + delete locals).

- [ ] **Step 1: Read all 6** and confirm they're equivalent (extract the string value of a `DefElem`'s arg — typically a `String`/`Integer`/`Float` node, or a `TypeName`/qualified value). They may differ slightly (some accept only `String`, some also numbers/keywords). Design ONE `shared::def_elem_string(de: &DefElem) -> Option<String>` that covers the UNION of what the call sites need (the most permissive correct form), OR — if two genuinely-different behaviors exist (e.g. one needs the raw keyword, another the numeric text) — add two clearly-named helpers rather than forcing one. Do NOT change any call site's accepted inputs/outputs.
- [ ] **Step 2: Replace each of the 6** with the shared helper; delete the locals. Confirm each call site's behavior is byte-identical (the parsed IR for each object is unchanged).
- [ ] **Step 3: Verify + commit.** Per-task gate (fmt/clippy/test/doc). The per-object parse round-trip tests prove behavior unchanged. Commit:
```
refactor(parse): one shared DefElem->String extractor (dedup ~6 copies)
```

---

### Task 3: Route open-coded List-of-String→qname through `shared::qname_from_string_list` (decision 7a)

**Why:** `builder::shared::qname_from_string_list` (shared.rs:53) is the canonical "List node of String parts → `QualifiedName`" parser, but several builders open-code the `NodeEnum::List` walk instead.

**Files:** the builders that open-code it (find them: `grep -rn 'NodeEnum::List' crates/pgevolve-core/src/parse/builder/` and the `apply_statistics_comment` inline walk in `parse/mod.rs:~216`).

- [ ] **Step 1: Find every open-coded List-of-String→qname site.** For each, confirm it's doing the same thing `qname_from_string_list` does (collect the String segments into a 1- or 2-part qualified name, with the same error/location handling). Sites the parse review named: `cast_stmt`, `comment_stmt`, `owner_stmt`, `default_privileges`, `event_trigger_stmt`, `aggregate_stmt`, `text_search_stmt`, plus `mod.rs::apply_statistics_comment`. Verify by reading; only convert the ones that are genuinely the same operation.
- [ ] **Step 2: Replace each** with a `shared::qname_from_string_list(...)` call; delete the local walk. Preserve the exact `SourceLocation`/error behavior (qname_from_string_list takes a location — pass the right one).
- [ ] **Step 3: Verify + commit.** Per-task gate. Parse round-trip tests prove unchanged. Commit:
```
refactor(parse): route List->qname parsing through shared::qname_from_string_list
```

---

### Task 4: Replace the 18-argument `process_file` with a `ParseContext` (decision 9)

**Why:** `process_file` (`parse/mod.rs:394`) takes ~18 `&mut` params (six pending-vecs, seven accumulators, catalog, locations) under `#[allow(clippy::too_many_arguments)]`. Bundle them into one struct to tame the signature.

**Files:** `crates/pgevolve-core/src/parse/mod.rs`.

- [ ] **Step 1: Read `process_file` and its caller** (the per-file loop in `parse_directory_with_locations`). Catalogue the 18 params into groups (pending-fragment vecs, accumulators, catalog, locations, file path).
- [ ] **Step 2: Define a `ParseContext` struct** holding the mutable state that's threaded through `process_file` and the multi-pass finalization (the pending vecs + accumulators + catalog + locations). Give it a clear doc comment. Construct it once in `parse_directory_with_locations`, mutate it across the per-file loop and the finalization passes, then consume it.
- [ ] **Step 3: Change `process_file(&mut self_or_ctx, stmt, …)`** to take `&mut ParseContext` plus the per-statement inputs (the `Statement`/node + its location). Remove the `#[allow(clippy::too_many_arguments)]` (the goal is for it to no longer be needed; if a residual clippy arg-count remains, reduce further rather than re-adding the allow). This is a MECHANICAL refactor — no behavior change; `cargo build` guides the field accesses.
- [ ] **Step 4: Verify + commit.** Per-task gate INCLUDING the full `cargo test -p pgevolve-core` (parse is load-bearing). Behavior unchanged — the parse round-trip + parse_directory tests prove it. Commit:
```
refactor(parse): bundle process_file state into ParseContext (was 18 args)
```

---

### Task 5: Phase wrap — full workspace gate + push
- [ ] **Step 1:** `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps && cargo deny check`. (If a local `cargo test --workspace` flakes on Docker ephemeral-PG contention, re-run the failed test in isolation to confirm contention — CI's per-major jobs avoid it.)
- [ ] **Step 2:** Confirm dedup: `grep -rn 'fn extract_string_value\|fn extract_value\|fn string_from_def' crates/pgevolve-core/src/parse/builder/` → empty (or only the shared one). `grep -n 'too_many_arguments' crates/pgevolve-core/src/parse/mod.rs` → gone for `process_file`.
- [ ] **Step 3:** `git push origin main`; wait for CI green across all 5 PG majors. **This completes the pre-1.0 cleanup (all 5 phases).** Update `project_pre1_cleanup_phases` memory to all-done.

---

## Self-review notes
- **Spec coverage:** Task 1 = decision 5 (qualifier recursion); Tasks 2–3 = decision 7a (parse extractor/qname dedup); Task 4 = decision 9 (ParseContext). The two-walker unification is explicitly OUT (not a decision).
- **The load-bearing risk is Task 1** — body normalization changes view-diff equality. The direction must stay "fewer spurious diffs" (false positives), NEVER "two different bodies now compare equal" (false negative). The per-scope safety invariant + the correlated-subquery test guard against that. If any test starts treating distinct bodies as equal, STOP.
- Tasks 2–4 are behavior-preserving dedups/refactors; the parse round-trip + `parse_directory` + conformance tests are the safety net.
- Per-task gate INCLUDES `cargo fmt --check` AND `cargo doc -D warnings` (Phase 3 lessons).
