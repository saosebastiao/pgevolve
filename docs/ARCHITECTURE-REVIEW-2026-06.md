# pgevolve Architecture Review — June 2026 (pre-1.0)

**Status:** Advisory. This document is a snapshot architectural assessment taken
while the feature surface is "substantially complete," ahead of the 1.0 cut
defined in [`v1.md`](./v1.md). Its job is to surface the architectural
compromises accumulated during feature work *before* 1.0 freezes the public
surface and makes backwards-incompatible refactors expensive.

**Method.** Six parallel deep-dives, one per subsystem (macros, plan/ALTER,
diff, ir, parse, and the supporting catalog/render/lint/CLI tier), each
producing file:line evidence. Key claims were re-verified directly against the
source. Findings are graded against the [Constitution](./CONSTITUTION.md):
*simplest architecture that solves the **full** problem; readability over
cleverness; make illegal states unrepresentable; dependencies are a liability.*

**Reviewer's headline.** The architecture is fundamentally sound and the bones
are good — a clean, I/O-light pipeline with a disciplined domain model. The
compromises are real but *local*: duplication that wants a helper, a handful of
illegal-state escape hatches, a confusing dual use of the word "diff," and one
proc-macro that no longer earns its keep. None of them are load-bearing
mistakes. **The single most important finding is that the owner's instinct
about ALTER-vs-drop/create is half right — and acting on the wrong half would
do real damage. That section is first.**

---

## 1. The architecture in one picture

```
            ┌──────────────┐
 *.sql ───► │  parse/      │ ──┐                         desired-state IR
            │  (pg_query→IR)│   │                              │
            └──────────────┘   ├──► ir::Catalog ──► diff() ──► ChangeSet ──► plan() ──► Steps ──► render SQL
            ┌──────────────┐   │        ▲             (unordered     (ordering +    (emit/*.rs)
 live DB ─► │  catalog/    │ ──┘        │              change set)    dependency
            │  (pg_cat→IR) │       same canonicalize()                 graph)
            └──────────────┘       (ir::canon)                            │
                                                                     lint/ gates
                                                                          │
                                                       pgevolve crate: executor + shadow-validate (I/O)
```

The pipeline is the right shape and the layering is mostly honest:

- **Two front-ends, one IR.** `parse/` lowers desired-state SQL; `catalog/`
  reads live DB state. Both produce the *same* `ir::Catalog` and both pass
  through the *same* `ir::canon::canonicalize`. Diff correctness depends
  entirely on these two paths agreeing, and that symmetry is the single
  most-tested invariant in the codebase (Tier-3 goldens + Tier-C apply
  round-trip across PG 14–18 in CI). This is the project's strongest
  architectural decision.
- **`pgevolve-core` is network-I/O-free by construction** — live-DB access is
  injected via the `CatalogQuerier` trait. The CLI executor only ever runs
  *pre-rendered* SQL; it never builds migration SQL. The core/CLI boundary is
  clean.
- **LOC distribution:** plan 25k, parse 21k, diff 14k, ir 10.7k, catalog 10.4k,
  lint 9.5k, render 1.6k. Two important caveats the raw numbers hide: **lint is
  ~4k production LOC** (the rest is tests), and **`render/` is not the migration
  emitter** — migration SQL is generated in `plan/rewrite/.../emit/`. `render/`
  is the dump-only path.

**Verdict on the bones: keep them.** Nothing below argues for re-architecting
the pipeline. The recommendations are surgical.

---

## 2. The central thesis: "could drop/create replace most ALTER logic?"

> *Owner's hypothesis: the prime directive — declarative change without data
> loss — is fundamentally simple, and we may have too many ALTER capabilities.
> Could DROP+CREATE (with data preservation) replace most ALTER "just as easily,
> without losing data, with minimal downtime"?*

**Verdict: the instinct is correct that the core problem is simple, but the
proposed simplification is the wrong lever — and the codebase has already proven
it.** Acting on it would invert the project's central safety strategy.

### 2.1 The empirical fact that settles it

There is **zero table-data copy/swap logic anywhere in the 25k-LOC plan tree.**
Verified directly: `grep 'INSERT INTO'` across `plan/` returns **0** hits; there
is no rename-old / create-new / copy-data / drop-old path for any data-bearing
table. The team has *already chosen* in-place ALTER plus online-rewrite patterns
over drop/create-with-copy — and that was the right call. The places where
pgevolve *does* use drop/create are exactly its **most complex and most
dangerous** code (see 2.3), not its simplest.

### 2.2 Why "too many ALTER capabilities" misreads the breadth

The ALTER surface is large, but most of it is **catalog coverage, not
ALTER-vs-recreate complexity.** Drop/create cannot serve it either way:

- ~70% of the ALTER LOC is *metadata-only* operations on non-table objects:
  29 ALTER PUBLICATION paths, 17 ALTER SUBSCRIPTION paths, 25 text-search
  config paths, sequence option setters, policies, grants, owners, collations,
  roles. **Drop/create on these is strictly worse, not simpler** — dropping a
  sequence loses `last_value`; dropping a subscription loses replication-slot
  state; dropping/recreating a publication is pure churn. These exist because
  *Postgres objects have many independently-settable attributes* — that's the
  "Full Postgres support" goal, and the ALTER-vs-drop/create question doesn't
  touch it.
- For **data-bearing tables**, drop/create-with-copy is *categorically more
  dangerous*: inbound FKs from other tables pin the old table (can't drop
  without `CASCADE`, which silently drops *their* constraints); the copy doubles
  disk and turns an O(1) metadata change into an O(rows) outage; identity/
  sequence ownership, partition attachments, and OID-based extension
  dependencies are all lost on recreate. "Minimal downtime" is false for any
  non-trivial table.

The clearest proof the project already knows this: `SET NOT NULL` is *not* a
naive operation — it's decomposed into `ADD CHECK NOT VALID → VALIDATE → SET NOT
NULL → DROP CHECK` (`plan/rewrite/set_not_null_check_pattern.rs`) specifically to
**avoid** the full-table scan/rewrite a naive approach would force. The whole
online-rewrite family (FK/CHECK `NOT VALID`+`VALIDATE`, `CREATE INDEX
CONCURRENTLY`, `REFRESH MV CONCURRENTLY`) is the *opposite* of drop/create: it
exists to dodge the rewrite that drop/create would make mandatory.

### 2.3 Where drop/create IS used — and why it's the scary code

- `UserTypeChange::ReplaceWithCascade` (`plan/rewrite/emit/user_type.rs:66`)
  emits `DROP TYPE … CASCADE` + `CREATE TYPE`. This is the **only** `CASCADE` in
  the codebase, used solely because enum/composite type changes have no in-place
  ALTER. Both halves are flagged destructive.
- Incompatible view/MV replacement drives `plan/recreate_views.rs` — at **1,204
  LOC the single most complex file in the audit** — precisely because it
  *refuses* `DROP … CASCADE` (for auditability) and instead hand-walks the
  dependency graph to emit explicit ordered DROP+CREATE per dependent.

**This is what "replace ALTER with drop/create" actually costs in practice: it
*is* the biggest, most intricate machinery in the codebase.** Generalizing it
to tables would grow `ordering.rs`/`edges.rs` (the two largest files, 2.6k each,
already pure dependency-graph machinery), not shrink them.

### 2.4 The simpler core that *does* exist

There is a defensible simplification — but it's the inverse of the thesis. Make
the existing implicit tiers explicit and *contain* drop/create rather than
expand it:

| Tier | What | Strategy | Pre-1.0 stance |
|------|------|----------|----------------|
| **1. Cheap metadata ALTERs** | defaults, comments, storage/compression, drop-constraint, tablespace, all sequence/publication/subscription/policy/grant/owner/TS/role setters, `ALTER TYPE ADD VALUE`, `ALTER DOMAIN` setters | direct ALTER | **Keep.** ~70% of LOC. Drop/create is pure downside here. |
| **2. Data-affecting table ALTERs** | `AlterColumnType`, `SetColumnNullable`, `AddConstraint(FK/CHECK)`, add/drop column | ALTER + online-rewrite patterns | **Keep & strengthen.** This is the prime-directive core. Drop/create would be catastrophic. |
| **3. No-ALTER-path objects** | views/MVs, enum/composite/range types, collations | drop/create (the *narrow, audited* fallback) | **Contain.** Unify behind one "recreate-with-dependents" engine; today only views have the full walker, types use raw `CASCADE`. Hard rule: **heap-data tables are never recreated.** |

**Recommendation:** Adopt Tier 3 unification as a *real* pre-1.0 simplification
(it removes genuine duplicated complexity), and write the "tables are never
recreated" rule into the Constitution so the prime directive is structurally
enforced. Do **not** pursue drop/create-for-tables.

### 2.5 The one place the current design *does* lose data avoidably

Column **rename** is invisible to the differ — columns are paired strictly by
name (`diff/columns.rs:28`), so a rename diffs as DROP-old + ADD-new and trips
the data-loss warning. Same for reorder (explicitly punted). These are the only
logically-safe operations that currently risk data, and drop/create makes them
*worse*, not better. If rename support is wanted, it needs an explicit rename
directive in the source-of-truth (a small, bounded feature) — not a strategy
change. Decide before 1.0 whether this is in-scope or a documented limitation.

---

## 3. Cross-cutting findings

### 3.1 The `#[derive(Diff)]` proc-macro — REMOVE before 1.0

`pgevolve-core-macros` is a 151-line proc-macro (3 deps: `syn` full, `quote`,
`proc-macro2`) **published to crates.io as a second crate** solely so
`pgevolve-core` resolves. It generates a `Diff` impl that is a flat list of
per-field comparison calls. Assessment:

- **It covers only the trivial half.** It applies to ~16 flat structs. Every
  *interesting* diff (enum dispatch, keyed-collection pairing in
  `Table`/`View`/`Catalog`) is hand-written anyway — 8 hand-written `impl Diff`
  coexist with it. Notably `Function` is hand-written but `Procedure` (a subset)
  is derived: an inconsistency with no principle behind it.
- **It's barely structured.** **63 of ~95 derived fields use
  `#[diff(via_debug)]`** (verified), i.e. `format!("{:?}")` string comparison.
  Two-thirds of the "structured diff" is actually a *stringified* diff. The
  macro's expressive core is mostly bypassed.
- **It's the worst code shape for the stated goal.** For a derived type like
  `Column`, the diff logic *exists in no file* — you can't grep it, breakpoint
  it, or read it inline; you must mentally expand the macro. For an AI agent or
  a new developer debugging a wrong diff path, this is maximally opaque. A
  hand-written impl (like `Table::diff`) is greppable and steppable.
- **Constitution scorecard:** fails "dependencies are a liability" (a published
  crate + 3 proc-macro deps + the documented two-crate publish-ordering footgun,
  to save ~250 mechanical LOC); fails "readability over cleverness"; fails
  "simplest architecture" (creates two divergent ways to implement one trait).

**Recommendation:** Delete the macro. Replace the derives with hand-written
impls using exhaustive `let Self { field, .. } = self;` destructuring — which
gives the *same* compile-time "you forgot a field" guarantee the macro's only
real benefit provides, with zero macro machinery and fully readable code.
(Stretch option: collapse *all* diff impls onto one serialize-tree walk for a
single uniform codepath — larger change, evaluate separately.) See also 3.2.

### 3.2 Two unrelated subsystems both called "diff" — rename one

This is the chief comprehension trap in the codebase, and it directly caused the
audit to nearly over-weight the macro:

- **`ir::eq::Diff` / `DiffMacro`** produces `Vec<Difference>` where `Difference`
  is `{ path: String, from: String, to: String }`. It is an **equivalence /
  round-trip verification** tool — used by `validate`, the testkit's
  `assert_canonical_eq`, and conformance. Outside tests it's used almost
  nowhere in production.
- **`diff::ChangeSet`** (the `diff()` function family) is the **actual migration
  engine** — and it does *not* use the macro at all. It decides equality with
  direct `PartialEq`, hand-written `structurally_eq`, and bespoke comparators.

They share only the word "diff." **Recommendation:** rename the equivalence
machinery to make "diff" unambiguously mean the change engine (it already exposes
`canonical_eq` — lean into `Equiv`/`canonical_eq` naming). This is a pure
clarity win and pairs naturally with removing the macro (3.1).

### 3.3 Illegal-state escape hatches (the constitution's own yardstick)

The IR is genuinely disciplined — newtypes and enums dominate, Option-over-
sentinel is the norm. But a few public-surface types let illegal states through
and should be fixed *before* 1.0 locks them:

1. **`ColumnType::Numeric { precision: Option<u16>, scale: Option<i16> }`**
   (`ir/column_type.rs:28`) — `scale: Some` + `precision: None` is illegal and
   guarded only by a runtime `unreachable!` (`column_type.rs:186`). This is the
   exact pattern the constitution names. Fix to `Numeric(Option<NumericPrecision
   { precision: u16, scale: Option<i16> }>)` and delete the panic.
2. **`ViewColumn` "unresolved" string sentinel** (`ir/view.rs:46`) — an
   unresolved alias-list type is `ColumnType::Other { raw: "unresolved" }` with a
   prose warning it "must never serialize." Use `Option<ColumnType>` or a real
   `Unresolved` variant.
3. **`GrantObjectPrivilege.signature: String`** (`diff/change.rs:169`) — invariant
   "empty for non-routine kinds" enforced only by comment. Make it
   `Option<RoutineSignature>`.
4. **Direction-as-bool**: `AlterDefaultPrivileges { is_grant: bool }`,
   `ViewChange::ReplaceBody { compatible: bool }`,
   `SetTableRowSecurity { enable }` — closed 2-state sets that should be enums.
5. **`AlterObjectOwner` carries both `kind` and `id`** whose variants partly
   re-encode each other (`diff/owner_op.rs:104`) — they can disagree.
6. **`IrError::InvalidIdentifier(String)` is overloaded for duplicate-object
   errors** (`ir/catalog.rs:316`) — a duplicate table is not an invalid
   identifier. Add `DuplicateObject`-style variants (the taxonomy already has
   them for event-triggers/aggregates/casts; extend to core collections).

### 3.4 Duplication that wants a helper (mechanical, high-value, low-risk)

Three near-identical patterns are copy-pasted across the object families. Each
is pure deletion with no behavior change, and each is *much* cheaper to fix
before 1.0 than after:

- **Owner+grants diffing** is open-coded ~10× (`diff_type_owner_grants`,
  `diff_function_owner_grants`, … and inline copies in schemas/sequences/tables/
  views). Collapse to one `diff_owner_and_grants(kind, qname, …, &mut out)`.
- **Owner/grants *fields*** are duplicated across 16/8 IR structs with identical
  doc comments. Extract a shared `Ownable { owner, grants }` embed (also
  collapses the parallel 16-arm loop in `ir/canon/mod.rs`).
- **SQL string-literal escaping** (`s.replace('\'', "''")`) is reimplemented 5×
  (`escape_sql_string`, `escape_sql_str`, `escape_sql_literal`, inline in
  `render_comment`). Collapse to one `sql_string_literal()` so literal hardening
  has a single site. (Identifier quoting, by contrast, is already correctly
  centralized in `Identifier::render_sql` and is injection-safe.)
- **Parse-side extractors**: `DefElem → String` is reimplemented 5–6× and
  "List-of-String → qname" ~10× despite a canonical `shared::qname_from_string_list`
  existing. Route callers through `builder::shared`.

### 3.5 Ordering invariants leaking into the diff layer

`ChangeSet` is documented "unordered" (plan owns ordering), but the grant
differs require callers to "push revoke before add" (`diff/grants.rs:23`) and
`subscriptions.rs:191` calls `out.sort()`. These execution-ordering contracts
belong in `plan`, which re-orders everything anyway. Moving them makes the
"unordered" contract true rather than aspirational. Related: `compatible`,
`function_can_or_replace`, and `ReplaceWithCascade` are *migration-strategy*
(plan) decisions computed in `diff`, which is why `plan/recreate_views.rs` then
*mutates the change vector* — the two layers co-author one `Vec<Change>`. Worth
a deliberate decision pre-1.0, though lower priority than 3.1–3.4.

---

## 4. Per-subsystem scorecard

| Subsystem | Grade | Summary |
|-----------|-------|---------|
| **catalog/** | A | Exemplary. Two-path IR risk fully mitigated by shared canonicalizer + 5-major Docker conformance. Version divergence tiny and well-contained (PG17/18 query files are 1 line). |
| **ir/** | A− | Disciplined, newtype/enum-heavy, centralized canon pipeline. Docked for the illegal-state hatches (3.3) and owner/grants duplication (3.4). |
| **parse/** | B+ | Well-typed, ~zero production `unwrap`, size is mostly the irreducible cost of lowering Postgres's untyped leaf AST. Real targets: duplicated AST walkers/extractors, an 18-argument `process_file`, and a **four-way, partially-incomplete normalization story** (the main false-positive-diff risk — paren-folding and commutative-operand sorting are deferred; qualifier-stripping is single-table-only). |
| **diff/** | B | Sound function-family design. Docked for owner/grants copy-paste, the "diff" naming collision, and strategy decisions leaking from plan. |
| **plan/** | B | Correct and safe; the online-rewrite patterns are the project's best safety work. Complexity concentrates in `recreate_views.rs` (1.2k) and the type cascade-replace heuristics. The 780-line `emit_change` dispatcher is long but not convoluted. |
| **lint/** | B+ | ~4k production LOC (not 9.5k). 25/47 rules are **diff-correctness guardrails** for the managed-schema model, not style nags — they earn their weight. Prime scope-creep vector: freeze the catalogue at 1.0. |
| **render/** | A− | Appropriately minimal (dump-only). Injection-safe identifier quoting. Only nit: the duplicated literal escapers (3.4) and the misleading name (it's not the migration emitter). |
| **macros** | F | Remove (3.1). |
| **CLI/executor** | A | Clean boundary; executor runs only core-rendered SQL; secrets (`${VAR}` interp) handled at apply time, never on disk. |

---

## 5. Pre-1.0 action list (prioritized)

**Tier A — fix before 1.0 (surface-locking; cheap now, expensive later):**

1. Remove `pgevolve-core-macros`; hand-write `Diff` impls with exhaustive
   destructuring (3.1). Eliminates a published crate + 3 deps + the
   publish-ordering footgun.
2. Fix the illegal-state hatches that sit on public types: `ColumnType::Numeric`,
   `ViewColumn` sentinel, grant `signature`, the direction-bools, the
   `AlterObjectOwner` redundancy (3.3).
3. Rename the equivalence `Diff`/`DiffMacro` machinery so "diff" means the change
   engine (3.2). Pairs with #1.
4. Decide explicitly: is the parse-side normalization gap (no paren-folding /
   operand-sorting; single-table qualifier-stripping) in-scope for 1.0 or a
   documented diff-stability limitation? This is the top user-visible-churn risk.
5. Decide explicitly: column rename/reorder — bounded feature, or documented
   limitation (2.5)?

**Tier B — simplification, safe any time (do before 1.0 if cheap):**

6. Extract the three duplication helpers: owner+grants diff, `Ownable` field
   embed, `sql_string_literal()` (3.4). Pure deletion.
7. Unify the Tier-3 drop/create paths (views/types/collations) behind one
   "recreate-with-dependents" engine; replace the raw type `CASCADE` (2.4).
8. Consolidate parse-side extractors/walkers into `builder::shared`; introduce a
   `ParseContext` to retire the 18-arg `process_file` (3.4).

**Tier C — policy / documentation:**

9. Write into the Constitution: **heap-data tables are never recreated** — only
   ALTERed (structurally enforces the prime directive; closes the door on the
   drop/create-for-tables idea) (2.4).
10. Freeze the lint rule catalogue at 1.0; new rules require a
    diff-correctness or safety justification (4).
11. Soften the `lib.rs` "I/O-free" doc to "no network I/O; DB access injected via
    `CatalogQuerier`" — `parse_directory` does read the filesystem.
12. Move the revoke-before-grant / `out.sort()` ordering contracts from `diff`
    into `plan` (3.5).

**Consider-cutting (breadth the simplicity-owner may not want to lock into the
public IR at 1.0)** — each is coverage-driven, low-frequency, and freezes
surface: the typed autovacuum reloption matrix (collapse into the existing
`extra` map), `UserTypeKind::Range` multirange detail, Subscriptions'
CREATE-only asymmetric fields, and the thin Cast/Aggregate models. Not a
recommendation to cut — a recommendation to *consciously decide* before 1.0,
since the constitution's "Full Postgres support" goal and the owner's
"simplicity over breadth" preference are in genuine tension here, and 1.0 is
where that tension must be resolved on the record.

---

## 6. What NOT to change (guard against over-correction)

- **Don't pursue drop/create for data-bearing tables.** (Section 2.) The
  absence of copy/swap logic is a feature.
- **Don't re-architect the pipeline.** parse→ir→diff→plan→render with the
  shared canonicalizer is correct.
- **Don't "simplify" the online-rewrite patterns** (`NOT VALID`/`VALIDATE`,
  CHECK-for-NOT-NULL, CONCURRENTLY). They look elaborate but are the project's
  core safety value.
- **Don't collapse the two parse/catalog front-ends into one.** The two-path
  cost buys an enforced symmetry that is the most-tested invariant in the repo.
- **Don't gut lint.** Most of it is diff-correctness machinery, not style.

---

## 7. Evidence index (spot-check anchors)

- No table-data copy path: `grep 'INSERT INTO' crates/pgevolve-core/src/plan/` → 0.
- Only `CASCADE` in codebase: `plan/rewrite/emit/user_type.rs:66`.
- Online-rewrite (anti-drop/create) core: `plan/rewrite/set_not_null_check_pattern.rs`.
- Largest file / drop-create engine: `plan/recreate_views.rs` (1,204 LOC).
- Macro: `pgevolve-core-macros/src/lib.rs` (151 LOC); `via_debug` used 63×;
  8 hand-written `impl Diff` coexist.
- Two "diff"s: `ir/eq.rs` + `ir/difference.rs` (equivalence) vs `diff/changeset.rs`
  (change engine).
- Illegal-state hatches: `ir/column_type.rs:186` (`unreachable!`), `ir/view.rs:46`
  ("unresolved"), `diff/change.rs:169` (`signature`).
- Symmetry enforcement: `conformance/src/assertions/apply.rs:136`; CI matrix
  `ci.yml` PG 14–18 with Docker in the conformance job.

---

*Prepared as input to the [v1.0 charter](./v1.md) surface-freeze decision.
Re-run the per-subsystem deep-dives if the code moves materially before 1.0.*
