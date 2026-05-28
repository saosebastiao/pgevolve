---
status: design
target_version: v0.3.8
sub_specs: [create-collation, range-type]
---

# `CREATE COLLATION` + `RANGE TYPE` — design (v0.3.8)

Bundles two roadmap items into one release, mirroring the v0.3.7
(STATISTICS + VIEW CHECK OPTION) ship pattern. The two features are
independent — range type slots additively into the existing `UserType`
machinery; collation is a new top-level managed kind.

Scope is **conservative**: minimum useful coverage of each feature,
with edge cases (e.g. `CREATE COLLATION FROM existing`, full `REFRESH
VERSION` diff, multirange-type customization beyond the name) explicitly
deferred to v0.3.9+.

---

## 1. Architecture

### `RANGE TYPE` — additive on `UserType`

```
ir::user_type::UserTypeKind::Range { ... }     ← new variant
catalog::assemble::user_types::build_user_types ← extend to read pg_range
diff::types::diff_range                         ← new fn called from diff_user_types
plan::rewrite::emit::user_type::emit            ← extend Create/Drop arms
```

No new top-level `Catalog::*` field. No new `Change::*` sub-enum.
Fully reuses `UserType` machinery. Drop emits `DROP TYPE … CASCADE`;
the existing cascade logic handles multirange and dependents.

### `CREATE COLLATION` — new top-level managed kind

Mirrors the Publication / Subscription / Statistic shape:

```
ir::collation::{Collation, CollationProvider}    ← new module
ir::canon::collations                            ← canon pass
ir::catalog::Catalog::collations: Vec<Collation> ← new field
catalog::collations + assemble::collations       ← new reader + assembler
parse::builder::create_collation_stmt            ← new parser
parse::builder::comment_stmt                     ← extend for COLLATION kind
diff::collations                                 ← new differ
diff::change::CollationChange                    ← new nested sub-enum
plan::raw_step::StepKind::{...}Collation         ← 5 new variants
plan::rewrite::collations                        ← new SQL renderer
plan::edges                                      ← Collation node + edges
lint::rules::unmanaged_collation                 ← uses check_unmanaged_objects helper
lint::rules::column_references_unmanaged_collation
lint::rules::range_type_references_unmanaged_subtype
lint::rules::nondeterministic_collation_requires_pg_12
lint::rules::builtin_provider_requires_pg_17
```

One cross-cutting addition: dep-graph edge `Column → Collation` when a
column's `collation` qname matches a managed collation. Same shape for
`Domain → Collation` and `Range → Collation`. Built-in collations
(`pg_catalog.*`, plus a hardcoded shortname allow-list) are graph leaves
with no incoming edges.

---

## 2. `RANGE TYPE`

### IR

Extend `ir::user_type::UserTypeKind`:

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
}
```

### Parser

`parse::builder::create_stmt::build_user_type` detects
`RangeBoundsClause` in the `CreateStmt` AST, validates the option set
(reject unknown options with a clear error naming the bad key),
constructs the `Range` variant.

`subtype` validation: must resolve to a real type — either a managed
type in source or a known PG built-in (`text`, `int4`, `int8`,
`numeric`, `timestamp`, `timestamptz`, `date`, `interval`, common
others). For non-built-in unmanaged subtypes, defer the check to
canon-phase via the existing `closed-world-references` lint extension
(see lint surface below).

### Catalog reader

Extend the `CREATE TYPE` family query in `catalog::queries::types`
with `LEFT JOIN pg_range ON pg_range.rngtypid = pg_type.oid`. When
`rngtypid IS NOT NULL`, build a `Range` variant; otherwise fall through
to existing enum/domain/composite logic.

Filter out auto-generated multirange types at query time using
`pg_type.typtype != 'm'` (multirange kind). This keeps the multirange
companion implicit — it never surfaces as a separate `UserType`.

The custom `multirange_type_name` is recovered via `pg_range.rngmultitypid
→ pg_type.typname` if the user named the multirange explicitly. When
the multirange name matches PG's auto-generated `<range>_multirange`
pattern, store `None` (round-trip preserves source omission).

### Differ

`diff::types::diff_range` (called from `diff_user_types` when both
sides have a `Range` kind):

- Compare structural fields (`subtype`, `subtype_opclass`, `collation`,
  `canonical`, `subtype_diff`, `multirange_type_name`).
- Any structural difference → `UserTypeChange::ReplaceWithCascade {
  source, catalog }` (PG has no in-place ALTER for these fields).
- `UserTypeChange::SetComment` for comment-only changes.
- Owner via existing `Change::AlterObjectOwner` path
  (`OwnedObjectId::Qualified(qname)`).

### Plan

`UserTypeChange::Create` arm in `emit::user_type::emit` extends to
render `CREATE TYPE … AS RANGE (subtype = …, …)` when the kind is
`Range`. Drop arm needs no change. No new `StepKind` variants for
range type.

### Dep graph

`Range.subtype` adds a `Type → Type` edge if the subtype is managed.
`canonical` / `subtype_diff` add `Type → Function` edges.
`collation` adds a `Type → Collation` edge.

---

## 3. `CREATE COLLATION`

### IR

New module `ir::collation`:

```rust
pub struct Collation {
    pub qname: QualifiedName,
    pub provider: CollationProvider,
    /// `lc_collate`. Stored separately even when source used
    /// `locale = '…'` shorthand. Renderer collapses back to
    /// `locale = '…'` when lc_collate == lc_ctype for terser output.
    pub lc_collate: String,
    /// `lc_ctype`.
    pub lc_ctype: String,
    /// `true` → standard sort; `false` → nondeterministic (PG 12+, ICU
    /// only). Defaults to `true` in source.
    pub deterministic: bool,
    /// Read-only; from `pg_collation.collversion`. Source never declares it;
    /// differ ignores it. REFRESH VERSION management is deferred to v0.3.9.
    pub version: Option<String>,
    pub owner: Option<Identifier>,
    pub comment: Option<String>,
}

pub enum CollationProvider {
    Libc,
    Icu,
    /// PG 17+ `builtin` provider (e.g. `C.UTF-8`).
    Builtin,
}
```

### Canon pass

`ir::canon::collations`:
- Sort `Catalog::collations` by qname.
- Default `deterministic` to `true` when source omits it.
- Default `provider` to `Libc` when source omits it (matches PG's
  pre-15 default behavior; PG 15+ defaults to ICU at cluster level but
  per-collation defaults to libc).
- Reject `nondeterministic + Libc` combinations at canon time with a
  clear error (PG rejects this at runtime; pre-empting gives a better
  message).

### Catalog reader

`catalog::collations`: query `pg_collation` JOIN `pg_namespace`. Filter
out:

- Collations in the `pg_catalog` namespace (covers `default`, `C`,
  `POSIX`, all libc system collations seeded at initdb, and the ICU
  built-ins `und-x-icu` / `unicode` / their derivatives).
- Collations whose `pg_depend.deptype = 'e'` (extension-owned).

This leaves only user-created collations. The version column
(`collversion`) is read and stored but the differ never touches it.

### Parser

`parse::builder::create_collation_stmt`:

- Handle `CREATE COLLATION [IF NOT EXISTS] qname (opt = val, …)`.
- Normalize `locale = 'X'` to `lc_collate = 'X', lc_ctype = 'X'` (IR
  always stores both separately).
- Accept `provider = libc | icu | builtin` (Builtin gated by lint at
  plan time, not parse time).
- Accept `deterministic = true | false`.
- Reject unknown options with a clear error naming the bad key.

### Differ

`diff::collations`: per-name pair-up (BTreeMap by qname).

```rust
pub enum CollationChange {
    Create(Collation),
    /// `DROP COLLATION qname` — destructive (intent required).
    Drop { qname: QualifiedName },
    /// `ALTER COLLATION qname RENAME TO new_name`.
    Rename { from: QualifiedName, to: Identifier },
    /// `DROP COLLATION old; CREATE COLLATION new;` — PG has no in-place
    /// ALTER for provider / locale / deterministic.
    Replace { from: Collation, to: Collation },
    /// `COMMENT ON COLLATION qname IS '…'`.
    CommentOn { qname: QualifiedName, comment: Option<String> },
}
```

Owner changes use the standard `Change::AlterObjectOwner` path
(`OwnedObjectId::Qualified(qname)`), not a new `CollationChange`
variant. This mirrors the Schema / Sequence / Table / View pattern.

The new variant on `Change` is `Change::Collation(CollationChange)`,
following the nested pattern established in v0.3.7's retrospective
cleanup.

### Plan

New `StepKind` variants:

| StepKind | kind_name (plan.sql) |
|---|---|
| `CreateCollation` | `create_collation` |
| `DropCollation` | `drop_collation` |
| `RenameCollation` | `rename_collation` |
| `ReplaceCollation` | `replace_collation` |
| `CommentOnCollation` | `comment_on_collation` |

New module `plan::rewrite::collations` holds the SQL emitters. The
renderer for `Create` chooses `locale = 'X'` form when lc_collate ==
lc_ctype.

### Dep graph

New `NodeId::Collation(QualifiedName)` variant in `plan::edges`.

`build_create_graph`:
- Add a node for every managed collation.
- Add `Column → Collation` edge when a column's `collation` qname
  matches a managed collation (skip built-ins via the
  `BUILTIN_COLLATIONS` allow-list).
- Add `Domain → Collation` for domain `collation` field.
- Add `Range → Collation` for range `collation` field.
- Add `CompositeAttribute → Collation` for composite attributes.

`build_drop_graph` mirrors edges in reverse so collations drop after
their dependents.

### Ordering

`plan::ordering::partition`:
- `Change::Collation(CollationChange::Create(_))` → creates bucket.
- `Change::Collation(CollationChange::Drop { .. } | Replace { .. })` →
  drops bucket.
- `Change::Collation(CollationChange::Rename { .. } | CommentOn { .. })`
  → modifies bucket.

`change_node` maps each variant to its `NodeId::Collation(qname)`.

---

## 4. Lint surface

| Rule ID | Severity | When |
|---|---|---|
| `unmanaged-collation` | Warning | Catalog has a collation not declared in source (lenient drift). Uses `check_unmanaged_objects` helper. |
| `column-references-unmanaged-collation` | Warning | A column's `collation` is a non-built-in qname not declared in source. Mirrors `view-body-references-unmanaged-schema`. |
| `range-type-references-unmanaged-subtype` | Warning | Range type's `subtype` is a non-built-in qname not declared in source. Extends the closed-world reference family. |
| `nondeterministic-collation-requires-pg-12` | Error | `[managed].min_pg_version < 12` and source declares a nondeterministic collation. Plan-time gate. |
| `builtin-provider-requires-pg-17` | Error | `[managed].min_pg_version < 17` and source uses `provider = builtin`. Plan-time gate. |

A new `BUILTIN_COLLATIONS` constant (in `ir::collation` or
`lint::rules::mod`) lists shortname collations that bypass
`column-references-unmanaged-collation`: `default`, `C`, `POSIX`,
`und-x-icu`, `unicode`, `ucs_basic`.

---

## 5. Conformance fixtures

`crates/pgevolve-conformance/tests/cases/objects/collations/` (6):

- `create-libc` — provider = libc, en_US.utf8
- `create-icu` — provider = icu, locale = und (PG 15+ via fixture
  `[pg.expect]`)
- `create-nondeterministic` — provider = icu, deterministic = false
  (PG 12+ gated)
- `drop` — destructive, intent required
- `comment-on` — non-destructive
- `rename` — `ALTER COLLATION x RENAME TO y`

`crates/pgevolve-conformance/tests/cases/objects/ranges/` (5):

- `create-simple-int4range` — subtype = int4, all other fields default
- `create-with-opclass` — explicit `subtype_opclass`
- `create-with-canonical-fn` — subtype + canonical (uses a
  source-declared function)
- `drop` — destructive, intent required
- `column-with-range-type` — table column whose type references a
  user-defined range type (exercises the column-type system path)

Plus 1 cross-cutting scenario:

- `scenarios/column-references-managed-collation` — table column with
  `COLLATE my_collation`, dep graph ordering verified.

---

## 6. Property tests

`crates/pgevolve-testkit/src/ir_generator/`:

- New `collation.rs` — `arb_collation()` (provider, locale shapes,
  deterministic toggle, hardcoded list of safe locale strings).
- New `arb_range_type_kind()` extending the user-type generation matrix
  (user-type strategies currently live in `ir_generator/mod.rs`; the
  new arm may justify pulling them into `ir_generator/user_type.rs`
  during the implementation plan).
- Both fed into `arbitrary_catalog` for round-trip property tests on
  PG 14-18.

---

## 7. Pre-commit verify gate addition

Lesson from the v0.3.4–v0.3.7 cycles + this session's cleanup runs:
`cargo doc` with strict warnings is the gate that has bitten refactor
commits twice. Add to the per-stage verify checklist alongside the
existing fmt / clippy / test:

```sh
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

Subagent implementer prompts (per-stage) reference this gate so each
stage's commits land green on first CI run.

---

## 8. Out of scope (deferred to v0.3.9+)

- `CREATE COLLATION FROM existing_collation` (round-trip ambiguity).
- `ALTER COLLATION … REFRESH VERSION` and `collation-version-drift` lint.
- Multirange type customization beyond the `multirange_type_name` field.
- Explicit multirange `IR` object (kept implicit per Section 2).
- `CollationProvider::Builtin`-specific locale value validation
  (lint only checks the PG version gate).

---

## 9. Sub-spec dependencies

- Independent of each other.
- COLLATION unblocks future `TEXT SEARCH` (v0.4.3).
- RANGE TYPE may surface latent column-type bugs for range-typed
  columns; tier-C fixture `column-with-range-type` exercises that path.

---

## 10. Stage outline (to be expanded by writing-plans)

Interleaved (v0.3.7 pattern), bundling both features per pipeline layer:

1. IR — Range variant on UserType
2. IR — Collation module + Catalog::collations + canon
3. Catalog reader — range (extend user_types) + collation (new)
4. Parser — CREATE COLLATION + RANGE option set
5. Diff — diff_range + diff_collations + CollationChange sub-enum
6. Plan — collation StepKinds + renderer + dep edges (incl. Column → Collation)
7. Lint — unmanaged-collation, column-references-unmanaged-collation,
   range-type-references-unmanaged-subtype, two PG-version gates
8. Conformance — 11 fixtures (6 collations + 5 ranges + 1 scenario)
9. Property tests — arb_collation + arb_range_type_kind
10. Docs — spec catalogue entries; roadmap rotation
11. Release — pre-release verify gate including cargo doc; tag, push,
    publish

The writing-plans skill expands each stage into TDD-shaped tasks.
