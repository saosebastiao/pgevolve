# v0.2 Sub-spec #4 — Functions and Procedures (Design)

**Status:** Approved 2026-05-18. Implementation plan to follow.

**Consumes architectural decisions:** Decisions 2 (identity), 3 (canonical bodies),
6 (AST resolution), 7 (function signature in NodeId), 8 (body cycles → error),
9 (AST-derived dep graph), 10 (NormalizedBody), 11 (PL/pgSQL directive close-out).

**Closes open question:** §14 — procedure transactional semantics.

---

## 1. Scope

This sub-spec brings Postgres functions and procedures under pgevolve's managed
surface — full lifecycle (CREATE / CREATE OR REPLACE / DROP) including body
canonicalization, attribute diffing, overload identity, PL/pgSQL static-SQL
dependency extraction, and dependent-recreation cascades.

### In scope

- **Languages:** SQL and PL/pgSQL only.
- **Object kinds:** `CREATE FUNCTION`, `CREATE PROCEDURE` (PG ≥ 11).
- **Function overloads:** full support. Identity is `(qname, NormalizedArgTypes)`.
- **Attributes:** full common set — language, volatility (IMMUTABLE/STABLE/VOLATILE),
  strict, security (DEFINER/INVOKER), parallel (UNSAFE/RESTRICTED/SAFE), leakproof,
  cost, rows, return type, argument modes (IN/OUT/INOUT/VARIADIC), argument defaults.
- **Trigger functions:** functions returning `TRIGGER` are normal functions; no
  special-case shape. Sub-spec #5 (triggers) consumes them by reference.
- **PL/pgSQL static-SQL deps:** parsed via `pg_query::plpgsql_parse`. Static
  queries (SELECT INTO, embedded queries) produce AST-derived `DepEdge`s.
- **PL/pgSQL dynamic-SQL deps:** closed by `-- @pgevolve dep: <qname>` directives
  (object-level only). The `pl-pgsql-dynamic-sql` lint flags missing directives.
- **Procedure `COMMIT`/`ROLLBACK`:** auto-detected at parse time. Sets
  `commits_in_body = true` on the IR record; planner emits the step with
  `transactional = Forbidden` so it runs outside any per-step transaction.
- **Diff shape:** `CREATE OR REPLACE` for any in-place-compatible change.
  Signature changes surface as `Drop(old_sig) + Create(new_sig)`.
- **Catalog reader:** `pg_proc` query + body re-parse through the same
  canonicalizer.

### Out of scope (v0.3 or later)

- Aggregate functions (`CREATE AGGREGATE`) — already future per `objects.md`.
- Window functions (`prokind = 'w'`) — filtered at the SQL level.
- Other languages (plperl, plpython, C, extension-provided) — surface as the
  `unsupported-function-language` lint error. Catalog reader emits
  `DriftReport::UnmanagedLanguageFunction { qname }` and skips the row.
- Function rename via directive (`-- @pgevolve replaces:`) — defer unless asked.
- Column-level dep directives — object-level only suffices for v0.2.

---

## 2. Key design decisions

### Decision A — One sub-spec covers both functions and procedures.

They share ~95% of infrastructure: same body canonicalization, same language
scope, same diff shape. Procedures lack a return type and most attributes; they
add the `commits_in_body` flag. Splitting would duplicate IR machinery and
fixtures. The arch spec also pairs them in §16.

### Decision B — Languages: SQL + PL/pgSQL only in v0.2.

Other languages (plperl, plpython, C, extension-provided) surface as a lint
error on the source side and as a `DriftReport` entry on the catalog side. The
function exists in PG but pgevolve treats it as unmanaged — same shape as the
existing `[managed].schemas` boundary in v0.1.

**Rationale:** Decision 11's directive mechanism was designed for PL/pgSQL.
plperl / plpython bodies are also opaque text but require their own AST parsing
infrastructure. C functions reference compiled `.so` files that pgevolve cannot
validate. Both belong to later sub-specs.

### Decision C — Procedure `COMMIT`/`ROLLBACK` → auto-detect, run outside transaction.

The PL/pgSQL parser walks the body AST for `PLpgSQL_stmt_commit` /
`PLpgSQL_stmt_rollback` nodes. If present, `Procedure.commits_in_body = true`.
The planner reads this flag and emits the step with
`transactional = TransactionConstraint::Forbidden`.

The user gets correct behavior without configuring anything. A separate
`procedure-contains-commit` lint (Warning, not Error) surfaces the situation so
it's visible in code review.

### Decision D — Full common attribute set; CREATE OR REPLACE for in-place changes.

Every common attribute is modeled: language, volatility, strict, security,
parallel, leakproof, cost, rows, return type, arg modes, arg defaults.
Any in-place-compatible change (body OR attribute OR comment) emits a single
`CreateOrReplaceFunction` step. The recreated function carries the new
everything; PG's `CREATE OR REPLACE FUNCTION` is the right tool.

A small set of return-type changes are NOT in-place-compatible per PG semantics
(changing OUT parameters, switching from scalar to setof). These trigger
`ReplaceWithCascade` — drop + create, cascading to dependent views/functions
via the existing `recreate_views` walker.

### Decision E — Signature change = Drop + Create (no auto-pair-rename).

When a function's argument types change, identity changes. The differ emits
two entries: `Drop(old_sig)` and `Create(new_sig)`. The differ never
heuristically pairs functions across signatures — too brittle without explicit
user intent. Users who genuinely want a rename can drop the old and add the
new in the same plan (both get intent gates).

### Decision F — Full overload support.

Identity: `(qname, NormalizedArgTypes)` per Decision 2. `app.compute(int)`
and `app.compute(text)` coexist as separate IR records, separate dep-graph
nodes, separate diff/plan/apply units. The AST resolver picks the right
overload by argument types when a function body calls another function. If no
overload matches the call signature, AST resolution fails with the expected vs.
observed signatures.

`NormalizedArgTypes` is `Vec<ColumnType>` over the IN/INOUT/VARIADIC args only
(matching PG's `proargtypes`). Hashed to BLAKE3 for fast identity equality.

### Decision G — Trigger functions are normal functions.

A function returning `TRIGGER` (or `event_trigger`) carries no special shape.
The parser, IR, diff, and planner handle it like any other function. Sub-spec
#5 (triggers) will add `CREATE TRIGGER` as a new object kind whose dep graph
node points at the trigger function's `NodeId::Function(qname, args)`.

### Decision H — Object-level dependency directives only.

`-- @pgevolve dep: <qname>` is the only directive form in v0.2. The qname can
be schema-qualified (`app.users`) or unqualified (resolves via the file's
`@pgevolve schema=` directive). Column-level and overload-signature directives
are deferred to v0.3 if needed.

---

## 3. IR additions

Two new flat collections on `Catalog`:

```rust
// pgevolve-core/src/ir/catalog.rs
pub struct Catalog {
    // ... existing fields ...
    pub functions: Vec<Function>,
    pub procedures: Vec<Procedure>,
}
```

Canonicalize sorts both by qname (functions then by `NormalizedArgTypes` lex
order for overload determinism). Duplicate detection follows the existing
`views`/`materialized_views`/`types` pattern.

### `Function`

```rust
// pgevolve-core/src/ir/function.rs
pub struct Function {
    pub qname: QualifiedName,
    pub args: Vec<FunctionArg>,
    pub arg_types_normalized: NormalizedArgTypes,
    pub return_type: ReturnType,
    pub language: FunctionLanguage,
    pub body: NormalizedBody,
    pub volatility: Volatility,
    pub strict: bool,
    pub security: SecurityMode,
    pub parallel: ParallelSafety,
    pub leakproof: bool,
    pub cost: Option<f32>,
    pub rows: Option<f32>,
    pub comment: Option<String>,
}

pub struct FunctionArg {
    pub name: Option<Identifier>,
    pub mode: ArgMode,
    pub ty: ColumnType,
    pub default: Option<NormalizedExpr>,
}

pub enum ArgMode { In, Out, InOut, Variadic }

pub enum ReturnType {
    Scalar(ColumnType),
    SetOf(ColumnType),
    Table(Vec<TableColumn>),    // RETURNS TABLE (col1 int, col2 text)
    Trigger,                    // RETURNS TRIGGER
    EventTrigger,               // RETURNS event_trigger
    Void,                       // RETURNS void (legal in SQL, common in PL/pgSQL)
}

pub struct TableColumn {
    pub name: Identifier,
    pub ty: ColumnType,
}

pub enum FunctionLanguage { Sql, PlPgSql }

pub enum Volatility { Immutable, Stable, Volatile }

pub enum SecurityMode { Invoker, Definer }

pub enum ParallelSafety { Unsafe, Restricted, Safe }

pub struct NormalizedArgTypes {
    pub types: Vec<ColumnType>,     // IN/INOUT/VARIADIC only
    pub canonical_hash: [u8; 32],   // BLAKE3 of canonicalized arg type strings
}
```

### `Procedure`

```rust
// pgevolve-core/src/ir/procedure.rs
pub struct Procedure {
    pub qname: QualifiedName,
    pub args: Vec<FunctionArg>,
    pub language: FunctionLanguage,
    pub body: NormalizedBody,
    pub security: SecurityMode,
    pub commits_in_body: bool,
    pub comment: Option<String>,
}
```

Procedures intentionally lack `return_type`, `volatility`, `strict`,
`parallel`, `leakproof`, `cost`, `rows`. PG syntax disallows these on
procedures. `arg_types_normalized` is omitted — procedure identity is
qname-only (Decision 2).

### Derives

`Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize` on every new type.
`f32` fields in `Function` (cost, rows) get the same manual `Eq`/`Hash` treatment
as `EnumValue.sort_order` from the types sub-spec (via `to_bits()`).

---

## 4. Source pipeline

### 4.1 Parser

PG uses **one** statement node for both `CREATE FUNCTION` and `CREATE PROCEDURE`
— `pg_query::protobuf::CreateFunctionStmt` with `is_procedure: bool`.

**New module:** `parse/builder/create_function_stmt.rs`.

`build_function_or_procedure(stmt, default_schema, location)`:

1. Resolve qname via the existing `qname_from_string_list` (signature was added
   in the types sub-spec).
2. Walk `stmt.parameters` (`FunctionParameter` nodes) → `Vec<FunctionArg>`.
   Each parameter carries `name`, `mode` (`FUNC_PARAM_IN/OUT/INOUT/VARIADIC/TABLE`),
   `arg_type` (a `TypeName`), and `defexpr` (default expression).
3. Compute `NormalizedArgTypes` from the IN/INOUT/VARIADIC args (matching PG's
   `proargtypes`). Hash via BLAKE3.
4. Walk `stmt.options` (a vec of `DefElem` nodes) for attributes. Recognized
   option names: `language`, `volatility`, `strict`, `security`, `parallel`,
   `leakproof`, `cost`, `rows`, `as` (body), `set` (search_path / GUCs —
   deferred to v0.3, lint-rejected for now). Unknown options → `ParseError::Structural`.
5. If `language` is not `sql` or `plpgsql` → `ParseError::Structural`
   with a message pointing at the option location.
6. Parse the body from the `as` option (one or two string literals — most
   functions have one, two is for C-language `(obj, sym)` pairs which are out of scope).
7. Run body through the language-specific canonicalizer (§4.2 / §4.3).
8. If `stmt.is_procedure`:
   - Apply procedure-specific constraints: reject `volatility`, `strict`,
     `parallel`, `leakproof`, `cost`, `rows` options (PG doesn't permit them).
     Reject return-type clause.
   - Detect `COMMIT`/`ROLLBACK` in body AST → `commits_in_body`.
   - Produce `Statement::CreateProcedure(...)`.
9. Else (function):
   - Parse return type from `stmt.return_type` (a `TypeName` or table-column list).
   - Produce `Statement::CreateFunction(...)`.

### 4.2 PL/pgSQL body parsing

**New module:** `parse/builder/plpgsql.rs`.

```rust
pub(crate) fn parse_plpgsql_body(
    body_text: &str,
    function_qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<(NormalizedBody, bool /* commits_in_body */), ParseError>
```

Internal flow:
1. Synthesize a wrapper SQL fragment that pg_query can parse for plpgsql AST:
   `pg_query::plpgsql_parse(format!("CREATE FUNCTION pgevolve_temp() RETURNS void LANGUAGE plpgsql AS ${{body_text}}$"))`.
   This is the standard pg_query API for PL/pgSQL parsing.
2. Walk the resulting `PLpgSQL_function` AST tree:
   - For each `PLpgSQL_stmt_execsql` (static embedded SQL) → re-parse the
     embedded statement via `pg_query::parse` to extract relation/function/type
     references → push `DepEdge`s with `DepSource::AstExtracted`.
   - For each `PLpgSQL_stmt_dynexecute` / `PLpgSQL_stmt_dynfors` (dynamic SQL,
     `EXECUTE format(...)`) → record the location for the
     `pl-pgsql-dynamic-sql` lint.
   - For each `PLpgSQL_stmt_commit` / `PLpgSQL_stmt_rollback` → set
     `commits_in_body = true`.
3. Scan the body text for `-- @pgevolve dep: <qname>` directive lines.
   Resolve `<qname>` (handling schema-directive default) → push `DepEdge` with
   `DepSource::AstDeclared`. Each declared directive also clears one flagged
   dynamic-SQL site (the lint checks that every dynamic site has at least one
   directive in the same body).
4. Build `NormalizedBody { canonical_text, canonical_hash, dependencies }`.
   The `canonical_text` is the body text after canonicalization (lowercased
   keywords, normalized whitespace within statement boundaries, preserved
   comments-as-directives). The `canonical_hash` is BLAKE3 of `canonical_text`.

### 4.3 SQL body parsing

For `language = sql`, parse the body via `pg_query::parse` directly — the body
is a sequence of SQL statements. Walk the AST for relation/function/type
references → `DepEdge`s. Build `NormalizedBody` via the same path as PL/pgSQL.

SQL functions don't have `COMMIT`/`ROLLBACK` — `commits_in_body` is meaningless
(the field is procedure-only anyway).

### 4.4 AST resolution

`parse/ast_resolution.rs` gains a `resolve_routine_references` pass:

For each `Function` and `Procedure`:
1. Walk `body.dependencies`. Each `DepEdge.to` is a `NodeId` variant — resolve
   it against the appropriate catalog collection:
   - `NodeId::Table(q)` → `catalog.tables` (and views/MVs for read-only deps).
   - `NodeId::Type(q)` → `catalog.types`.
   - `NodeId::Function(q, args)` → `catalog.functions` with overload resolution.
   - `NodeId::Procedure(q)` → `catalog.procedures`.
2. Unresolved references → `AstResolutionError { message, location }`.

**Overload resolution:** when the dep extractor sees `compute(x)` in a body,
it knows the call's syntactic argument types from the surrounding expression
context (or `UNKNOWN` if it's a column reference). The resolver tries to match
against `catalog.functions` entries:
- If exactly one overload's `NormalizedArgTypes` matches → resolved.
- If multiple match → `function-overload-ambiguous-call` warning, picks the
  lexicographically first match for the edge target. (Rare; warning is
  diagnostic.)
- If none match → `AstResolutionError`.

**Body cycles:** when ordering detects a cycle whose edges are all AST-derived
(`DepSource::AstExtracted` or `DepSource::AstDeclared`), surface
`PlanError::BodyCycle { nodes }`. The existing path in `plan/ordering.rs`
(introduced for views) already handles this — no new error variant.

### 4.5 Failure-mode tiers

| Phase | Surfaced as | Example |
|---|---|---|
| Parse | `ParseError::Structural` | Unknown option, unsupported language, malformed `AS $$...$$`. |
| AST resolution | `AstResolutionError` | Body references unknown table/type/function; overload not found. |
| Ordering | `PlanError::BodyCycle` | Two functions mutually call each other via static SQL. |
| Diff destructiveness | `Destructiveness::*` | Function drop, return-type-change cascade. |

---

## 5. Catalog reader

**New file:** `catalog/queries/functions.rs` with one SQL query covering both
functions and procedures (dispatched by `prokind` at the assembler).

```sql
SELECT
    n.nspname                                   AS schema_name,
    p.proname                                   AS name,
    p.prokind                                   AS kind,
    pg_get_function_identity_arguments(p.oid)   AS arg_signature,
    pg_get_function_arguments(p.oid)            AS arg_full,
    pg_get_function_result(p.oid)               AS return_type,
    l.lanname                                   AS language,
    p.provolatile                               AS volatility,
    p.proisstrict                               AS strict,
    p.prosecdef                                 AS security_definer,
    p.proparallel                               AS parallel,
    p.proleakproof                              AS leakproof,
    p.procost                                   AS cost,
    p.prorows                                   AS rows,
    pg_get_functiondef(p.oid)                   AS full_def,
    obj_description(p.oid, 'pg_proc')           AS comment
FROM pg_proc p
JOIN pg_namespace n ON p.pronamespace = n.oid
JOIN pg_language l ON p.prolang = l.oid
WHERE n.nspname = ANY($1::text[])
  AND p.prokind IN ('f', 'p')
ORDER BY n.nspname, p.proname, pg_get_function_identity_arguments(p.oid);
```

(Aggregate and window function rows — `prokind IN ('a', 'w')` — are filtered out.
A future sub-spec can lift them.)

**Catalog query slot:** `CatalogQuery::Functions`, fetched per-PG-major (same
SQL works for 14–17).

**Assembler** (`build_functions_and_procedures` in `assemble.rs`):

For each row:
1. Parse `arg_full` via a small sub-parser: pg_query handles
   `(a int, b text DEFAULT 1)` as a `FunctionParameter` list inside a synthetic
   `CREATE FUNCTION pgevolve_temp(${arg_full}) RETURNS void LANGUAGE sql AS $$$$;`
   wrapper.
2. Parse `return_type` (a string like `integer` or `SETOF app.user_row` or
   `TABLE(a int, b text)`) into the `ReturnType` enum. NULL for procedures.
3. Locate the function body inside `full_def` (output of `pg_get_functiondef`):
   - Find the `AS $tag$` opening dollar-quote marker.
   - Find the matching `$tag$` close.
   - Extract the body between them.
4. Re-parse the body via the same `parse_plpgsql_body` / `parse_sql_body`
   pipeline as the source side. Same canonicalizer = byte-stable `canonical_text`.
5. For `prokind = 'f'`: build `Function`, push to `catalog.functions`.
6. For `prokind = 'p'`: build `Procedure`, set `commits_in_body` from the AST
   walk, push to `catalog.procedures`.

**Unsupported-language rows** (`language ∈ {plperl, plpython, c, ...}`):
- Emit `DriftReport::UnmanagedLanguageFunction { qname, language }`.
- Skip the row — the function exists in PG but pgevolve doesn't manage it.
- The diff won't try to drop it (it's not in source AND it's not in the
  managed snapshot). This matches the v0.1 unmanaged-schema convention.

**Tier-3 goldens:** existing catalog fixtures gain `"functions": []` and
`"procedures": []` after blessing. Same shape as the types sub-spec golden
update.

---

## 6. Differ

**New module:** `diff/routines.rs` (one module covers both functions and
procedures since they share the differ shape).

### `FunctionChange` variants

```rust
pub enum FunctionChange {
    Create(Function),
    Drop { qname: QualifiedName, args: NormalizedArgTypes },
    CreateOrReplace(Function),       // body / attrs / return type / arg defaults
    ReplaceWithCascade {              // return type change PG can't OR-REPLACE through
        catalog: Function,
        source: Function,
    },
    SetComment { qname: QualifiedName, args: NormalizedArgTypes, comment: Option<String> },
}
```

### `ProcedureChange` variants

```rust
pub enum ProcedureChange {
    Create(Procedure),
    Drop(QualifiedName),
    CreateOrReplace(Procedure),
    SetComment { qname: QualifiedName, comment: Option<String> },
}
```

Procedures don't get `ReplaceWithCascade` because procedures don't have a
return type and `CREATE OR REPLACE PROCEDURE` accepts arg-mode/default changes
that don't change identity. Signature changes (different arg types) become
`Drop + Create`.

### Differ logic

**`diff_functions(catalog: &[Function], source: &[Function]) -> Vec<Change>`:**

1. Pair by `(qname, arg_types_normalized.canonical_hash)`.
2. **Source-only** → `Create`.
3. **Catalog-only** → `Drop` (destructive).
4. **Both present** → compare:
   - `body.canonical_hash`
   - return_type
   - language
   - All attributes (volatility, strict, security, parallel, leakproof, cost, rows)
   - args (mode, default expressions — the types themselves are identity, so
     they're already equal at this branch)
   - comment

   If any non-comment field differs:
   - **OR-REPLACE compatible?** Check via `function_can_or_replace`
     (see below). If yes → `CreateOrReplace`. If no → `ReplaceWithCascade`.

   If only the comment differs → `SetComment` (safe).

**`function_can_or_replace(catalog: &Function, source: &Function) -> bool`:**

PG's `CREATE OR REPLACE FUNCTION` rejects:
- Changing the number of output parameters.
- Changing the names of output parameters.
- Changing the return type kind (scalar ↔ setof ↔ table).
- Changing a function's language (effectively forces a recreate).

Returns `false` if any of the above; `true` otherwise.

**`diff_procedures`** is the same shape but smaller (no return type, fewer
attributes, no `ReplaceWithCascade`).

### Destructiveness

| Change | Destructiveness |
|---|---|
| `FunctionChange::Create` | Safe |
| `FunctionChange::CreateOrReplace` with no return-type narrowing | Safe (idempotent) |
| `FunctionChange::CreateOrReplace` with arg-default removal (breaks callers passing fewer args) | RequiresApproval |
| `FunctionChange::Drop` | RequiresApprovalAndDataLossWarning |
| `FunctionChange::ReplaceWithCascade` | RequiresApprovalAndDataLossWarning |
| `FunctionChange::SetComment` | Safe |
| `ProcedureChange::Create` / `CreateOrReplace` | Safe |
| `ProcedureChange::Drop` | RequiresApprovalAndDataLossWarning |
| `ProcedureChange::SetComment` | Safe |

---

## 7. Planner

### 7.1 New StepKind variants (6)

```rust
StepKind::CreateOrReplaceFunction
StepKind::DropFunction
StepKind::CommentOnFunction
StepKind::CreateOrReplaceProcedure
StepKind::DropProcedure
StepKind::CommentOnProcedure
```

`CreateOrReplaceFunction` covers both the initial create AND the
in-place replace — PG accepts `CREATE OR REPLACE FUNCTION` for both cases,
which lets us emit one less step kind. `ReplaceWithCascade` decomposes into
`DropFunction + CreateOrReplaceFunction + <dependent recreations>`.

snake_case names: `create_or_replace_function`, `drop_function`,
`comment_on_function`, `create_or_replace_procedure`, `drop_procedure`,
`comment_on_procedure`.

### 7.2 NodeId additions

```rust
pub enum NodeId {
    // ...existing variants...
    Function(QualifiedName, NormalizedArgTypes),
    Procedure(QualifiedName),
}
```

`Ord` on `NodeId::Function` is lex-on-(qname, args.canonical_hash). Min-heap
tiebreaker in Kahn's topo sort continues to produce deterministic order.

### 7.3 Dep graph edges (`build_create_graph`)

- **Phase 1b.0**: `Function → Schema` and `Procedure → Schema` for every routine.
- **Phase 1c**: For each `FunctionArg.ty == ColumnType::UserDefined(q)` or
  return type with a UserDefined component → edge `Function → Type(q)`.
  Similarly for procedure args.
- **Phase 2c**: For each `DepEdge` in `body.dependencies` → edge `Function/Procedure → <target>`.
  The target may be `Table`, `View`, `Mv`, `Type`, `Function`, or `Procedure`.

### 7.4 Dependent-recreation walker

Extend `plan/recreate_views.rs` to handle function and procedure cascades:

- `object_drop_qname` (or the renamed `object_drop_node`) now matches
  `FunctionChange::Drop`, `FunctionChange::ReplaceWithCascade`, and
  `ProcedureChange::Drop`. The target node becomes `NodeId::Function(...)` or
  `NodeId::Procedure(...)`.
- `build_dep_index` filter_map now includes function and procedure nodes
  alongside table/view/MV/type. Views/MVs/functions with `body_dependencies`
  pointing at a dropped function get a `CreateOrReplace` recreation.
- Functions that reference each other transitively form a recreation chain —
  the existing topological recursion handles it without new logic.

### 7.5 Step transaction policy

Procedures with `commits_in_body = true` emit their `CreateOrReplaceProcedure`
step with `transactional = TransactionConstraint::Forbidden`. The planner
groups it into its own per-step group. All other function/procedure steps run
inside the normal per-step transaction (`InTransaction`).

### 7.6 SQL emission

New file: `plan/rewrite/functions.rs`. Per-StepKind emitters:

- `emit_create_or_replace_function(fun: &Function) -> String` — assembles
  `CREATE OR REPLACE FUNCTION qname(args) RETURNS type LANGUAGE lang
   {volatility} {strict} {security} {parallel} {leakproof} COST {cost} ROWS {rows}
   AS $body$ ... $body$;`.
  Attributes emitted in a fixed order for determinism. Body uses an
  unambiguous dollar-quote tag (e.g., `$pgevolve$`).
- `emit_drop_function(qname, args)` → `DROP FUNCTION qname(arg_signature);`.
- `emit_comment_on_function(qname, args, comment)` → `COMMENT ON FUNCTION qname(arg_signature) IS '...';` (or `NULL`).
- Mirrors for procedures.

---

## 8. Lints

Five new universal rules:

### 8.1 `unsupported-function-language` (Error)

Fires when a source function/procedure declares a language other than `sql`
or `plpgsql`. The message names the unsupported language and points at the
source line. Users can either remove the function from the managed surface
(move it out of `[managed].schemas`) or wait for v0.3.

### 8.2 `pl-pgsql-dynamic-sql` (Error)

Fires when a PL/pgSQL body contains `EXECUTE` / dynamic SQL without at least
one `-- @pgevolve dep: <qname>` directive in the same body. The lint
description includes the directive syntax for actionable fix.

The check passes if at least one directive is present, even if the directive
set is incomplete. Verifying completeness requires `--shadow-validate`
(Decision 12).

### 8.3 `function-overload-ambiguous-call` (Warning)

Fires when AST resolution finds a function call where multiple overloads
match the syntactic argument types. The dep edge picks the lexicographically
first match; the warning surfaces the ambiguity so users can disambiguate via
explicit type casts in source.

### 8.4 `procedure-contains-commit` (Warning)

Informational: surfaces that a procedure body contains `COMMIT`/`ROLLBACK`,
which causes pgevolve to emit it with `transactional = Forbidden`. Visible in
code review without blocking the plan.

### 8.5 `function-references-unmanaged-schema` (Warning)

Mirrors the existing `view-body-references-unmanaged-schema` lint. Fires when
a function/procedure body has a `DepEdge` whose target qname's schema is
neither in `[managed].schemas` nor a PG built-in (`pg_catalog`,
`information_schema`).

---

## 9. Documentation updates

- `docs/spec/objects.md` — flip `FUNCTION (SQL)`, `FUNCTION (PL/pgSQL)`,
  `PROCEDURE` rows from 📋 Planned to ✅ Implemented with `change_kinds`
  annotations: `[create, drop, create_or_replace, replace_with_cascade,
  comment_on]` (functions) and `[create, drop, create_or_replace, comment_on]`
  (procedures).
- `docs/spec/lint-and-layout.md` — 5 new rule rows.
- `docs/user/plan-format.md` — 6 new step kinds documented.
- `docs/user/cookbook.md` — "Managing functions and procedures" section with
  worked examples: SQL function, PL/pgSQL function with `@pgevolve dep:`,
  function body replacement, adding an overload, procedure with `COMMIT`.
- `docs/system/ir.md` — `Function` and `Procedure` sections.
- `docs/system/planner.md` — overload signature disambiguator notes; cascade
  walker extension for functions.
- `README.md` — flip v0.2 sub-spec #4 to ✅ Landed.
- `CHANGELOG.md` — extend `[0.2.0]` with function + procedure entries.

---

## 10. Testing

### 10.1 Conformance fixtures (~22)

**Functions** (`objects/functions/`):
- `create-sql/` — bare SQL function returning scalar.
- `create-plpgsql/` — PL/pgSQL function with static SELECT INTO.
- `create-with-table-return/` — `RETURNS TABLE(a int, b text)`.
- `create-trigger-function/` — `RETURNS TRIGGER`.
- `replace-body/` — body changes, attributes stay same → single `CreateOrReplace`.
- `replace-attribute-volatility/` — body unchanged, volatility flips → single `CreateOrReplace`.
- `replace-return-type-cascade/` — return-type kind changes → ReplaceWithCascade.
- `overload-pair/` — `app.compute(int)` and `app.compute(text)` declared together.
- `drop/` — function exists in catalog, removed from source.
- `comment-on-function/` — comment add/change.

**Procedures** (`objects/procedures/`):
- `create-simple/` — bare procedure.
- `create-with-commit/` — procedure body contains COMMIT → step is `transactional=Forbidden`.
- `replace-body/`.
- `drop/`.
- `comment-on-procedure/`.

**Intent** (`intent/`):
- `drop-function-requires-intent/`.
- `drop-procedure-requires-intent/`.
- `function-return-type-change-cascade-requires-intent/`.

**Scenarios** (`scenarios/`):
- `function-calls-function/` — transitive dep through static SQL.
- `view-uses-function/` — cross-kind dep (view body calls function).
- `function-with-dynamic-sql-directive/` — `-- @pgevolve dep:` clears the `pl-pgsql-dynamic-sql` lint.
- `function-cycle-rejected/` — mutual recursion via static SQL → `PlanError::BodyCycle`.

### 10.2 Property tests

One nightly property test, `#[ignore]`-gated, no Docker:

```rust
/// For a random function with a random PL/pgSQL body, parsing then re-emitting
/// produces byte-identical canonical text. This is the byte-stable round-trip
/// invariant that the differ relies on.
fn plpgsql_canonicalization_is_idempotent(...) { ... }
```

### 10.3 Tier-3 catalog goldens

Existing catalog fixtures regenerated to include `"functions": []` and
`"procedures": []` for v0.1 / earlier-v0.2 fixtures. New fixtures specific to
function/procedure rows author one of each per PG major (PG 14/15/16/17).

---

## 11. Open questions (deferred)

- **Extension-provided languages** (plperl, plpython, plv8): covered when
  the extension sub-spec (#3) lands or via a follow-up function sub-spec.
- **Function rename via directive** (`-- @pgevolve replaces: app.old(int)`):
  defer until a user asks.
- **Column-level dep directives** (`-- @pgevolve dep: app.users.email`):
  defer to v0.3.
- **Function-signature dep directives** (`-- @pgevolve dep: app.compute(int)`):
  defer to v0.3 unless overload ambiguity becomes a real pain point.
- **`SET` option clause** (`SET search_path = ...` on a function): rejected at
  parse time in v0.2; lift in a follow-up.
- **Aggregate functions** (`CREATE AGGREGATE`): explicitly future per
  `docs/spec/objects.md`.
- **PG patch-version drift in `pg_get_functiondef`**: handled by the
  test-strategy spec; the canonicalizer absorbs cosmetic drift.

---

## 12. Phasing — implementation plan outline

Approximately 14 tasks:

1. **IR types** — `Function`, `Procedure`, `FunctionArg`, supporting enums,
   `NormalizedArgTypes`. `Catalog` gains `functions` and `procedures` collections.
2. **Source parser — CREATE FUNCTION** — `build_function_or_procedure` for the
   function branch. Arg parsing, option parsing, return type parsing.
3. **Source parser — CREATE PROCEDURE** — branch of the same builder.
   Reject function-only attributes; allow procedure-only behaviors.
4. **PL/pgSQL body parsing** — `parse_plpgsql_body`; dep extraction;
   `COMMIT`/`ROLLBACK` detection; directive scanning.
5. **SQL body parsing** — `parse_sql_body`; dep extraction.
6. **AST resolution** — `resolve_routine_references`; overload resolution.
7. **Catalog reader** — `SELECT_FUNCTIONS` SQL + `build_functions_and_procedures`
   assembler; body re-parse pipeline.
8. **Differ** — `FunctionChange` / `ProcedureChange` variants;
   `function_can_or_replace` predicate; pair-by-identity logic.
9. **NodeId::Function/Procedure + dep graph edges + cascade walker** —
   extends `recreate_views` for routine drops/replaces.
10. **Planner step kinds + SQL emission** — 6 new StepKinds;
    `plan/rewrite/functions.rs`; transaction policy.
11. **Five lint rules** — `unsupported-function-language`,
    `pl-pgsql-dynamic-sql`, `function-overload-ambiguous-call`,
    `procedure-contains-commit`, `function-references-unmanaged-schema`.
12. **Conformance fixtures** — ~22 across objects/intent/scenarios.
13. **Property test + documentation** — `plpgsql_canonicalization_is_idempotent`
    nightly; flip docs to ✅; cookbook section.
14. **Final review + branch finishing**.

---

## 13. Implementation venue

This spec produces decisions. The implementation plan in
`docs/superpowers/plans/2026-05-18-functions-procedures.md` (to be written
next) decomposes Section 12 into bite-sized TDD-shaped tasks.
