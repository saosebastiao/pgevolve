# Phase 3 — Diff-layer cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the changeset illegal-state hatches (decisions 3, 4, 5), dedup the owner/grants diff logic and SQL-literal escapers (decision 7a/7b), and move the grant-ordering invariant out of the diff layer into plan (decision 11). No migration-output change except where a representation is deliberately tightened on the explicitly-unstable changeset JSON.

**Architecture:** `crate::diff` produces a `ChangeSet` of `Change` variants (`diff/change.rs`), which `plan/` orders and `plan/rewrite/` renders. The owner/grants diffing pattern is copy-pasted across ~13 `diff/*.rs` files; `kind`+`qname`+`signature` is a redundant triple on three `Change` variants where `signature` is only valid for routines; several closed 2-state fields are bare `bool`s; and the grant emit-order contract (revoke-before-grant) leaked into the diff layer as an insertion-order requirement.

**Tech Stack:** Rust, serde, thiserror. STRICT lints (clippy pedantic+nursery, `-D warnings`); no `unwrap`/`expect` in production. Per-commit gate INCLUDES `cargo fmt --check` (Phase 1 lesson). Commits go directly to `main`. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Prerequisite:** Phase 2 CI green across all 5 PG majors.

**A design choice for the reviewer (Task 3):** decisions 3 and 5 are the same wart. This plan unifies them into one `GrantableObject` enum that carries a routine signature only on its `Function`/`Procedure` variants — eliminating the `kind`+`qname`+`signature` triple and the separate `OwnerObjectKind`/`OwnedObjectId`. The minimal alternative (signature → `Option<RoutineSignature>` AND a separate `AlterObjectOwner` collapse) is smaller but leaves a representable illegal state (a non-routine carrying a signature) and keeps two near-duplicate object-reference types. The unified approach is recommended; flag at plan-review if you prefer minimal.

---

## Pre-flight
- [ ] **P1:** Confirm Phase 2 CI green across all 5 PG majors (`gh run list --branch main --limit 3`).
- [ ] **P2:** Green baseline: `cargo fmt --check && cargo test -p pgevolve-core && cargo clippy -p pgevolve-core --all-targets`.

---

### Task 1: Replace closed 2-state `bool`s in `Change` with enums (decision 4)

**Why:** `AlterDefaultPrivileges.is_grant: bool`, `SetTableRowSecurity.enable: bool`, `SetTableForceRowSecurity.force: bool`, and `ViewChange::ReplaceBody.compatible: bool` (+ the MV `ReplaceBody`) are closed 2-state sets better expressed as enums. `compatible` additionally encodes a *migration strategy* (CREATE-OR-REPLACE vs DROP+CREATE) — naming it as such clarifies intent.

**Files:** `crates/pgevolve-core/src/diff/change.rs` (defs); ripple in `diff/default_privileges.rs`, `diff/tables.rs`, `diff/views.rs`, `plan/recreate_views.rs`, `plan/rewrite/{grants,views,mv,table}.rs` (compiler-flagged).

- [ ] **Step 1: Define the enums** in `change.rs` (near the variants that use them). Derive `Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize` and `#[serde(rename_all = "snake_case")]`:
```rust
/// Direction of a default-privilege adjustment.
pub enum GrantDirection { Grant, Revoke }
/// A table's ROW LEVEL SECURITY toggle.
pub enum RowSecurity { Enable, Disable }
/// A table's FORCE ROW LEVEL SECURITY toggle.
pub enum ForceRowSecurity { Force, NoForce }
/// How a view/MV body change is applied.
/// `InPlace` = `CREATE OR REPLACE` (compatible signature); `Recreate` = explicit
/// ordered DROP + CREATE of the object and its dependents (incompatible).
pub enum BodyReplaceStrategy { InPlace, Recreate }
```
- [ ] **Step 2: Swap the fields:** `is_grant: bool` → `direction: GrantDirection`; `enable: bool` → `security: RowSecurity`; `force: bool` → `force: ForceRowSecurity`; both `ReplaceBody { compatible: bool }` → `strategy: BodyReplaceStrategy`. Update the doc comments (drop the `true =`/`false =` prose).
- [ ] **Step 3: Compiler-guided ripple.** `cargo build -p pgevolve-core`. At each construction site, map `true`/`false` per the old doc: `is_grant: true` → `GrantDirection::Grant`; `enable: true` → `RowSecurity::Enable`; `force: false` → `ForceRowSecurity::NoForce`; `compatible: true` → `BodyReplaceStrategy::InPlace`, `compatible: false` → `BodyReplaceStrategy::Recreate`. At each match/render site (`plan/rewrite/...`), branch on the enum instead of the bool. Re-run until clean.
- [ ] **Step 4: Verify + commit.** `cargo fmt --check && cargo clippy -p pgevolve-core --all-targets && cargo test -p pgevolve-core`. Snapshot fixtures may shift on the changeset JSON (e.g. `"is_grant": true` → `"direction": "grant"`); re-bless via `cargo run -p xtask -- bless` and confirm shape-only. Commit:
```
refactor(diff): replace closed-2-state bools in Change with enums
```

---

### Task 2: One `sql_string_literal()` helper (decision 7a, escapers)

**Why:** the `'`-doubling SQL-literal escape is reimplemented as `escape_sql_string` (triggers.rs:156, extensions.rs:52), `escape_sql_str` (collations.rs:70), `escape_sql_literal` (subscriptions.rs:198), plus inline in `render_comment`. One helper = one place to harden literal quoting.

**Files:** create the helper in `crates/pgevolve-core/src/plan/rewrite/sql.rs` (next to `render_comment` / identifier rendering); update the 4 named escapers' call sites + `render_comment`.

- [ ] **Step 1:** Add to `plan/rewrite/sql.rs`:
```rust
/// Escape a string for use as a single-quoted SQL string literal (doubles
/// embedded single quotes). The sole place literal escaping is defined.
#[must_use]
pub(crate) fn sql_string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
```
Check each existing escaper first: if it returns the **quoted** form (`'...'`) use `sql_string_literal` directly; if it returns the **bare-doubled** form (no surrounding quotes), either adapt callers or add a `sql_string_literal_inner` returning just the doubled body. Match existing call-site expectations exactly — do not change emitted SQL.
- [ ] **Step 2:** Replace the 4 named fns + the inline `render_comment` escape with calls to the helper; delete the now-unused fns.
- [ ] **Step 3: Verify + commit.** Gate (`fmt`/`clippy`/`test`). The emitted SQL must be byte-identical — existing render/conformance tests prove it. Commit:
```
refactor(plan): single sql_string_literal() helper (dedup 5 escapers)
```

---

### Task 3: Unify `GrantableObject` — fix grant `signature` + `AlterObjectOwner` redundancy (decisions 3 & 5)

**Why:** `GrantObjectPrivilege`, `RevokeObjectPrivilege`, and `AlterObjectOwner` each carry a `kind: OwnerObjectKind` + `qname`/`id` + a `signature: String` that is meaningful only for routines ("empty for non-routine kinds" by comment). Replace all three's object-reference with one enum that makes the signature representable only for routines, and that subsumes both `OwnerObjectKind` (the SQL keyword) and `OwnedObjectId` (the name shape).

**Files:** `crates/pgevolve-core/src/diff/owner_op.rs` (new type, replaces `OwnerObjectKind` + `OwnedObjectId`); `diff/change.rs` (the 3 variants); ripple across `diff/*.rs` construction sites and `plan/rewrite/{grants,owner,...}.rs` renderers.

- [ ] **Step 1: Define `RoutineSignature` and `GrantableObject`** in `owner_op.rs`:
```rust
/// A routine's argument-type signature, e.g. `(integer, text)`. Rendered verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineSignature(pub String);

/// A grantable / ownable object, carrying exactly the data its kind needs.
/// A routine signature is representable ONLY on Function/Procedure — the old
/// `signature: String` (empty for non-routines) illegal state is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantableObject {
    Schema(Identifier),
    Sequence(QualifiedName),
    Table(QualifiedName),
    View(QualifiedName),
    MaterializedView(QualifiedName),
    UserType(QualifiedName),
    Function { name: QualifiedName, signature: RoutineSignature },
    Procedure { name: QualifiedName, signature: RoutineSignature },
    Statistic(QualifiedName),
    Collation(QualifiedName),
    Publication(Identifier),
    Subscription(Identifier),
}

impl GrantableObject {
    /// SQL keyword for `GRANT ... ON <keyword> <name>` / `ALTER <keyword> ... OWNER TO`.
    pub fn sql_keyword(&self) -> &'static str { /* match → "SCHEMA"/"TABLE"/... (port OwnerObjectKind::sql_keyword) */ }
    /// The object's target name + routine signature suffix for SQL.
    pub fn render_target(&self) -> String { /* qname/ident render + "(sig)" for routines (port OwnedObjectId::render_sql + signature) */ }
}
```
Keep the variant set aligned with the kinds the renderer supports (cross-check the current `OwnerObjectKind` arms). NOTE: `#[serde(tag = "kind")]` cannot wrap an `Option` in a newtype variant — not an issue here (struct/newtype variants without `Option` payloads), but see [[reference_serde_internal_tag_option]] if you adjust.

- [ ] **Step 2: Refactor the 3 `Change` variants** in `change.rs`:
  - `GrantObjectPrivilege { qname, kind, signature, grant }` → `GrantObjectPrivilege { object: GrantableObject, grant }`.
  - `RevokeObjectPrivilege { ... }` → `RevokeObjectPrivilege { object: GrantableObject, grant }`.
  - `AlterObjectOwner(AlterObjectOwner)` where `AlterObjectOwner { kind, id, signature, from, to }` → `AlterObjectOwner { object: GrantableObject, from: Option<Identifier>, to: Identifier }`.
  Delete `OwnerObjectKind` and `OwnedObjectId` (now subsumed) unless something outside owner/grants uses them — grep first; if a stray user exists, migrate it too.

- [ ] **Step 3: Compiler-guided ripple — construction sites.** `cargo build -p pgevolve-core`. Every site that built one of these variants (in `diff/types.rs`, `routines.rs`, `schemas.rs`, `sequences.rs`, `tables.rs`, `views.rs`, `collations.rs`, `statistics.rs`, `publications.rs`, `subscriptions.rs`, `default_privileges.rs`) now constructs a `GrantableObject` variant. For routines, build `Function { name, signature: RoutineSignature(sig) }`; for non-routines, the bare-name variant (no signature — the illegal state is now impossible).

- [ ] **Step 4: Compiler-guided ripple — renderers.** In `plan/rewrite/grants.rs` and the owner-emit path, replace `kind.sql_keyword()` + `qname.render_sql()` + signature concatenation with `object.sql_keyword()` + `object.render_target()`. The emitted SQL must be byte-identical.

- [ ] **Step 5: Verify + commit.** Gate. Re-bless changeset snapshots (shape-only: the JSON nests `kind`/`signature` inside `object`). Confirm emitted SQL unchanged via render/conformance tests. Commit:
```
refactor(diff): unify GrantableObject (signature valid only for routines)
```

---

### Task 4: One owner+grants diff helper (decision 7a/7b)

**Why:** the "diff owner → emit `AlterObjectOwner`; `diff_grants` → loop revoke/add/unmanaged emitting `Revoke/GrantObjectPrivilege` + observations" sequence is copy-pasted across ~13 `diff/*.rs` files (named `diff_type_owner_grants`/`diff_function_owner_grants`/`diff_procedure_owner_grants` + inline copies in schemas/sequences/tables/views/collations/statistics/publications/aggregates). Extract one helper. **Keep the flat `owner`/`grants` fields on the IR structs** (decision 7b — no `Ownable` embed); the dedup is of the diff *logic*, not the field layout.

**Files:** add the helper to `diff/grants.rs` (or a new `diff/owner_grants.rs`); replace the ~13 call sites.

- [ ] **Step 1: Design the helper** — signature roughly:
```rust
/// Emit the owner change + grant adds/revokes/unmanaged observations for one
/// grantable object, in the canonical order (owner, then grants).
pub(crate) fn diff_owner_and_grants(
    object: &GrantableObject,           // from Task 3
    target_owner: Option<&Identifier>, source_owner: Option<&Identifier>,
    target_grants: &[Grant], source_grants: &[Grant],
    managed_roles: &BTreeSet<Identifier>,
    out: &mut ChangeSet,
)
```
It encapsulates: emit `AlterObjectOwner { object, from, to }` when `source_owner` is `Some` and differs; call `diff_grants`; push `RevokeObjectPrivilege`/`GrantObjectPrivilege` (built from `object.clone()`); push the `unmanaged`/`revoke-with-owner` observations. Read `diff/types.rs:56` (`diff_type_owner_grants`) and `diff/sequences.rs` as the reference behavior — the helper must reproduce it exactly.
- [ ] **Step 2: Replace each of the ~13 sites** with a `diff_owner_and_grants(...)` call, passing the right `GrantableObject` variant. Delete the now-dead `diff_*_owner_grants` fns.
- [ ] **Step 3: Verify + commit.** Gate. Behavior must be identical — the existing per-object grant/owner tests + conformance prove it. (Do NOT change emit order here; that's Task 5.) Commit:
```
refactor(diff): single diff_owner_and_grants helper (dedup ~13 sites)
```

---

### Task 5: Move the grant emit-order invariant from diff → plan (decision 11)

**Why:** `diff_grants`'s doc requires callers to "push `to_revoke` before `to_add`," and `diff/subscriptions.rs:191` calls `out.sort()` — execution-ordering concerns that leaked into the diff layer, contradicting `ChangeSet`'s documented "unordered" contract. Ordering belongs in `plan`, which already topologically orders steps.

**Files:** `plan/ordering.rs` (or wherever step order is assigned); `diff/grants.rs` (relax the doc), `diff/subscriptions.rs` (remove `out.sort()`), `diff/owner_grants.rs` (Task 4 helper — order no longer load-bearing).

- [ ] **Step 1: Add a plan-side ordering rule** so that for the same object, a `RevokeObjectPrivilege` is emitted before a `GrantObjectPrivilege` (the WGO-change correctness case in `diff/grants.rs`'s issue-#33 tests). Read `plan/ordering.rs`/`edges.rs` to find where same-node step order is decided; add an edge/comparator: revoke-before-grant for the same `(object, grantee, privilege)`. If subscriptions relied on `out.sort()` for deterministic option order, move that determinism into the plan/render step for subscriptions.
- [ ] **Step 2: Relax the diff layer.** Remove the "callers must push revoke before add" requirement from `diff_grants`'s doc (keep the WGO explanation, but state ordering is now enforced by plan). Remove `out.sort()` from `diff/subscriptions.rs`. The diff helper (Task 4) may now push in any order.
- [ ] **Step 3: Verify the WGO regression still holds end-to-end.** The issue-#33 behavior (REVOKE before GRANT on a with-grant-option change) MUST still produce correct SQL — now via plan ordering, not diff insertion order. Find/keep an e2e or plan-level test asserting the emitted step order; add one if only the diff-level contract test existed. Run the full grant/subscription test set + conformance.
- [ ] **Step 4: Verify + commit.** Gate. Commit:
```
refactor(plan): own the grant revoke-before-grant ordering (was leaked into diff)
```

---

### Task 6: Phase wrap — full workspace gate + push
- [ ] **Step 1:** `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps && cargo deny check` — all green.
- [ ] **Step 2:** Confirm illegal states gone: `grep -rn 'signature: String\|is_grant\|OwnerObjectKind\|OwnedObjectId' crates/pgevolve-core/src/diff/` → empty (or only intended). `grep -rn 'fn escape_sql' crates/` → empty. `grep -rn 'diff_.*_owner_grants' crates/` → empty.
- [ ] **Step 3:** `git push origin main`; wait for CI green across all 5 PG majors before Phase 3 is done.

---

## Self-review notes
- **Spec coverage:** Task 1 = decision 4 (bools→enums); Task 2 = decision 7a (escapers); Task 3 = decisions 3 + 5 (signature + AlterObjectOwner, unified); Task 4 = decision 7a/7b (owner/grants dedup, flat fields kept); Task 5 = decision 11 (ordering → plan).
- **The load-bearing risk in Tasks 3–4 is behavior preservation** — the emitted GRANT/REVOKE/OWNER SQL must be byte-identical. The changeset JSON shape changes deliberately (unstable surface); re-bless and confirm shape-only.
- **Task 5 is the subtlest** — the WGO revoke-before-grant correctness (issue #33) must survive the move to plan ordering. Verify with an end-to-end/plan-level order assertion, not just the (now-removed) diff-insertion contract.
- Per-commit gate INCLUDES `cargo fmt --check`.
- **Decisions deliberately NOT in this phase:** the `changeset.rs` `object_label`/`privilege_label` String duplication in the observation structs was noted by review but not a chosen decision — leave it unless it falls out naturally of Task 3.
