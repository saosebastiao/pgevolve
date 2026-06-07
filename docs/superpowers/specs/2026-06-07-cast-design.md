---
status: design
target: v0.4.x
sub_spec: cast
---

# `CREATE CAST` — design

Adds user-defined casts (`CREATE CAST`), the v0.5.2 roadmap row pulled forward.
A cast defines a coercion from one type to another, optionally via a conversion
function. A new **global** object kind (not schema-scoped — a cast lives in no
schema and has no owner), modeled very closely on the just-shipped `AGGREGATE`
feature: both are global, both reference a managed function, both are managed on
drop, and neither has an in-place `ALTER`.

Brainstorming decisions:
- **Managed (auto-drop).** A live cast absent from source is dropped (`DROP CAST`,
  Safe — casts carry no data), consistent with aggregates and user types. This
  makes the reader's exclusion of non-managed casts load-bearing (§4).
- **Managed-function constraint, mirroring AGGREGATE.** A `WITH FUNCTION` cast
  whose function is not a pgevolve-managed SQL/plpgsql function is **rejected at
  IR-build** (source) and **skipped** (reader, recorded as drift), exactly as
  aggregate state functions are handled. `WITHOUT FUNCTION` / `WITH INOUT` carry
  no function and are unaffected. The constraint relaxes in v0.4.2 when
  PL-language wiring lands.
- **No `ALTER CAST`.** Postgres has no `ALTER CAST`; identity is
  `(source, target)`. Any structural change (method or context) reads as
  drop-old + create-new (`Replace`). Casts carry no data, so this is safe.

---

## §1. IR

New module `crates/pgevolve-core/src/ir/cast.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cast {
    /// Source type.
    pub source: QualifiedName,
    /// Target type. Identity is `(source, target)`.
    pub target: QualifiedName,
    /// How the cast is performed.
    pub method: CastMethod,
    /// When the cast is applied implicitly.
    pub context: CastContext,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CastMethod {
    /// `WITH FUNCTION fn(argtypes)` — a managed function. `arg_types` is the
    /// recorded conversion-function signature (1–3 args: source[, int4, bool]).
    Function { name: QualifiedName, arg_types: Vec<ColumnType> },
    /// `WITH INOUT` — uses the types' I/O functions.
    Inout,
    /// `WITHOUT FUNCTION` — binary-coercible, no function.
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastContext {
    /// Default — explicit casts only.
    Explicit,
    /// `AS ASSIGNMENT`.
    Assignment,
    /// `AS IMPLICIT`.
    Implicit,
}
```

`Catalog` gains `pub casts: Vec<Cast>`. Canon sorts by `(source, target)` (each
rendered via `QualifiedName::render_sql`) and rejects a duplicate identity
(`IrError::DuplicateCast`). `source`/`target`/`arg_types` reuse the existing type
representations so normalization is shared. A cast has **no owner** (pg_cast has
no owner column), so there is no `AlterOwner` path.

## §2. The managed-function constraint

For `CastMethod::Function`, the named function must resolve to a
pgevolve-managed function (present in the catalog, readable language SQL or
plpgsql), resolved by qname + the recorded `arg_types` signature. Enforcement
mirrors `AGGREGATE` (`diff`/`lint`/`assemble` for aggregates):

- **Source (parse / IR-build):** a cast whose `WITH FUNCTION` function does not
  match a managed function is rejected via a closed-world check —
  `Finding::error("cast-references-unmanaged-function", …)` at plan time (the
  same severity/wiring as `aggregate-references-unmanaged-function`).
- **Reader:** a live cast whose `castfunc` is an unsupported-language or
  otherwise-unmanaged function is **skipped** and recorded in
  `DriftReport.unmanaged_casts`, exactly as unmanaged aggregates are. A skipped
  cast is never diffed or dropped.

`WITHOUT FUNCTION` and `WITH INOUT` have no function and are never subject to
this check.

## §3. Parser

`crates/pgevolve-core/src/parse/builder/cast_stmt.rs`:

- `CreateCastStmt` → a `Cast`. Read `sourcetype`/`targettype` (`TypeName` →
  `QualifiedName`), the function (`func` present → `Function`; else `inout` flag
  → `Inout`; else → `Binary`), and `context` (`COERCION_IMPLICIT` → `Implicit`,
  `COERCION_ASSIGNMENT` → `Assignment`, else `Explicit`). For `WITH FUNCTION`,
  capture the function's qname and arg-type list (reuse the shared
  `qname_from_string_list` / `type_name_to_column_type` helpers the aggregate
  parser uses).
- `DropStmt` with `ObjectType::ObjectCast` and `CommentStmt` over a cast → not
  parsed from source as standalone drops (drops come from the diff); a
  `COMMENT ON CAST (src AS tgt) IS '…'` sets `comment`. Reject any unsupported
  form with a structured error.

## §4. Catalog reader

`crates/pgevolve-core/src/catalog/assemble/casts.rs` + a query over `pg_cast`
joined to `pg_type` (twice, for source/target) and `pg_proc` (for the function):

- `castsource` / `casttarget` → `source` / `target` `QualifiedName` (schema +
  name resolved via `pg_type`/`pg_namespace`).
- `castmethod` (`f`/`i`/`b`) → `Function`/`Inout`/`Binary`; for `f`, `castfunc`
  → `pg_proc` → function qname + arg types (**reject/skip if its language is
  unmanaged**).
- `castcontext` (`e`/`a`/`i`) → `Explicit`/`Assignment`/`Implicit`.
- comment via `pg_description`.

**Exclusion (load-bearing because drop is managed):**
- skip **system/built-in casts** (`pg_cast.oid < 16384`, i.e. below
  `FirstNormalObjectId`);
- skip **extension-owned casts** (`pg_depend` `deptype = 'e'` referencing the
  `pg_cast` row), mirroring the aggregate reader's extension exclusion;
- skip + record in `DriftReport.unmanaged_casts` any cast whose `WITH FUNCTION`
  function is unmanaged.

## §5. Canon

`crates/pgevolve-core/src/ir/canon/casts.rs`: sort `Catalog::casts` by
`(source, target)`; reject duplicate identity. Type names are normalized through
the same canon the rest of the IR uses (catalog-wide user-type resolution
already runs).

## §6. Diff

`crates/pgevolve-core/src/diff/casts.rs`, paired by `(source, target)` identity
(global, **managed** — not lenient on drop):
- source-only → `Create` (Safe).
- target-only → `Drop` (Safe — casts carry no data).
- both present:
  - any structural difference (`method`, `context`) → `Replace` (DROP + CREATE;
    no `ALTER CAST`). Safe.
  - `comment` differs → `CommentOn`.

New `CastChange` enum (`Create` / `Replace{from,to}` / `Drop{source,target}` /
`CommentOn{source,target,comment}`) on `Change`. No owner variant.

## §7. Render + dependency graph

- Render `CREATE CAST (src AS tgt) WITH FUNCTION fn(argtypes) [AS ASSIGNMENT | AS
  IMPLICIT];` (or `WITHOUT FUNCTION` / `WITH INOUT`), `DROP CAST (src AS tgt);`,
  `COMMENT ON CAST (src AS tgt) IS '…';`. New `StepKind`s
  (`CreateCast`/`DropCast`/`CommentOnCast`); `Replace` = drop-then-create.
- Dependency graph: `NodeId::Cast((source, target))` with an edge to the
  `WITH FUNCTION` function node (function-node id is `(func_qname,
  normalized_arg_types)`, as for aggregate `sfunc`) and to the `source` / `target`
  **user-type** nodes when those types are managed. So the function and any
  managed types are created before the cast and dropped after.

## §8. Tests

- **Conformance** `objects/casts/`: `create-with-function` (managed plpgsql
  function between two managed types), `create-without-function`,
  `create-with-inout`, `create-as-implicit`, `drop`, `comment-on`, and
  `failure/reject-unmanaged-cast-function` (a cast over a built-in/C function →
  source rejected).
- **E2E**: apply a catalog with two managed types, a managed plpgsql conversion
  function, and a cast over them to an ephemeral PG; introspect; assert
  round-trip convergence.
- **Unit:** parser (each method + each context; reject unsupported); reader
  decode (incl. skip of built-in/extension/unmanaged-function casts); canon
  (sort + dup); diff (create/drop/replace on method and on context; comment);
  the closed-world constraint (source cast over an undeclared/unmanaged function
  → finding); render strings; dep-graph edge to function + managed types.

## §9. Out of scope / non-goals

- Built-in / system casts (excluded by the reader OID filter) and
  extension-owned casts (excluded via `pg_depend`).
- Casts whose `WITH FUNCTION` function is not a managed SQL/plpgsql function —
  rejected (source) / skipped (reader); relaxes in v0.4.2 with PL-language wiring.
- `ALTER CAST` (does not exist in Postgres); a method/context change is
  drop + create.
