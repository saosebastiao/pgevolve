# `CREATE COLLATION` + `RANGE TYPE` Implementation Plan (v0.3.8)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bundle two roadmap items (CREATE COLLATION + RANGE TYPE) into v0.3.8, mirroring the v0.3.7 (STATISTICS + VIEW CHECK OPTION) ship pattern.

**Architecture:** RANGE TYPE is purely additive on the existing `UserType` machinery — a new `UserTypeKind::Range` variant slotting into the established enum/domain/composite flow. COLLATION is a new top-level managed kind following the Publication / Subscription / Statistic shape from v0.3.4-v0.3.7: new IR module, new `Catalog::collations` field, new `CollationChange` sub-enum, new lint rules, dedicated reader/differ/renderer. The two features share no code paths so they can be staged independently per pipeline layer.

**Tech Stack:** Rust 1.95+, `pg_query` 6.x AST, `tokio_postgres` for catalog reads, `proptest` 1.11, `serde_json` + `toml` for plan persistence.

**Spec:** [`../specs/2026-05-28-collation-and-range-type-design.md`](../specs/2026-05-28-collation-and-range-type-design.md)

---

## Pre-flight

Before starting any stage:

1. Read [`docs/CONSTITUTION.md`](../../CONSTITUTION.md) — binding principles (license, deps, type safety, lint strictness, no `unwrap`/`expect`, signed releases).
2. Read [`CLAUDE.md`](../../../CLAUDE.md) — project operating directives.
3. Read the spec linked above end-to-end.
4. Confirm `main` is green: `git log --oneline -3` shows a clean ship state; `gh run list --branch main --limit 1` shows ✅.

## Per-stage verify gate (run before every commit)

```sh
cargo fmt --check                                            # 0 diffs
cargo clippy --workspace --all-targets -- -D warnings        # 0 warnings
cargo test --lib -p pgevolve-core                            # all pass
cargo test --lib -p pgevolve                                 # all pass
cargo test --lib -p pgevolve-testkit                         # all pass
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace   # cargo doc clean (new in v0.3.8 cycle)
```

The `cargo doc -D warnings` step is new — it caught two regressions in this session's cleanups. Every refactor + new public item must keep intra-doc links resolvable.

## Subagent dispatch template (one per stage)

Each stage = one commit = one fresh implementer subagent. Use the standard template from `~/.claude/plugins/cache/claude-plugins-official/superpowers/5.1.0/skills/subagent-driven-development/implementer-prompt.md`, with the following constants for this plan:

- Branch: `main` (commits land directly; see CLAUDE.md §9).
- Co-author trailer: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Established patterns to reference when reviewing implementer output:
  - Nested `Change::*` sub-enum pattern (from the v0.3.7 retrospective #4): any new variants for a new object go in a sub-enum, not flat on `Change`.
  - `check_unmanaged_objects` helper (from #3): any new `unmanaged-X` lint uses the helper at `lint::rules::check_unmanaged_objects`.
  - `OwnedObjectId` for owner ops (from #2): never construct synthetic `QualifiedName::new("__cluster__", …)`.

After each stage:
1. Spec compliance review (per the subagent-driven-development skill).
2. Code-quality review.
3. Mark task complete; advance to next stage.

---

## File structure

### Created

```
crates/pgevolve-core/src/ir/collation.rs                    (Collation, CollationProvider, BUILTIN_COLLATIONS)
crates/pgevolve-core/src/ir/canon/collations.rs              (canon pass)
crates/pgevolve-core/src/catalog/collations.rs               (Querier helpers)
crates/pgevolve-core/src/catalog/queries/collations.rs       (per-PG-version SQL)
crates/pgevolve-core/src/catalog/assemble/collations.rs      (rows → Vec<Collation>)
crates/pgevolve-core/src/parse/builder/create_collation_stmt.rs
crates/pgevolve-core/src/diff/collations.rs                  (diff_collations + CollationChange emission)
crates/pgevolve-core/src/plan/rewrite/collations.rs          (SQL renderer for 5 StepKinds)
crates/pgevolve-core/src/lint/rules/unmanaged_collation.rs
crates/pgevolve-core/src/lint/rules/column_references_unmanaged_collation.rs
crates/pgevolve-core/src/lint/rules/range_type_references_unmanaged_subtype.rs
crates/pgevolve-core/src/lint/rules/nondeterministic_collation_requires_pg_12.rs
crates/pgevolve-core/src/lint/rules/builtin_provider_requires_pg_17.rs
crates/pgevolve-testkit/src/ir_generator/collation.rs        (arb_collation + arb_collation_provider)
docs/spec/collations.md                                       (capability catalogue entry)
```

Plus 12 conformance fixture directories under `crates/pgevolve-conformance/tests/cases/`:

```
objects/collations/{create-libc,create-icu,create-nondeterministic,drop,comment-on,rename}/
objects/ranges/{create-simple-int4range,create-with-opclass,create-with-canonical-fn,drop,column-with-range-type}/
scenarios/column-references-managed-collation/
```

### Modified

```
crates/pgevolve-core/src/ir/user_type.rs                     (+UserTypeKind::Range variant)
crates/pgevolve-core/src/ir/catalog.rs                       (+collations field)
crates/pgevolve-core/src/ir/canon/mod.rs                     (register collation canon pass)
crates/pgevolve-core/src/ir/mod.rs                           (pub use ir::collation)
crates/pgevolve-core/src/catalog/mod.rs                      (register collations module)
crates/pgevolve-core/src/catalog/queries/{mod,pg14,pg15,pg16,pg17,pg18}.rs  (wire Collations query)
crates/pgevolve-core/src/catalog/queries/types.rs            (extend pg_type query to LEFT JOIN pg_range)
crates/pgevolve-core/src/catalog/assemble/mod.rs             (register collations + extend user_types for ranges)
crates/pgevolve-core/src/catalog/assemble/user_types.rs      (read pg_range fields, build Range variant)
crates/pgevolve-core/src/parse/builder/mod.rs                (register create_collation_stmt)
crates/pgevolve-core/src/parse/builder/create_stmt.rs        (extend build_user_type for RangeBoundsClause)
crates/pgevolve-core/src/parse/statement.rs                  (CreateCollationStmt arm)
crates/pgevolve-core/src/parse/builder/comment_stmt.rs       (Collation kind)
crates/pgevolve-core/src/diff/mod.rs                         (wire diff_collations)
crates/pgevolve-core/src/diff/types.rs                       (diff_range helper)
crates/pgevolve-core/src/diff/change.rs                      (CollationChange + Change::Collation)
crates/pgevolve-core/src/plan/raw_step.rs                    (5 new StepKind variants)
crates/pgevolve-core/src/plan/plan.rs                        (kind_name + parse_kind_name + round-trip list)
crates/pgevolve-core/src/plan/ordering.rs                    (partition arms + change_node)
crates/pgevolve-core/src/plan/rewrite/mod.rs                 (emit arm dispatching to collations)
crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs      (extend Create arm for Range kind)
crates/pgevolve-core/src/plan/edges.rs                       (NodeId::Collation + 4 edge types)
crates/pgevolve-core/src/lint/universal.rs                   (register 5 new rules in correct entry points)
crates/pgevolve-core/src/lint/rules/mod.rs                   (pub mod entries)
crates/pgevolve-core/src/lint/test_helpers.rs                (Collation fixture helper)
crates/pgevolve-testkit/src/ir_generator/mod.rs              (arb_collation in arbitrary_catalog)
crates/pgevolve/src/commands/diff.rs                         (print_human arm + change_kind_name)
docs/spec/objects.md                                         (promote 📋 → ✅ for both kinds)
docs/spec/roadmap.md                                         (move v0.3.8 row to Shipped)
docs/spec/README.md                                          (naming-conventions paragraph for v0.3.8)
CHANGELOG.md                                                 (v0.3.8 entry)
Cargo.toml                                                   (workspace.package.version: 0.3.7 → 0.3.8)
crates/pgevolve/Cargo.toml                                   (pgevolve-core version constraint 0.3.7 → 0.3.8)
```

---

## Stage 1 — RANGE TYPE end-to-end (IR + parse + catalog + diff + plan + 1 fixture)

Range type is small enough to land in a single end-to-end stage. The full COLLATION pipeline gets its own stages 2-7.

**Files:**
- Modify: `crates/pgevolve-core/src/ir/user_type.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/types.rs`
- Modify: `crates/pgevolve-core/src/catalog/assemble/user_types.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/create_stmt.rs`
- Modify: `crates/pgevolve-core/src/diff/types.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs`
- Modify: `crates/pgevolve-core/src/plan/edges.rs` (Range → Function edges)
- Test: inline `#[cfg(test)] mod tests` in each modified file
- Create: `crates/pgevolve-conformance/tests/cases/objects/ranges/create-simple-int4range/{before.sql,after.sql,fixture.toml,expected/plan.sql}`

### Step 1.1: Add `UserTypeKind::Range` variant

- [ ] Add to `crates/pgevolve-core/src/ir/user_type.rs` immediately after the `Composite` arm of `UserTypeKind`:

```rust
/// `CREATE TYPE … AS RANGE (…)`.
Range {
    /// Element type — `pg_range.rngsubtype`.
    subtype: QualifiedName,
    /// Optional opclass for the subtype's comparison.
    subtype_opclass: Option<QualifiedName>,
    /// Optional collation (only meaningful for collatable subtypes like text).
    collation: Option<QualifiedName>,
    /// Optional canonical function — `pg_range.rngcanonical`.
    canonical: Option<QualifiedName>,
    /// Optional subtype_diff function — `pg_range.rngsubdiff`.
    subtype_diff: Option<QualifiedName>,
    /// Custom multirange-type name (`None` → PG auto-names `<range>_multirange`).
    multirange_type_name: Option<Identifier>,
},
```

- [ ] Run: `cargo check -p pgevolve-core` and confirm only "non-exhaustive match" errors at expected call sites.
- [ ] Add a unit test in `ir/user_type.rs`:

```rust
#[test]
fn range_variant_serde_round_trip() {
    let r = UserType {
        qname: qn("app", "tsrange_co"),
        kind: UserTypeKind::Range {
            subtype: qn("pg_catalog", "timestamptz"),
            subtype_opclass: None,
            collation: None,
            canonical: None,
            subtype_diff: None,
            multirange_type_name: None,
        },
        owner: None,
        comment: None,
        grants: vec![],
    };
    let json = serde_json::to_string(&r).unwrap();
    let back: UserType = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}
```

- [ ] Verify gate: `cargo test --lib -p pgevolve-core ir::user_type` passes.

### Step 1.2: Extend differ for Range kind

- [ ] In `crates/pgevolve-core/src/diff/types.rs`, add a `diff_range` helper called from `diff_one_user_type` when both sides are `Range`:

```rust
fn diff_range(catalog: &UserType, source: &UserType, out: &mut ChangeSet) {
    let (cat_subtype, cat_opclass, cat_collation, cat_canonical, cat_diff, cat_mrtn) =
        match &catalog.kind {
            UserTypeKind::Range { subtype, subtype_opclass, collation, canonical,
                                  subtype_diff, multirange_type_name } =>
                (subtype, subtype_opclass, collation, canonical, subtype_diff, multirange_type_name),
            _ => unreachable!("diff_range called on non-Range kind"),
        };
    let (src_subtype, src_opclass, src_collation, src_canonical, src_diff, src_mrtn) =
        match &source.kind {
            UserTypeKind::Range { subtype, subtype_opclass, collation, canonical,
                                  subtype_diff, multirange_type_name } =>
                (subtype, subtype_opclass, collation, canonical, subtype_diff, multirange_type_name),
            _ => unreachable!("diff_range called on non-Range kind"),
        };

    if (cat_subtype, cat_opclass, cat_collation, cat_canonical, cat_diff, cat_mrtn)
        != (src_subtype, src_opclass, src_collation, src_canonical, src_diff, src_mrtn)
    {
        out.push(
            Change::UserType(UserTypeChange::ReplaceWithCascade {
                source: source.clone(),
                catalog: catalog.clone(),
            }),
            Destructiveness::RequiresApproval {
                reason: format!("range type {} structural change", source.qname),
            },
        );
        return; // structural change handled; skip per-field diffs
    }
    // Comment-only change (handled by surrounding diff_one_user_type via SetComment).
}
```

- [ ] Wire into `diff_one_user_type` next to the existing `diff_domain` dispatch:

```rust
(UserTypeKind::Range { .. }, UserTypeKind::Range { .. }) => diff_range(catalog, source, out),
```

- [ ] Add test in `diff/types.rs` tests block:

```rust
#[test]
fn diff_range_subtype_change_emits_replace_with_cascade() {
    // Build a target with int4 subtype, source with int8 → expect ReplaceWithCascade.
    let target = catalog_with_range(qn("pg_catalog", "int4"));
    let source = catalog_with_range(qn("pg_catalog", "int8"));
    let mut cs = ChangeSet::new();
    diff_user_types(&target, &source, &mut cs);
    assert!(cs.iter().any(|e| matches!(
        &e.change,
        Change::UserType(UserTypeChange::ReplaceWithCascade { .. })
    )));
}
```

- [ ] Verify gate: `cargo test --lib -p pgevolve-core diff::types` passes.

### Step 1.3: Extend catalog reader

- [ ] In `crates/pgevolve-core/src/catalog/queries/types.rs`, extend the existing user-types query with:

```sql
LEFT JOIN pg_range r ON r.rngtypid = t.oid
LEFT JOIN pg_type st ON st.oid = r.rngsubtype
LEFT JOIN pg_namespace stn ON stn.oid = st.typnamespace
LEFT JOIN pg_opclass o ON o.oid = r.rngsubopc
LEFT JOIN pg_namespace on_ ON on_.oid = o.opcnamespace
LEFT JOIN pg_collation c ON c.oid = r.rngcollation
LEFT JOIN pg_namespace cn ON cn.oid = c.collnamespace
LEFT JOIN pg_proc canon ON canon.oid = r.rngcanonical
LEFT JOIN pg_namespace cann ON cann.oid = canon.pronamespace
LEFT JOIN pg_proc dif ON dif.oid = r.rngsubdiff
LEFT JOIN pg_namespace difn ON difn.oid = dif.pronamespace
LEFT JOIN pg_type mr ON mr.oid = r.rngmultitypid
```

Add to the SELECT list:

```sql
, r.rngtypid IS NOT NULL AS is_range
, stn.nspname AS rng_subtype_schema, st.typname AS rng_subtype_name
, on_.nspname AS rng_subopc_schema, o.opcname AS rng_subopc_name
, cn.nspname AS rng_collation_schema, c.collname AS rng_collation_name
, cann.nspname AS rng_canonical_schema, canon.proname AS rng_canonical_name
, difn.nspname AS rng_subdiff_schema, dif.proname AS rng_subdiff_name
, mr.typname AS rng_multirange_name
```

Also extend the existing `pg_type.typtype` filter to also exclude `'m'` (multirange) so auto-generated multirange types are skipped.

- [ ] In `crates/pgevolve-core/src/catalog/assemble/user_types.rs`, extend the `match typtype` arms with a `'r' => build_range_kind(&row)` branch. Implementation pulls the optional qnames from the joined columns; the multirange name compares against the auto-generated `<range>_multirange` pattern — if they match, `multirange_type_name` is `None`; otherwise `Some(Identifier::from_unquoted(rng_multirange_name))`.

- [ ] Add tier-2 fixture under `crates/pgevolve-core/tests/fixtures/round_trip/` covering one range type. The existing `dump_round_trip` harness picks it up automatically.

- [ ] Verify gate: `cargo test --lib -p pgevolve-core catalog::assemble::user_types` passes.

### Step 1.4: Extend parser

- [ ] In `crates/pgevolve-core/src/parse/builder/create_stmt.rs::build_user_type`, add a `DefineStmtKind::Range` arm that decodes the `pg_query` AST's `definition` list into the `Range { … }` variant. Reject unknown option names with `ParseError::Structural` naming the bad key.

- [ ] Parser unit test in `parse/builder/create_stmt.rs`:

```rust
#[test]
fn parse_range_with_subtype_only() {
    let sql = "CREATE TYPE app.tsrange_co AS RANGE (subtype = timestamptz);";
    let cat = parse_to_catalog(sql);
    let rt = cat.user_types.iter().find(|t| t.qname.name.as_str() == "tsrange_co").unwrap();
    assert!(matches!(&rt.kind, UserTypeKind::Range { .. }));
}

#[test]
fn parse_range_rejects_unknown_option() {
    let sql = "CREATE TYPE app.bad AS RANGE (subtype = int4, bogus = 1);";
    let err = parse_to_catalog_err(sql);
    assert!(matches!(err, ParseError::Structural { .. }));
}
```

- [ ] Verify gate: `cargo test --lib -p pgevolve-core parse::builder::create_stmt` passes.

### Step 1.5: Extend plan renderer

- [ ] In `crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs`, extend the `UserTypeChange::Create` arm to render `CREATE TYPE qname AS RANGE (...)` when the kind is `Range`. Option list order: `subtype`, then any of `subtype_opclass`, `collation`, `canonical`, `subtype_diff`, `multirange_type_name` that are `Some`. Identifier rendering uses `QualifiedName::render_sql` / `Identifier::render_sql`.

- [ ] Renderer unit test in the same file:

```rust
#[test]
fn render_create_range_minimal() {
    let rt = make_range("app", "tsr", qn("pg_catalog", "timestamptz"));
    let sql = render_create_user_type(&rt);
    assert_eq!(sql.trim(), "CREATE TYPE app.tsr AS RANGE (subtype = pg_catalog.timestamptz);");
}
```

- [ ] Verify gate: `cargo test --lib -p pgevolve-core plan::rewrite::emit::user_type` passes.

### Step 1.6: Extend dep graph

- [ ] In `crates/pgevolve-core/src/plan/edges.rs::build_create_graph`, after the existing user-type edge logic, when the kind is `Range`:
  - If `canonical` is `Some(qn)` and qn matches a managed function → add `NodeId::Function(...) → NodeId::Type(rt.qname)` edge.
  - Same for `subtype_diff`.
  - If `subtype` matches a managed user-type (not a built-in) → `NodeId::Type(subtype) → NodeId::Type(rt.qname)` edge.

(Collation edge added in Stage 7 once `NodeId::Collation` exists.)

- [ ] Unit test in `plan/edges.rs`:

```rust
#[test]
fn range_canonical_fn_adds_edge() {
    let mut cat = Catalog::empty();
    cat.schemas.push(Schema::new(id("app")));
    cat.functions.push(make_function("app", "canon_fn"));
    cat.user_types.push(make_range_with_canonical("app", "myrange", qn("app", "canon_fn")));
    let g = build_create_graph(&cat);
    assert!(g.edges().any(|(from, to)|
        *from == NodeId::Function(qn("app", "canon_fn")) &&
        *to == NodeId::Type(qn("app", "myrange"))
    ));
}
```

- [ ] Verify gate: `cargo test --lib -p pgevolve-core plan::edges` passes.

### Step 1.7: Conformance fixture

- [ ] Create `crates/pgevolve-conformance/tests/cases/objects/ranges/create-simple-int4range/before.sql`:

```sql
CREATE SCHEMA app;
```

- [ ] Create `.../after.sql`:

```sql
CREATE SCHEMA app;
CREATE TYPE app.int_window AS RANGE (subtype = int4);
```

- [ ] Create `.../fixture.toml`:

```toml
[meta]
description = "CREATE TYPE … AS RANGE (subtype = int4) with no extras"

[pg]
majors = [14, 15, 16, 17, 18]

[expect.plan]
order = []
touches_only = ["app", "app.int_window"]
```

- [ ] Run: `cargo xtask bless objects/ranges/create-simple-int4range` (needs Docker). Commit the generated `expected/plan.sql` + `expected/dep-graph.dot`.

### Step 1.8: Stage 1 verify gate + commit

- [ ] Run the full per-stage verify gate (see Pre-flight section).
- [ ] Run conformance: `cargo test --release -p pgevolve-conformance -- conformance_suite::create_simple_int4range` (or equivalent).
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/ir/user_type.rs \
        crates/pgevolve-core/src/diff/types.rs \
        crates/pgevolve-core/src/catalog/queries/types.rs \
        crates/pgevolve-core/src/catalog/assemble/user_types.rs \
        crates/pgevolve-core/src/parse/builder/create_stmt.rs \
        crates/pgevolve-core/src/plan/rewrite/emit/user_type.rs \
        crates/pgevolve-core/src/plan/edges.rs \
        crates/pgevolve-conformance/tests/cases/objects/ranges/create-simple-int4range/
git commit -m "$(cat <<'EOF'
feat: RANGE TYPE — UserTypeKind::Range end-to-end

Add Range variant to UserType, extend reader (LEFT JOIN pg_range),
parser (CREATE TYPE … AS RANGE), differ (ReplaceWithCascade on any
structural change), renderer (CREATE TYPE … AS RANGE (...)), and dep
graph (Range → subtype Type, canonical Function, subtype_diff Function
edges). Multirange handled implicitly — auto-generated multirange
types filtered from pg_type via typtype != 'm'.

One conformance fixture in objects/ranges/create-simple-int4range
covering the minimal subtype-only form.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Collation IR + canon + Catalog::collations

**Files:**
- Create: `crates/pgevolve-core/src/ir/collation.rs`
- Create: `crates/pgevolve-core/src/ir/canon/collations.rs`
- Modify: `crates/pgevolve-core/src/ir/catalog.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs`

### Step 2.1: Create the IR module

- [ ] Write `crates/pgevolve-core/src/ir/collation.rs`:

```rust
//! `CREATE COLLATION` IR — first-class managed collation kind.
//!
//! Source `lc_collate` and `lc_ctype` are always stored separately, even
//! when the user wrote `locale = 'X'` shorthand. The renderer collapses
//! back to `locale = '...'` when the two are equal.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};

/// A user-defined collation managed by pgevolve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collation {
    /// Schema-qualified collation name.
    pub qname: QualifiedName,
    /// libc / icu / PG 17+ builtin.
    pub provider: CollationProvider,
    /// `lc_collate` from `pg_collation.collcollate`.
    pub lc_collate: String,
    /// `lc_ctype` from `pg_collation.collctype`.
    pub lc_ctype: String,
    /// `deterministic` toggle — default `true`. PG 12+, ICU only when false.
    pub deterministic: bool,
    /// Read-only `pg_collation.collversion`. Source declares as `None`;
    /// the differ ignores this field. REFRESH VERSION management deferred
    /// to v0.3.9.
    pub version: Option<String>,
    /// Lenient owner field (per v0.3.1 cross-cutting state pattern).
    pub owner: Option<Identifier>,
    /// `COMMENT ON COLLATION qname IS '...'`.
    pub comment: Option<String>,
}

/// Locale-data provider — controls which OS / library produces the
/// sort + ctype tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollationProvider {
    /// `pg_collation.collprovider = 'c'`.
    Libc,
    /// `pg_collation.collprovider = 'i'`.
    Icu,
    /// `pg_collation.collprovider = 'b'` — PG 17+ only.
    Builtin,
}

/// Collation shortnames that bypass `column-references-unmanaged-collation`
/// even when they have no schema qualifier. PG seeds these at initdb.
pub const BUILTIN_COLLATIONS: &[&str] = &[
    "default", "C", "POSIX", "und-x-icu", "unicode", "ucs_basic",
];

impl CollationProvider {
    /// SQL keyword used in `CREATE COLLATION … (provider = …)`.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::Libc => "libc",
            Self::Icu => "icu",
            Self::Builtin => "builtin",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier { Identifier::from_unquoted(s).unwrap() }
    fn qn(s: &str, n: &str) -> QualifiedName { QualifiedName::new(id(s), id(n)) }

    #[test]
    fn collation_serde_round_trip() {
        let c = Collation {
            qname: qn("app", "case_insensitive"),
            provider: CollationProvider::Icu,
            lc_collate: "und".into(),
            lc_ctype: "und".into(),
            deterministic: false,
            version: None,
            owner: None,
            comment: Some("CI collation".into()),
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Collation = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn provider_sql_keywords() {
        assert_eq!(CollationProvider::Libc.sql_keyword(), "libc");
        assert_eq!(CollationProvider::Icu.sql_keyword(), "icu");
        assert_eq!(CollationProvider::Builtin.sql_keyword(), "builtin");
    }
}
```

- [ ] Add `pub mod collation;` to `crates/pgevolve-core/src/ir/mod.rs` next to the other `pub mod` lines.
- [ ] Verify gate: `cargo test --lib -p pgevolve-core ir::collation` passes.

### Step 2.2: Add `collations` field to Catalog

- [ ] In `crates/pgevolve-core/src/ir/catalog.rs`, add to the `Catalog` struct alongside other Vec fields:

```rust
/// User-defined collations (v0.3.8+).
pub collations: Vec<crate::ir::collation::Collation>,
```

- [ ] Initialize to `Vec::new()` in `Catalog::empty()`.
- [ ] Run: `cargo check -p pgevolve-core` and confirm only "missing field `collations`" errors at deliberate struct-literal sites in tests. Fix them (search for `Catalog {` literal constructors and add `collations: Vec::new(),`).
- [ ] Verify gate: `cargo test --lib -p pgevolve-core` passes.

### Step 2.3: Add canon pass

- [ ] Write `crates/pgevolve-core/src/ir/canon/collations.rs`:

```rust
//! Canon pass for the Catalog's `collations` field.
//!
//! - Sorts by qname for byte-stable comparison.
//! - Defaults `deterministic` to `true` when missing (in practice always
//!   true in the IR; the field is non-optional, so this is a guard).
//! - Rejects `nondeterministic + Libc` combinations with a clear error.

use crate::canon::CanonError;
use crate::ir::catalog::Catalog;
use crate::ir::collation::CollationProvider;

pub fn canonicalize(cat: &mut Catalog) -> Result<(), CanonError> {
    cat.collations.sort_by(|a, b| a.qname.cmp(&b.qname));
    for c in &cat.collations {
        if !c.deterministic && c.provider == CollationProvider::Libc {
            return Err(CanonError::InvalidCollation {
                qname: c.qname.clone(),
                reason: "nondeterministic = false is only valid with provider = icu".into(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::collation::Collation;

    fn id(s: &str) -> Identifier { Identifier::from_unquoted(s).unwrap() }
    fn qn(s: &str, n: &str) -> QualifiedName { QualifiedName::new(id(s), id(n)) }

    fn make_libc(qname: QualifiedName, deterministic: bool) -> Collation {
        Collation {
            qname,
            provider: CollationProvider::Libc,
            lc_collate: "en_US.utf8".into(),
            lc_ctype: "en_US.utf8".into(),
            deterministic,
            version: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn sorts_by_qname() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_libc(qn("app", "z"), true));
        cat.collations.push(make_libc(qn("app", "a"), true));
        canonicalize(&mut cat).unwrap();
        assert_eq!(cat.collations[0].qname.name.as_str(), "a");
    }

    #[test]
    fn rejects_libc_nondeterministic() {
        let mut cat = Catalog::empty();
        cat.collations.push(make_libc(qn("app", "bad"), false));
        let err = canonicalize(&mut cat).unwrap_err();
        assert!(matches!(err, CanonError::InvalidCollation { .. }));
    }
}
```

- [ ] Add `InvalidCollation { qname: QualifiedName, reason: String }` variant to `crate::canon::CanonError` if it doesn't already exist.
- [ ] Register the pass in `crates/pgevolve-core/src/ir/canon/mod.rs` between existing passes (alphabetical or matching the field order on Catalog).
- [ ] Verify gate: `cargo test --lib -p pgevolve-core ir::canon::collations` passes.

### Step 2.4: Stage 2 verify gate + commit

- [ ] Run the full per-stage verify gate.
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/ir/collation.rs \
        crates/pgevolve-core/src/ir/canon/collations.rs \
        crates/pgevolve-core/src/ir/catalog.rs \
        crates/pgevolve-core/src/ir/canon/mod.rs \
        crates/pgevolve-core/src/ir/mod.rs
git commit -m "$(cat <<'EOF'
feat(ir): Collation, CollationProvider, BUILTIN_COLLATIONS

New top-level IR object for CREATE COLLATION (v0.3.8). Source stores
lc_collate + lc_ctype separately; the renderer collapses to the
locale = '...' shorthand when they match. version field is read-only
(differ ignores it; REFRESH VERSION management deferred to v0.3.9).

Adds Catalog::collations: Vec<Collation> and a canon pass that sorts
by qname and rejects the invalid Libc + nondeterministic combination
at canon time (pre-empting PG's runtime rejection with a clearer
error).

BUILTIN_COLLATIONS const lists shortnames that bypass the future
column-references-unmanaged-collation lint.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Collation catalog reader

**Files:**
- Create: `crates/pgevolve-core/src/catalog/queries/collations.rs`
- Create: `crates/pgevolve-core/src/catalog/collations.rs`
- Create: `crates/pgevolve-core/src/catalog/assemble/collations.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/{mod,pg14,pg15,pg16,pg17,pg18}.rs`
- Modify: `crates/pgevolve-core/src/catalog/mod.rs`
- Modify: `crates/pgevolve-core/src/catalog/assemble/mod.rs`

### Step 3.1: Per-PG-version SQL query

- [ ] Write `crates/pgevolve-core/src/catalog/queries/collations.rs`:

```rust
//! SQL queries for reading user-defined collations from pg_collation.
//!
//! Built-in collations (pg_catalog namespace) and extension-owned
//! collations (pg_depend.deptype = 'e') are filtered out — only
//! user-created collations surface.

/// Shared base query — works on PG 14+ unchanged. Per-version files
/// re-export this for now; PG 17+ adds the `builtin` provider but the
/// column shape is identical.
pub const SELECT_COLLATIONS: &str = r#"
SELECT
    n.nspname AS schema,
    c.collname AS name,
    c.collprovider AS provider,
    c.collcollate AS lc_collate,
    c.collctype AS lc_ctype,
    c.collisdeterministic AS deterministic,
    c.collversion AS version,
    pg_catalog.pg_get_userbyid(c.collowner) AS owner,
    pg_catalog.obj_description(c.oid, 'pg_collation') AS comment
FROM pg_catalog.pg_collation c
JOIN pg_catalog.pg_namespace n ON n.oid = c.collnamespace
WHERE n.nspname <> 'pg_catalog'
  AND n.nspname <> 'information_schema'
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend d
      WHERE d.classid = 'pg_catalog.pg_collation'::regclass
        AND d.objid = c.oid
        AND d.deptype = 'e'
  )
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, c.collname
"#;
```

- [ ] Wire into `crates/pgevolve-core/src/catalog/queries/mod.rs`: add a `Collations` variant to `CatalogQuery` (next to `Publications`, `Subscriptions`, `Statistics`).
- [ ] Wire into each per-version file (`pg14.rs` through `pg18.rs`): add a `CatalogQuery::Collations => SELECT_COLLATIONS` arm.
- [ ] Verify: `cargo build -p pgevolve-core` clean.

### Step 3.2: Querier wrapper

- [ ] Write `crates/pgevolve-core/src/catalog/collations.rs`:

```rust
//! Thin wrapper around `CatalogQuerier::fetch(CatalogQuery::Collations)`.

use super::error::CatalogError;
use super::{CatalogQuerier, CatalogQuery, Row};

pub fn fetch_collations(
    querier: &dyn CatalogQuerier,
    managed_schemas: &[&str],
) -> Result<Vec<Row>, CatalogError> {
    querier.fetch(CatalogQuery::Collations, managed_schemas)
}
```

- [ ] Add to `catalog::CatalogQuery::takes_text_array_param`: `CatalogQuery::Collations => true`.
- [ ] Register in `crates/pgevolve-core/src/catalog/mod.rs` (mirroring `pub(crate) mod publications;`).

### Step 3.3: Assembler

- [ ] Write `crates/pgevolve-core/src/catalog/assemble/collations.rs`:

```rust
//! Build Vec<Collation> from `pg_collation` rows.

use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::collation::{Collation, CollationProvider};

pub(super) fn build_collations(rows: &[Row]) -> Result<Vec<Collation>, CatalogError> {
    rows.iter().map(build_one).collect()
}

fn build_one(row: &Row) -> Result<Collation, CatalogError> {
    let schema = row.text("schema")?;
    let name = row.text("name")?;
    let qname = QualifiedName::new(
        Identifier::from_unquoted(schema).map_err(|e| CatalogError::BadIdentifier {
            value: schema.to_string(), reason: e.to_string(),
        })?,
        Identifier::from_unquoted(name).map_err(|e| CatalogError::BadIdentifier {
            value: name.to_string(), reason: e.to_string(),
        })?,
    );
    let provider_char = row.char("provider")?;
    let provider = match provider_char {
        'c' => CollationProvider::Libc,
        'i' => CollationProvider::Icu,
        'b' => CollationProvider::Builtin,
        other => return Err(CatalogError::UnexpectedValue {
            column: "provider".into(),
            value: other.to_string(),
        }),
    };
    let owner = row.opt_text("owner")?.map(|s| Identifier::from_unquoted(s).unwrap());
    Ok(Collation {
        qname,
        provider,
        lc_collate: row.text("lc_collate")?.to_string(),
        lc_ctype: row.text("lc_ctype")?.to_string(),
        deterministic: row.bool("deterministic")?,
        version: row.opt_text("version")?.map(str::to_string),
        owner,
        comment: row.opt_text("comment")?.map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    // Row-decoding tests using crate::catalog::rows::test_helpers
}
```

- [ ] Register in `crates/pgevolve-core/src/catalog/assemble/mod.rs`: `pub(crate) mod collations;` and add a call site in `read_catalog`'s body to populate `cat.collations`.
- [ ] Verify gate: `cargo test --lib -p pgevolve-core catalog::assemble::collations` passes.

### Step 3.4: Tier-3 round-trip fixture stub

- [ ] Add a fixture under `crates/pgevolve-core/tests/fixtures/round_trip/collation_basic.sql` containing:

```sql
CREATE COLLATION app.case_insensitive (provider = icu, locale = 'und', deterministic = false);
```

The existing `dump_round_trip` harness picks it up. Will fail to bless until reader + parser + renderer are all wired; that's expected at this stage — leave it un-blessed and verify it'll bless after Stage 6.

### Step 3.5: Stage 3 verify gate + commit

- [ ] Run full verify gate (note: tier-3 round-trip test for `collation_basic.sql` will fail; expected).
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/catalog/collations.rs \
        crates/pgevolve-core/src/catalog/queries/collations.rs \
        crates/pgevolve-core/src/catalog/queries/{mod,pg14,pg15,pg16,pg17,pg18}.rs \
        crates/pgevolve-core/src/catalog/mod.rs \
        crates/pgevolve-core/src/catalog/assemble/collations.rs \
        crates/pgevolve-core/src/catalog/assemble/mod.rs
git commit -m "feat(catalog): read user-defined collations from pg_collation

Adds the Collations query (shared SQL works on PG 14+ unchanged),
querier wrapper, and assembler. Built-in and extension-owned collations
filtered at query time via namespace + pg_depend checks.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 4 — Collation source parser

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/create_collation_stmt.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs`
- Modify: `crates/pgevolve-core/src/parse/statement.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/comment_stmt.rs`

### Step 4.1: Parser module

- [ ] Write `crates/pgevolve-core/src/parse/builder/create_collation_stmt.rs`. Skeleton:

```rust
//! `CREATE COLLATION qname (option = value, …)` parser.
//!
//! Source may use the `locale = 'X'` shorthand; the IR always stores
//! `lc_collate` + `lc_ctype` separately.

use pg_query::protobuf::{DefineStmt, Node, node::Node as NodeEnum};

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::collation::{Collation, CollationProvider};
use crate::parse::error::{ParseError, SourceLocation};

pub(crate) fn apply(
    stmt: &DefineStmt,
    cat: &mut Catalog,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(), ParseError> {
    let qname = super::shared::qname_from_define(stmt, default_schema, location)?;
    let mut provider: Option<CollationProvider> = None;
    let mut locale: Option<String> = None;
    let mut lc_collate: Option<String> = None;
    let mut lc_ctype: Option<String> = None;
    let mut deterministic: Option<bool> = None;

    for def in &stmt.definition {
        let Some(NodeEnum::DefElem(de)) = def.node.as_ref() else {
            return Err(ParseError::Structural {
                message: "CREATE COLLATION: unexpected definition node".into(),
                location: location.clone(),
            });
        };
        match de.defname.as_str() {
            "provider" => provider = Some(parse_provider(de, location)?),
            "locale" => locale = Some(parse_string(de, "locale", location)?),
            "lc_collate" => lc_collate = Some(parse_string(de, "lc_collate", location)?),
            "lc_ctype" => lc_ctype = Some(parse_string(de, "lc_ctype", location)?),
            "deterministic" => deterministic = Some(parse_bool(de, "deterministic", location)?),
            other => return Err(ParseError::Structural {
                message: format!("CREATE COLLATION: unknown option `{other}`"),
                location: location.clone(),
            }),
        }
    }

    // Normalize shorthand: locale → lc_collate + lc_ctype.
    let (lc_collate, lc_ctype) = match (lc_collate, lc_ctype, locale) {
        (Some(c), Some(t), None) => (c, t),
        (None, None, Some(loc)) => (loc.clone(), loc),
        (None, None, None) => return Err(ParseError::Structural {
            message: format!("CREATE COLLATION {qname}: missing locale / lc_collate / lc_ctype"),
            location: location.clone(),
        }),
        _ => return Err(ParseError::Structural {
            message: format!("CREATE COLLATION {qname}: locale must not coexist with lc_collate / lc_ctype"),
            location: location.clone(),
        }),
    };

    cat.collations.push(Collation {
        qname,
        provider: provider.unwrap_or(CollationProvider::Libc),
        lc_collate,
        lc_ctype,
        deterministic: deterministic.unwrap_or(true),
        version: None,
        owner: None,
        comment: None,
    });
    Ok(())
}

// helpers: parse_provider, parse_string, parse_bool …

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_to_catalog;

    #[test]
    fn parse_libc_collation() {
        let cat = parse_to_catalog(
            "CREATE COLLATION app.de_DE (provider = libc, locale = 'de_DE.utf8');",
        ).unwrap();
        let c = &cat.collations[0];
        assert_eq!(c.provider, CollationProvider::Libc);
        assert_eq!(c.lc_collate, "de_DE.utf8");
        assert_eq!(c.lc_ctype, "de_DE.utf8");
        assert!(c.deterministic);
    }

    #[test]
    fn parse_icu_nondeterministic() {
        let cat = parse_to_catalog(
            "CREATE COLLATION app.ci (provider = icu, locale = 'und', deterministic = false);",
        ).unwrap();
        let c = &cat.collations[0];
        assert_eq!(c.provider, CollationProvider::Icu);
        assert!(!c.deterministic);
    }

    #[test]
    fn parse_separate_lc_fields() {
        let cat = parse_to_catalog(
            "CREATE COLLATION app.x (provider = libc, lc_collate = 'C', lc_ctype = 'en_US.utf8');",
        ).unwrap();
        let c = &cat.collations[0];
        assert_eq!(c.lc_collate, "C");
        assert_eq!(c.lc_ctype, "en_US.utf8");
    }

    #[test]
    fn parse_rejects_unknown_option() {
        let err = parse_to_catalog(
            "CREATE COLLATION app.x (locale = 'C', bogus = 1);",
        ).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn parse_rejects_locale_with_lc_fields() {
        let err = parse_to_catalog(
            "CREATE COLLATION app.x (locale = 'C', lc_collate = 'C');",
        ).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }
}
```

- [ ] Wire into `parse/builder/mod.rs` (`pub(crate) mod create_collation_stmt;`).
- [ ] In `parse/statement.rs::Statement::classify`, add `NodeEnum::DefineStmt(s)` branch (or extend existing one) that dispatches to `create_collation_stmt::apply` when `s.kind == ObjectType::ObjectCollation`.
- [ ] Verify gate: `cargo test --lib -p pgevolve-core parse::builder::create_collation_stmt` passes.

### Step 4.2: COMMENT ON COLLATION

- [ ] In `parse/builder/comment_stmt.rs::apply_comment_inner`, add an `ObjectType::ObjectCollation` arm that sets `cat.collations.iter_mut().find(|c| c.qname == qname).map(|c| c.comment = Some(comment));`.
- [ ] Add unit test:

```rust
#[test]
fn comment_on_collation_attaches_to_ir() {
    let cat = parse_to_catalog(
        "CREATE COLLATION app.x (provider = libc, locale = 'C');
         COMMENT ON COLLATION app.x IS 'pinned for sorting';"
    ).unwrap();
    assert_eq!(cat.collations[0].comment.as_deref(), Some("pinned for sorting"));
}
```

### Step 4.3: Stage 4 verify gate + commit

- [ ] Verify gate.
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/parse/
git commit -m "feat(parse): CREATE COLLATION + COMMENT ON COLLATION

Parser accepts both locale = 'X' shorthand (normalized to lc_collate
+ lc_ctype in IR) and explicit lc_collate / lc_ctype. Provider defaults
to libc; deterministic defaults to true. Unknown options rejected with
a clear error naming the bad key. Locale combined with lc_collate /
lc_ctype rejected.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 5 — Collation differ + `CollationChange` sub-enum

**Files:**
- Create: `crates/pgevolve-core/src/diff/collations.rs`
- Modify: `crates/pgevolve-core/src/diff/change.rs`
- Modify: `crates/pgevolve-core/src/diff/mod.rs`

### Step 5.1: Define `CollationChange` sub-enum

- [ ] In `crates/pgevolve-core/src/diff/change.rs`, append to the existing sub-enum section (after `TableChange`):

```rust
/// A structural change to a single collation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CollationChange {
    /// `CREATE COLLATION ...`
    Create(crate::ir::collation::Collation),
    /// `DROP COLLATION qname` — destructive.
    Drop {
        /// Schema-qualified collation name.
        qname: QualifiedName,
    },
    /// `ALTER COLLATION qname RENAME TO new_name`.
    Rename {
        /// Existing qname.
        from: QualifiedName,
        /// New unqualified name (same schema).
        to: Identifier,
    },
    /// `DROP COLLATION old; CREATE COLLATION new;` — PG has no in-place
    /// ALTER for provider / locale / deterministic.
    Replace {
        /// The collation as it exists in the target.
        from: crate::ir::collation::Collation,
        /// The collation as it should exist in the source.
        to: crate::ir::collation::Collation,
    },
    /// `COMMENT ON COLLATION qname IS '...'`.
    CommentOn {
        /// Schema-qualified collation name.
        qname: QualifiedName,
        /// New comment (`None` clears).
        comment: Option<String>,
    },
}
```

- [ ] Add `Collation(CollationChange)` variant to the master `Change` enum next to `Statistic`/`Subscription`/`Publication`.
- [ ] Verify: `cargo check -p pgevolve-core` reports only "non-exhaustive match" errors at expected sites (will fix in later stages).

### Step 5.2: Differ implementation

- [ ] Write `crates/pgevolve-core/src/diff/collations.rs`:

```rust
//! Differ for collations. Pair by qname; per-collation granular diff.

use std::collections::BTreeMap;

use crate::diff::change::{Change, CollationChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, OwnedObjectId, OwnerObjectKind};
use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::collation::Collation;

pub fn diff_collations(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&QualifiedName, &Collation> =
        target.collations.iter().map(|c| (&c.qname, c)).collect();
    let source_map: BTreeMap<&QualifiedName, &Collation> =
        source.collations.iter().map(|c| (&c.qname, c)).collect();

    // Creates: source-only.
    for (qname, src) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::Collation(CollationChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only: lenient — no auto-drop. Surfaces via unmanaged-collation lint.

    // Modifies: in both.
    for (qname, src) in &source_map {
        let Some(tgt) = target_map.get(qname) else { continue; };
        diff_one(tgt, src, out);
    }
}

fn diff_one(target: &Collation, source: &Collation, out: &mut ChangeSet) {
    // Structural change (provider / lc_collate / lc_ctype / deterministic)
    // → Replace. Ignore `version` (read-only).
    if (target.provider, &target.lc_collate, &target.lc_ctype, target.deterministic)
        != (source.provider, &source.lc_collate, &source.lc_ctype, source.deterministic)
    {
        out.push(
            Change::Collation(CollationChange::Replace {
                from: target.clone(),
                to: source.clone(),
            }),
            Destructiveness::RequiresApproval {
                reason: format!("collation {} structural change", source.qname),
            },
        );
        return;
    }

    // Owner: lenient.
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Collation,
                id: OwnedObjectId::Qualified(source.qname.clone()),
                signature: String::new(),
                from: target.owner.clone(),
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment.
    if target.comment != source.comment {
        out.push(
            Change::Collation(CollationChange::CommentOn {
                qname: source.qname.clone(),
                comment: source.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    // Standard create / replace / comment-on / owner / partial-overlap tests.
}
```

- [ ] Extend `OwnerObjectKind` in `diff/owner_op.rs` with a `Collation` variant and `sql_keyword` returning `"COLLATION"`.
- [ ] Wire `diff_collations` into `diff::mod::diff` next to `diff_statistics` / `diff_publications`.
- [ ] Verify gate: `cargo test --lib -p pgevolve-core diff::collations` passes.

### Step 5.3: Stage 5 verify gate + commit

- [ ] Verify gate.
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/diff/
git commit -m "feat(diff): collations — CollationChange sub-enum + diff

5 nested variants: Create, Drop, Rename, Replace, CommentOn. Structural
changes (provider / lc_collate / lc_ctype / deterministic) emit
Replace; version field ignored (REFRESH VERSION deferred to v0.3.9).
Owner changes use the standard Change::AlterObjectOwner path with
OwnedObjectId::Qualified — adds OwnerObjectKind::Collation variant.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 6 — Collation render + 5 StepKinds + emit dispatch

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/collations.rs`
- Modify: `crates/pgevolve-core/src/plan/raw_step.rs`
- Modify: `crates/pgevolve-core/src/plan/plan.rs`
- Modify: `crates/pgevolve-core/src/plan/ordering.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

### Step 6.1: StepKinds + kind_name round-trip

- [ ] In `crates/pgevolve-core/src/plan/raw_step.rs`, add 5 variants to `StepKind` (next to `CommentOnStatistic` and friends):

```rust
/// `CREATE COLLATION qname (...)`.
CreateCollation,
/// `DROP COLLATION qname` — destructive.
DropCollation,
/// `ALTER COLLATION qname RENAME TO new_name`.
RenameCollation,
/// `DROP COLLATION old; CREATE COLLATION new;` — structural change.
ReplaceCollation,
/// `COMMENT ON COLLATION qname IS '...'`.
CommentOnCollation,
```

- [ ] Add them to the round-trip list at the bottom of the file.
- [ ] In `crates/pgevolve-core/src/plan/plan.rs::kind_name`, add:

```rust
K::CreateCollation => "create_collation",
K::DropCollation => "drop_collation",
K::RenameCollation => "rename_collation",
K::ReplaceCollation => "replace_collation",
K::CommentOnCollation => "comment_on_collation",
```

- [ ] And mirror in `parse_kind_name`.

### Step 6.2: SQL renderer

- [ ] Write `crates/pgevolve-core/src/plan/rewrite/collations.rs`:

```rust
//! SQL emitters for collation StepKinds.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::collation::{Collation, CollationProvider};

#[must_use]
pub fn create_collation(c: &Collation) -> String {
    let mut opts: Vec<String> = Vec::with_capacity(4);
    opts.push(format!("provider = {}", c.provider.sql_keyword()));
    // Collapse to locale = '…' when lc_collate == lc_ctype.
    if c.lc_collate == c.lc_ctype {
        opts.push(format!("locale = '{}'", escape_sql_str(&c.lc_collate)));
    } else {
        opts.push(format!("lc_collate = '{}'", escape_sql_str(&c.lc_collate)));
        opts.push(format!("lc_ctype = '{}'", escape_sql_str(&c.lc_ctype)));
    }
    if !c.deterministic {
        opts.push("deterministic = false".into());
    }
    format!(
        "CREATE COLLATION {} ({});",
        c.qname.render_sql(),
        opts.join(", "),
    )
}

#[must_use]
pub fn drop_collation(qname: &QualifiedName) -> String {
    format!("DROP COLLATION {};", qname.render_sql())
}

#[must_use]
pub fn rename_collation(from: &QualifiedName, to: &Identifier) -> String {
    format!(
        "ALTER COLLATION {} RENAME TO {};",
        from.render_sql(),
        to.render_sql(),
    )
}

#[must_use]
pub fn comment_on_collation(qname: &QualifiedName, comment: Option<&str>) -> String {
    match comment {
        Some(s) => format!("COMMENT ON COLLATION {} IS '{}';", qname.render_sql(), escape_sql_str(s)),
        None => format!("COMMENT ON COLLATION {} IS NULL;", qname.render_sql()),
    }
}

fn escape_sql_str(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    // Render tests for each emitter including locale-collapse edge case.
}
```

### Step 6.3: `NodeId::Collation` (node, not edges)

- [ ] In `crates/pgevolve-core/src/plan/edges.rs`, add `Collation(QualifiedName)` to the `NodeId` enum. Stage 7 wires the actual *edges*; this step only adds the variant so `ordering.rs::change_node` (next step) can reference it without a compile error.

### Step 6.4: Ordering + emit dispatch

- [ ] In `crates/pgevolve-core/src/plan/ordering.rs::partition`:

```rust
Change::Collation(CollationChange::Create(_)) => creates.push(entry),
Change::Collation(
    CollationChange::Drop { .. } | CollationChange::Replace { .. }
) => drops.push(entry),
Change::Collation(
    CollationChange::Rename { .. } | CollationChange::CommentOn { .. }
) => modifies.push(entry),
```

- [ ] In `plan/ordering.rs::change_node`, dispatch each `CollationChange` arm to `NodeId::Collation(qname)`.
- [ ] In `crates/pgevolve-core/src/plan/rewrite/mod.rs::emit_change`, add a `Change::Collation(cc)` arm that matches on the inner variant and pushes the appropriate `RawStep` using the emitters from `plan::rewrite::collations`. Mirror the nested match idiom used by `Change::View(vc) => match vc { … }`.

### Step 6.5: Stage 6 verify gate + commit

- [ ] Tier-3 round-trip from Stage 3 (`collation_basic.sql`) should now bless cleanly: `cargo xtask bless` (needs Docker). Commit the generated snapshot.
- [ ] Verify gate.
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/plan/ \
        crates/pgevolve-core/tests/fixtures/round_trip/
git commit -m "feat(plan): collation render + 5 StepKinds + NodeId variant + ordering wired

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 7 — Dep edges: `Column → Collation` + Domain/Range/Composite edges

(`NodeId::Collation` variant landed in Stage 6.3 so `change_node` could compile. This stage adds the actual graph *edges*.)

**Files:**
- Modify: `crates/pgevolve-core/src/plan/edges.rs`

### Step 7.1: Add nodes + edges in build_create_graph

- [ ] After existing user-type node additions, loop over `catalog.collations` and add each as `NodeId::Collation(c.qname.clone())`.
- [ ] After existing column-edge loops, for each column with `collation: Some(qn)`:
  - If `qn.schema` is empty or `qn.schema == "pg_catalog"` or `qn.name.as_str()` is in `BUILTIN_COLLATIONS` → skip.
  - Else if a managed collation exists with that qname → add edge `NodeId::Collation(qn) → NodeId::Table(column.table)`.

- [ ] Same for `Domain.collation`, `Range.collation`, and `CompositeAttribute.collation`.

- [ ] Mirror in `build_drop_graph` with reversed edges.

### Step 7.2: Tests

- [ ] Add `plan/edges.rs` tests:

```rust
#[test]
fn column_with_managed_collation_adds_edge() {
    let mut cat = Catalog::empty();
    cat.schemas.push(Schema::new(id("app")));
    cat.collations.push(make_collation("app", "ci"));
    let mut t = make_table("app", "users");
    t.columns.push(make_column_with_collation("email", "text", qn("app", "ci")));
    cat.tables.push(t);
    let g = build_create_graph(&cat);
    assert!(g.edges().any(|(from, to)|
        *from == NodeId::Collation(qn("app", "ci")) &&
        *to == NodeId::Table(qn("app", "users"))
    ));
}

#[test]
fn column_with_pg_catalog_collation_no_edge() {
    let mut cat = Catalog::empty();
    cat.schemas.push(Schema::new(id("app")));
    let mut t = make_table("app", "users");
    t.columns.push(make_column_with_collation("email", "text", qn("pg_catalog", "C")));
    cat.tables.push(t);
    let g = build_create_graph(&cat);
    assert!(!g.edges().any(|(from, _)| matches!(from, NodeId::Collation(_))));
}
```

### Step 7.3: Stage 7 verify gate + commit

- [ ] Verify gate.
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/plan/edges.rs
git commit -m "feat(plan): NodeId::Collation + Column/Domain/Range → Collation edges

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 8 — Lint rules (5 new)

**Files:**
- Create: `crates/pgevolve-core/src/lint/rules/unmanaged_collation.rs`
- Create: `crates/pgevolve-core/src/lint/rules/column_references_unmanaged_collation.rs`
- Create: `crates/pgevolve-core/src/lint/rules/range_type_references_unmanaged_subtype.rs`
- Create: `crates/pgevolve-core/src/lint/rules/nondeterministic_collation_requires_pg_12.rs`
- Create: `crates/pgevolve-core/src/lint/rules/builtin_provider_requires_pg_17.rs`
- Modify: `crates/pgevolve-core/src/lint/rules/mod.rs`
- Modify: `crates/pgevolve-core/src/lint/universal.rs`

### Step 8.1: `unmanaged-collation`

- [ ] Write the rule as a 10-line wrapper over the shared `check_unmanaged_objects` helper, following the exact pattern in `lint/rules/unmanaged_statistic.rs`:

```rust
//! Warns when the catalog has a collation not declared in source.

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "unmanaged-collation";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    super::check_unmanaged_objects(
        &target.collations,
        &source.collations,
        |c| &c.qname,
        RULE_ID,
        "collation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::collation::{Collation, CollationProvider};
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::{id, qn};

    fn make(qname: crate::identifier::QualifiedName) -> Collation {
        Collation {
            qname, provider: CollationProvider::Libc,
            lc_collate: "C".into(), lc_ctype: "C".into(),
            deterministic: true, version: None, owner: None, comment: None,
        }
    }

    #[test]
    fn target_only_fires() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target.collations.push(make(qn("app", "drift")));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("app.drift"));
    }

    #[test]
    fn matching_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.collations.push(make(qn("app", "managed")));
        target.collations.push(make(qn("app", "managed")));
        assert!(check(&source, &target).is_empty());
    }
}
```

### Step 8.2: `column-references-unmanaged-collation`

- [ ] Write the rule. Iterate every column / domain / range / composite-attribute with a `collation` Some(qname). If the qname is *not* in `BUILTIN_COLLATIONS` (when no schema) and *not* a managed collation in source → emit a Warning Finding pointing at the referencing object.

### Step 8.3: `range-type-references-unmanaged-subtype`

- [ ] Similar shape — iterate `source.user_types` filtering for the `Range` variant, check `subtype` qname against a hardcoded list of safe PG built-in scalar types (`int2`, `int4`, `int8`, `numeric`, `text`, `varchar`, `bpchar`, `date`, `timestamp`, `timestamptz`, `time`, `timetz`, `interval`, `inet`, `cidr`) and source user-types. Otherwise Warning.

### Step 8.4: PG-version-gated rules

- [ ] `nondeterministic_collation_requires_pg_12.rs`: iterate source collations; if `deterministic == false` and `min_pg_version < 12` → Error finding.
- [ ] `builtin_provider_requires_pg_17.rs`: iterate source collations; if `provider == Builtin` and `min_pg_version < 17` → Error finding.

Both follow the exact pattern in `lint/rules/publication_feature_requires_pg_version.rs` for plan-time gates.

### Step 8.5: Register rules

- [ ] Add 5 `pub mod` lines to `lint/rules/mod.rs`.
- [ ] In `lint/universal.rs`:
  - Add `unmanaged_collation::check`, `column_references_unmanaged_collation::check`, `range_type_references_unmanaged_subtype::check` to `run_drift_lints` (which receives both source + target).
  - Add `nondeterministic_collation_requires_pg_12::check`, `builtin_provider_requires_pg_17::check` to `check_plan_time_catalog` (which receives `min_pg_version`).

- [ ] Update the module-level doc comment in `lint/universal.rs` listing the new rule IDs.

### Step 8.6: Stage 8 verify gate + commit

- [ ] Verify gate.
- [ ] Commit:

```bash
git add crates/pgevolve-core/src/lint/
git commit -m "feat(lint): 5 new collation + range rules

unmanaged-collation (Warning), column-references-unmanaged-collation
(Warning), range-type-references-unmanaged-subtype (Warning),
nondeterministic-collation-requires-pg-12 (Error, plan-time gate),
builtin-provider-requires-pg-17 (Error, plan-time gate).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 9 — Conformance fixtures (11 total: 6 collations + 4 ranges + 1 scenario)

Stage 1 already shipped `objects/ranges/create-simple-int4range`. Adds the rest.

**Files (create):**
- `crates/pgevolve-conformance/tests/cases/objects/collations/create-libc/{before.sql,after.sql,fixture.toml,expected/plan.sql,expected/dep-graph.dot}`
- `…/create-icu/…` (PG 15+ gated)
- `…/create-nondeterministic/…` (PG 12+ gated)
- `…/drop/…`
- `…/comment-on/…`
- `…/rename/…`
- `objects/ranges/create-with-opclass/…`
- `objects/ranges/create-with-canonical-fn/…`
- `objects/ranges/drop/…`
- `objects/ranges/column-with-range-type/…`
- `scenarios/column-references-managed-collation/…`

### Step 9.1: Per-fixture procedure

For each fixture:
1. Write `before.sql` (current state — usually `CREATE SCHEMA app;` or richer).
2. Write `after.sql` (desired state).
3. Write `fixture.toml` with `[meta]`, `[pg]` (majors list), `[expect.plan]`.
4. Run `cargo xtask bless <path>` (needs Docker) — generates `expected/plan.sql` + `expected/dep-graph.dot`.
5. Commit the directory.

Example `fixture.toml` for `objects/collations/create-icu` (PG 15+):

```toml
[meta]
description = "CREATE COLLATION with provider = icu — PG 15+"

[pg]
majors = [15, 16, 17, 18]

[expect.plan]
order = []
touches_only = ["app", "app.und_co"]
```

For `objects/collations/drop` (destructive):

```toml
[meta]
description = "DROP COLLATION — destructive, intent required"

[pg]
majors = [14, 15, 16, 17, 18]

[expect.plan]
order = []

[[expect.intent]]
rule = "drop-collation"
matches.qname = "app.legacy"
```

For `scenarios/column-references-managed-collation`:
- `before.sql`: only `CREATE SCHEMA app;`
- `after.sql`: `CREATE COLLATION app.ci (...); CREATE TABLE app.users (email text COLLATE app.ci);`
- `fixture.toml` asserts via `expect.plan.order` that the `create_collation` step precedes the `create_table` step.

### Step 9.2: Stage 9 verify gate + commit

- [ ] All 11 fixtures pass: `cargo test --release -p pgevolve-conformance` (needs Docker; ~10 min on the 5-PG matrix).
- [ ] Commit each fixture in its own commit OR one bundled commit:

```bash
git add crates/pgevolve-conformance/tests/cases/objects/collations/ \
        crates/pgevolve-conformance/tests/cases/objects/ranges/ \
        crates/pgevolve-conformance/tests/cases/scenarios/column-references-managed-collation/
git commit -m "test(conformance): 6 collation + 4 range + 1 scenario fixtures

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 10 — Property tests + docs

**Files:**
- Create: `crates/pgevolve-testkit/src/ir_generator/collation.rs`
- Modify: `crates/pgevolve-testkit/src/ir_generator/mod.rs`
- Modify: `crates/pgevolve-testkit/src/ir_generator/sequence.rs` (or new user_type.rs — see Section 6 of spec)
- Create: `docs/spec/collations.md`
- Modify: `docs/spec/objects.md`
- Modify: `docs/spec/roadmap.md`
- Modify: `docs/spec/README.md`

### Step 10.1: `arb_collation`

- [ ] Write `crates/pgevolve-testkit/src/ir_generator/collation.rs`:

```rust
//! Proptest strategy for the `Collation` IR.
//!
//! Locale strings drawn from a hand-curated safe list so generated
//! catalogs always apply cleanly on PG 14-18. Provider + deterministic
//! combination respects the canon-pass rejection of libc + false.

use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::collation::{Collation, CollationProvider};

const SAFE_LIBC_LOCALES: &[&str] = &["C", "POSIX", "en_US.utf8"];
const SAFE_ICU_LOCALES: &[&str] = &["und", "en-US", "de-DE"];

pub fn arb_collation(schema_name: Identifier) -> impl Strategy<Value = Collation> {
    (
        "[a-z][a-z0-9_]{2,15}",   // name
        prop_oneof![Just(CollationProvider::Libc), Just(CollationProvider::Icu)],
        any::<bool>(),             // deterministic
    )
        .prop_flat_map(move |(name, provider, det)| {
            let schema = schema_name.clone();
            let safe_locales = match provider {
                CollationProvider::Libc => SAFE_LIBC_LOCALES,
                CollationProvider::Icu => SAFE_ICU_LOCALES,
                CollationProvider::Builtin => SAFE_ICU_LOCALES, // unreached here
            };
            proptest::sample::select(safe_locales).prop_map(move |loc| {
                // libc + nondeterministic is rejected by canon — force true.
                let deterministic = matches!(provider, CollationProvider::Icu) && det;
                Collation {
                    qname: QualifiedName::new(
                        schema.clone(),
                        Identifier::from_unquoted(&name).unwrap(),
                    ),
                    provider,
                    lc_collate: loc.to_string(),
                    lc_ctype: loc.to_string(),
                    deterministic: if matches!(provider, CollationProvider::Libc) {
                        true
                    } else {
                        deterministic
                    },
                    version: None,
                    owner: None,
                    comment: None,
                }
            })
        })
}
```

- [ ] In `crates/pgevolve-testkit/src/ir_generator/mod.rs`, wire `arb_collation` into `arbitrary_catalog`: generate 0-2 collations per managed schema, feed them into `cat.collations`. Make sure column-generation can reference one of the generated collations when appropriate (judge: probably only on `text` columns, probability 0.1).

### Step 10.2: `arb_range_type_kind`

- [ ] Add `arb_range_type_kind()` in `ir_generator/mod.rs` (or pull all user-type strategies into a new `user_type.rs`). Subtype drawn from a safe list: `int4`, `int8`, `numeric`, `text`, `timestamptz`. Other fields default to `None` for v0.3.8.
- [ ] Wire so that ~10% of generated `UserType`s are `Range` kind.

### Step 10.3: Docs

- [ ] Create `docs/spec/collations.md` modeled on `docs/spec/publications.md` — capability table, lint references, fixture pointers.
- [ ] Update `docs/spec/objects.md`: flip `CREATE COLLATION` and `RANGE TYPE` from `📋 v0.3.8` to `✅ Implemented (v0.3.8)`.
- [ ] Update `docs/spec/roadmap.md`: move both rows from the "Active matrix" to the "Shipped" section.
- [ ] Update `docs/spec/README.md` naming-conventions paragraph to extend the "v0.3.6–v0.3.7" entry to "v0.3.6–v0.3.8".

### Step 10.4: Stage 10 verify gate + commit

- [ ] Verify gate **including** 10× proptest soak: `cargo test --release -p pgevolve-testkit -p pgevolve --release -- --include-ignored property_tests` runs at least 10× cleanly.
- [ ] Commit in 2 chunks — testkit and docs separately, or one combined:

```bash
git add crates/pgevolve-testkit/ docs/spec/
git commit -m "test(proptest)+docs: arb_collation, arb_range_type_kind; spec docs

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

---

## Stage 11 — v0.3.8 release

**Files:**
- Modify: `CHANGELOG.md` — new `[0.3.8] — 2026-MM-DD` entry
- Modify: `Cargo.toml` (workspace) — `version = "0.3.7"` → `"0.3.8"`
- Modify: `crates/pgevolve/Cargo.toml` — `pgevolve-core` constraint `0.3.7` → `0.3.8`
- Modify: `README.md` — bump current release + fixture count

### Step 11.1: Pre-release verify gate

- [ ] Run full gate locally (fmt / clippy / lib tests / cargo doc -D warnings / conformance suite / tier-3 re-bless if needed).
- [ ] Run proptest soak 10× clean (per CLAUDE.md §9).

### Step 11.2: CHANGELOG entry

- [ ] Append to `CHANGELOG.md` immediately after the existing `[0.3.7]` block, using the established format:

```markdown
## [0.3.8] — 2026-MM-DD

### Added
- `CREATE COLLATION` as a first-class IR object: libc / ICU / PG 17+ builtin
  providers, deterministic toggle, COMMENT, RENAME. Source uses
  `locale = 'X'` shorthand or explicit `lc_collate` + `lc_ctype`;
  IR always stores the latter. `version` field is read-only;
  `ALTER COLLATION … REFRESH VERSION` deferred to v0.3.9.
- `CREATE TYPE … AS RANGE` — additive `UserTypeKind::Range` variant.
  Subtype, opclass, collation, canonical, subtype_diff, multirange name.
  Structural changes go through the existing `ReplaceWithCascade` path
  (PG has no in-place ALTER for these fields).
- 5 new lint rules: `unmanaged-collation`,
  `column-references-unmanaged-collation`,
  `range-type-references-unmanaged-subtype`,
  `nondeterministic-collation-requires-pg-12`,
  `builtin-provider-requires-pg-17`.
- 5 new `StepKind` variants for collations
  (`create_collation` / `drop_collation` / `rename_collation` /
  `replace_collation` / `comment_on_collation`).
- 11 conformance fixtures.

### Out of scope (deferred to v0.3.9+)
- `CREATE COLLATION FROM existing_collation`.
- `ALTER COLLATION … REFRESH VERSION` and `collation-version-drift` lint.
- Multirange-type customization beyond `multirange_type_name`.
- Explicit multirange IR object.
```

### Step 11.3: Version bumps

- [ ] In root `Cargo.toml`, bump `workspace.package.version` from `"0.3.7"` to `"0.3.8"`.
- [ ] In `crates/pgevolve/Cargo.toml`, bump the `pgevolve-core` version constraint from `"0.3.7"` to `"0.3.8"`.
- [ ] Run `cargo build --workspace` to refresh `Cargo.lock`.

### Step 11.4: README bump

- [ ] Edit `README.md`:
  - `Current release: **v0.3.7**` → `Current release: **v0.3.8**`
  - Update fixture count `~200` → `~210` (or actual after `find … fixture.toml | wc -l`).

### Step 11.5: Release commit + tag

- [ ] Commit:

```bash
git add CHANGELOG.md Cargo.toml Cargo.lock crates/pgevolve/Cargo.toml README.md
git commit -m "release: v0.3.8 — CREATE COLLATION + RANGE TYPE

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
"
```

- [ ] User-handled release ceremony (per CLAUDE.md §9):

```bash
git push origin main
git tag -s v0.3.8 -m "pgevolve v0.3.8 — CREATE COLLATION + RANGE TYPE

[summary from CHANGELOG]"
git verify-tag v0.3.8
git push origin v0.3.8
cargo publish -p pgevolve-core
# wait ~30s for index sync
cargo publish -p pgevolve
```

Monitor the push CI run; confirm green across all 5 PG majors.

---

## Done.

Final acceptance:
- ✅ Both features land in conformance with ≥10 fixtures.
- ✅ 5 new lint rules registered + tested.
- ✅ Property tests cover both new arms; 10× soak green.
- ✅ CHANGELOG + roadmap + spec catalogue all reflect shipped state.
- ✅ v0.3.8 tag signed, pushed, published.
- ✅ Push CI green across PG 14/15/16/17/18.
