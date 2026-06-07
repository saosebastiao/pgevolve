---
status: design
target: v0.4.1
sub_spec: aggregate
---

# `AGGREGATE` — design

Adds user-defined aggregates (`CREATE AGGREGATE`), a v0.4.1 roadmap row. An
aggregate wraps a state function plus a state type and optional final
function / initial condition to define an application aggregate (e.g.
`weighted_avg(numeric, numeric)`). A new schema-scoped object kind, modeled
like the existing schema-scoped objects (functions, user types).

**Scope — ordinary aggregates only:** `sfunc` + `stype` + optional
`finalfunc` + optional `initcond`. Out: ordered-set aggregates
(`… ORDER BY`), moving aggregates (`MSFUNC`/`MINVFUNC`/…), `COMBINEFUNC`/
`SERIALFUNC`/`DESERIALFUNC` (parallel/serialization support), and any
hypothetical/variadic exotica. These are deferred; an aggregate using them
is rejected at IR-build (source) / skipped (reader) the same way an
unreadable state function is (§2).

Brainstorming decisions:
- **Managed SQL/plpgsql state functions only.** `sfunc` and `finalfunc` must
  resolve to functions pgevolve manages and reads (SQL or plpgsql). An
  aggregate whose state/final function is a C/internal/built-in or an
  unread-PL function is **rejected at IR-build** on the source side and
  **skipped** on the reader side (recorded as drift), mirroring how
  unsupported-language functions are already handled
  (`catalog/assemble/functions.rs`). The constraint relaxes in v0.4.2 when
  PL-language wiring lands.
- **No first-class rename.** Identity is `(schema, name, arg_types)`; a
  renamed aggregate reads as drop-old + create-new. Aggregates carry no data,
  so drop+create is safe. `ALTER AGGREGATE … RENAME TO` in source is rejected
  (consistent with triggers/event-triggers/tablespaces).

---

## §1. IR

New module `crates/pgevolve-core/src/ir/aggregate.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aggregate {
    /// Schema-qualified aggregate name.
    pub qname: QualifiedName,
    /// Aggregate argument types. Identity is `(schema, name, arg_types)`
    /// (aggregates are overloadable).
    pub arg_types: Vec<ColumnType>,
    /// State type (`STYPE`).
    pub state_type: ColumnType,
    /// State transition function (`SFUNC`) — a managed function name. Its
    /// implied signature is `(state_type, arg_types…)`.
    pub sfunc: QualifiedName,
    /// Optional final function (`FINALFUNC`) — managed; implied signature
    /// `(state_type)`.
    pub finalfunc: Option<QualifiedName>,
    /// Optional initial condition (`INITCOND`), stored as text.
    pub initcond: Option<String>,
    /// Lenient owner (`None` = unmanaged).
    pub owner: Option<Identifier>,
    pub comment: Option<String>,
}
```

`Catalog` gains `pub aggregates: Vec<Aggregate>`. Canon sorts by
`(schema, name, arg_types)` and rejects a duplicate identity
(`IrError::DuplicateAggregate`). `arg_types`/`state_type` reuse `ColumnType`
(the same representation function args use), so type normalization is shared.

## §2. The managed-function constraint

`sfunc` (and `finalfunc` if present) must resolve to a pgevolve-managed
function — i.e. a function present in the catalog with a readable language
(SQL or plpgsql). Enforcement:

- **Source (parse / IR-build):** an aggregate whose `sfunc`/`finalfunc` does
  not match a managed function in the same catalog (by name + the implied
  signature) is rejected with a structured `IrError`
  (`AggregateUnmanagedStateFunction { aggregate, function }`). This is a
  closed-world check run after the source catalog is assembled (like the
  existing closed-world reference checks).
- **Reader:** a live aggregate whose `aggtransfn`/`aggfinalfn` is an
  unsupported-language or otherwise-unmanaged function is **skipped** and
  recorded in the `DriftReport` (a new `unmanaged_aggregates` list), exactly
  as unsupported-language functions are skipped today. A skipped aggregate is
  never diffed or dropped.

Likewise, an aggregate using any out-of-scope feature (ordered-set, moving,
combine/serial/deserial) is rejected (source) / skipped (reader).

## §3. Parser

`crates/pgevolve-core/src/parse/builder/aggregate_stmt.rs`, dispatched on
`DefineStmt` with `kind = OBJECT_AGGREGATE` (pg_query models `CREATE
AGGREGATE` as a `DefineStmt`), plus `AlterOwnerStmt`/`RenameStmt`/`DropStmt`/
`CommentStmt` with `ObjectType::ObjectAggregate`:

- `CREATE AGGREGATE name (argtypes) (SFUNC = f, STYPE = t [, FINALFUNC = g] [, INITCOND = '…'])`
  → an `Aggregate`. The `DefineStmt.definition` is a list of `DefElem`s
  (`sfunc`, `stype`, `finalfunc`, `initcond`, …); read them into the fields.
  Reject any `DefElem` outside the supported set (ordered-set/moving/combine/…)
  with a structured error.
- `ALTER AGGREGATE name(argtypes) OWNER TO role` → set `owner`.
- `COMMENT ON AGGREGATE name(argtypes) IS '…'` → set `comment`.
- `DROP AGGREGATE` in source and `ALTER AGGREGATE … RENAME TO` are rejected
  (drops come from the diff; rename is drop+create).

## §4. Catalog reader

`crates/pgevolve-core/src/catalog/assemble/aggregates.rs` + a query over
`pg_aggregate` joined to its wrapper `pg_proc` (`prokind = 'a'`):

- wrapper `pg_proc`: `proname`/`pronamespace` → `qname`; `proargtypes` →
  `arg_types`; `proowner` → `owner`; comment via `pg_description`.
- `pg_aggregate`: `aggtransfn` → `sfunc` (resolve OID → qname; **reject/skip
  if its language is unreadable**), `aggtranstype` → `state_type`,
  `aggfinalfn` (`0` = none) → `finalfunc`, `agginitval` → `initcond`.
- Skip (record in `DriftReport.unmanaged_aggregates`) any aggregate that is
  ordered-set/moving (`aggkind <> 'n'`) or whose state/final function is
  unmanaged.

**Critical:** the existing function reader (`assemble/functions.rs`) must
exclude `prokind = 'a'` so aggregates do not also surface as functions
(aggregates share the `pg_proc` namespace). Verify/adjust the function query's
`prokind` filter.

## §5. Canon

`crates/pgevolve-core/src/ir/canon/aggregates.rs`: sort `Catalog::aggregates`
by `(schema, name, arg_types)`; reject duplicate identity. `arg_types` and
`state_type` are normalized through the same `ColumnType` canon the rest of
the IR uses (resolve-user-defined-types etc. already run catalog-wide).

## §6. Diff

`crates/pgevolve-core/src/diff/aggregates.rs`, paired by
`(schema, name, arg_types)` identity (schema-scoped, **managed** — not
lenient on drop):
- source-only → `CreateAggregate` (Safe).
- target-only → `DropAggregate` (Safe — aggregates carry no data).
- both present:
  - any structural difference (`state_type`, `sfunc`, `finalfunc`, `initcond`)
    → `Replace` (DROP + CREATE; PG has no in-place ALTER for these). Safe.
  - `owner` differs **and source declares one** → `AlterAggregateOwner` (lenient).
  - `comment` differs → `CommentOnAggregate`.

New `AggregateChange` enum (Create/Replace/Drop/AlterOwner/CommentOn) on
`Change`.

## §7. Render + dependency graph

- Render `CREATE AGGREGATE name (argtypes) (SFUNC = sfunc, STYPE = state_type
  [, FINALFUNC = finalfunc] [, INITCOND = 'initcond']);`,
  `DROP AGGREGATE name (argtypes);`,
  `ALTER AGGREGATE name (argtypes) OWNER TO owner;`,
  `COMMENT ON AGGREGATE name (argtypes) IS '…';`. New `StepKind`s.
- Dependency graph: `NodeId::Aggregate((qname, arg_types))` with edges to its
  `sfunc` and `finalfunc` function nodes (the function node id is
  `(func_qname, normalized_implied_signature)` — `sfunc` signature is
  `(state_type, arg_types…)`, `finalfunc` signature is `(state_type)`), and to
  the `state_type` / `arg_types` user-type nodes when those types are managed.
  So state functions and types are created before the aggregate (and dropped
  after).

## §8. Tests

- **Conformance** `objects/aggregates/`: `create-simple` (managed plpgsql
  sfunc + stype), `create-with-finalfunc`, `create-with-initcond`, `drop`,
  `comment-on`, and `failure/reject-unmanaged-state-fn` (an aggregate over a
  built-in/C sfunc → source rejected). Each non-failure fixture declares the
  managed sfunc/finalfunc function in its SQL.
- **E2E**: apply a catalog with a managed plpgsql `sfunc` and an aggregate over
  it to an ephemeral PG, introspect, assert round-trip convergence.
- **Unit**: parser (each DefElem; reject out-of-scope; reject rename/drop in
  source); reader decode (incl. skip of unmanaged/ordered-set); canon (sort +
  dup); diff (create/drop/replace on each structural field; lenient owner;
  comment); the closed-world constraint (source aggregate over an undeclared
  function → IrError); render strings; dep-graph edge to sfunc/finalfunc.

## §9. Out of scope / non-goals

- Ordered-set (`… ORDER BY`) and moving aggregates; `COMBINEFUNC`/`SERIALFUNC`/
  `DESERIALFUNC`/`MSFUNC`-family.
- State/final functions that are not managed SQL/plpgsql functions (C,
  internal, built-in, unread PL) — rejected/skipped (relaxes v0.4.2).
- First-class `ALTER AGGREGATE … RENAME TO` (name+arg-types is identity).
- `CREATE ACCESS METHOD`-style aggregate-machinery definitions.
