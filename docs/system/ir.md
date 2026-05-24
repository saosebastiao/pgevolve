# IR

Deeper dive on the data model that everything diffs against. Source for
this doc lives in `crates/pgevolve-core/src/ir/`.

## Top-level shape

```rust
pub struct Catalog {
    pub schemas:            Vec<Schema>,
    pub extensions:         Vec<Extension>,
    pub tables:             Vec<Table>,
    pub indexes:            Vec<Index>,
    pub sequences:          Vec<Sequence>,
    pub views:              Vec<View>,
    pub materialized_views: Vec<MaterializedView>,
    pub types:              Vec<UserType>,
    pub functions:          Vec<Function>,
    pub procedures:         Vec<Procedure>,
    pub triggers:           Vec<Trigger>,
    pub default_privileges: Vec<DefaultPrivilegeRule>,
}
```

Flat collections, no nesting (e.g., `Table::indexes`) for a reason:
indexes live in their own namespace within a schema and reference their
table by qname. Hierarchical nesting would have to maintain referential
integrity at every mutation; flat lists + name-based references defer
that to canonicalization time.

Cluster-level state (`Role`, `RoleAttributes`) lives in a sibling
`ClusterCatalog` rather than on `Catalog`, since cluster surface is
managed via the separate `pgevolve cluster …` subcommand family. See
[`docs/spec/cluster.md`](../spec/cluster.md).

The implicit name-based relationships look like this:

```mermaid
erDiagram
    Catalog ||--o{ Schema           : "schemas[]"
    Catalog ||--o{ Table            : "tables[]"
    Catalog ||--o{ Index            : "indexes[]"
    Catalog ||--o{ Sequence         : "sequences[]"
    Catalog ||--o{ View             : "views[]"
    Catalog ||--o{ MaterializedView : "materialized_views[]"
    Table   ||--o{ Column           : "columns[]"
    Table   ||--o{ Constraint       : "constraints[]"
    View    ||--o{ ViewColumn       : "columns[]"
    MaterializedView ||--o{ ViewColumn : "columns[]"
    Index    }o--|| Table           : "table (qname)"
    Sequence }o--o| Table           : "owned_by (optional)"
    Schema   ||--o{ Table           : "qname.schema"
    Schema   ||--o{ Index           : "qname.schema"
    Schema   ||--o{ Sequence        : "qname.schema"
    Schema   ||--o{ View            : "qname.schema"
    Schema   ||--o{ MaterializedView : "qname.schema"
```

## Canonicalization

`Catalog::canonicalize()` delegates to `ir::canon::canonicalize`, which
runs four ordered passes:

1. **`filter_pg_defaults`** — IR field values that match PG's
   documented defaults become `None` (sequence min/max, function
   cost/rows, column `pg_catalog.default` collation). Both source-built
   and catalog-read `Catalog`s pass through this, so a function
   declared without `COST` round-trips byte-equal with the catalog
   reading of the same function.
2. **`sentinel_view_columns`** — view and materialized-view column
   types collapse to a shared `view_column` sentinel. Body changes are
   already captured by `body_canonical` (an AST hash); per-column
   types are redundant info derived from the body.
3. **`renumber_enum_sort_orders`** — each enum's `sort_order` values
   are re-indexed to `1.0, 2.0, 3.0, …` in current order. PG stores
   floats; source assigns sequential 1-indexed; this pass aligns the
   two.
4. **`sort_and_dedupe`** — sort each collection by its canonical key
   (`schema.name`, `qname`, etc.); reject duplicates. Runs last so
   duplicate detection sees the post-normalization state.

The output is **byte-stable**: identical inputs always produce identical
serialized output. This is what makes `PlanId` deterministic.

When PG returns a default we hadn't expected, the fix lands in one of
the four passes (most commonly `filter_pg_defaults`). Catalog readers
and source builders are kept "raw" — they never filter — so the rule
is discoverable in one place.

Failure modes (only `sort_and_dedupe` is fallible):

- `IrError::InvalidIdentifier("duplicate schema: foo")` — two `Schema`s
  with the same name.
- Same for tables / indexes / sequences / views / MVs / types /
  functions / procedures.

## `Diff` derive

Most IR structs derive their `Diff` impl rather than hand-writing it:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Sequence {
    pub qname: QualifiedName,           // default strategy (Display)
    #[diff(via_debug)]
    pub data_type: ColumnType,
    pub start: i64,
    // ...
    #[diff(via_debug)]
    pub min_value: Option<i64>,
    // ...
}
```

The derive (from the `pgevolve-core-macros` crate, re-exported as
`pgevolve_core::ir::eq::DiffMacro`) supports three field attributes:

- *no attribute* — emit `diff_field(name, &self.x, &other.x)`. Requires
  `PartialEq + Display`.
- `#[diff(skip)]` — omit the field entirely.
- `#[diff(via_debug)]` — emit `diff_field(name, format!("{:?}", ...))`.
  For `Option<T>`, `Vec<T>`, enums without `Display`.
- `#[diff(nested)]` — emit `prefix_diffs(name, self.x.diff(&other.x))`.
  For fields whose type already implements `Diff`.

Hand-written impls remain for: `Catalog` (orchestrates `diff_keyed`
over many `Vec<T>` collections), `Function` (custom `qname(args)` key
in path), `Table` / `View` / `MaterializedView` (pair columns by name
with order-drift reporting), `UserType` (intentional dump-all on
inequality), and the enum impls (`ConstraintKind`, `DefaultExpr`,
`ColumnType`).

## `Identifier` and `QualifiedName`

`Identifier` is a single SQL identifier. Two constructors:

- `Identifier::from_unquoted(s)` — accepts `[a-z_][a-z0-9_$]*` shapes;
  rejects anything that would need quoting. This is what the parser
  and the user-facing config consume.
- `Identifier::from_quoted(s)` — accepts the body of a
  double-quoted identifier (with `""` → `"` already unescaped).

`render_sql()` returns the canonical SQL form, quoting only when
necessary. Two `Identifier`s compare equal iff their `as_str()`
representations match.

`QualifiedName { schema, name }` — schema-qualified `schema.name`.
Used wherever a top-level object name appears.

## `ColumnType`

The single most fact-laden type in the IR. `ColumnType` is the
**canonical** form of a Postgres column type:

```rust
pub enum ColumnType {
    Boolean,
    SmallInt, Integer, BigInt,
    Real, DoublePrecision,
    Numeric { precision: Option<u16>, scale: Option<i16> },
    Text, Varchar { len: Option<u32> }, Char { len: Option<u32> },
    Bytea,
    Date,
    Time { precision: Option<u8>, with_tz: bool },
    Timestamp { precision: Option<u8>, with_tz: bool },
    Interval { fields: Option<String>, precision: Option<u8> },
    Bit { len: u32, varying: bool },
    Uuid, Json, Jsonb,
    NetAddress(NetAddressKind),
    Array { element: Box<ColumnType>, dims: u8 },
    UserDefined(QualifiedName),
    Other { raw: String },
}
```

### Why the canonical form matters

PG accepts many spellings for the same type:

| Source spelling | Catalog form | Canonical IR |
|---|---|---|
| `int` | `integer` | `ColumnType::Integer` |
| `int4` | `integer` | `ColumnType::Integer` |
| `decimal` | `numeric` | `ColumnType::Numeric { .. }` |
| `bool` | `boolean` | `ColumnType::Boolean` |
| `timestamptz` | `timestamp with time zone` | `ColumnType::Timestamp { with_tz: true, .. }` |
| `varchar` (no length) | `character varying` (no length) | `ColumnType::Varchar { len: None }` |

`ColumnType::parse_from_pg_type_string` does the source-side and
catalog-side normalization to the same canonical form, so diff
operates on equality rather than on textual difference.

### `Other` and `UserDefined` — the escape hatches

- `Other { raw: String }` — types pgevolve doesn't recognize. Treated
  as opaque strings; two `Other`s match iff their raw strings match
  byte-for-byte. This lets the parser handle types it doesn't
  understand instead of aborting.
- `UserDefined(QualifiedName)` — qualified references to user-defined
  types (enums, domains, composites). v0.1 doesn't introspect their
  structure; v0.2 will.

## `DefaultExpr`

```rust
pub enum DefaultExpr {
    Literal(LiteralValue),
    Sequence(QualifiedName),
    Expr(NormalizedExpr),
}
```

- `Literal` — a typed literal (bool, integer, float, text, bytea,
  NULL).
- `Sequence` — detected from `nextval('schema.seq'::regclass)` or the
  bare `nextval('schema.seq')`. Both forms normalize to the same
  `QualifiedName`.
- `Expr(NormalizedExpr)` — any other expression, preserved as
  canonical text.

### `NormalizedExpr`

```rust
pub struct NormalizedExpr {
    pub canonical_text: String,
    pub ast_hash:       [u8; 32],
}
```

The canonical text is what you get after:

- Lowercasing keywords.
- Sorting operands of commutative operators (`a + b` ≡ `b + a`).
- Stripping redundant casts (`'foo'::text` → `'foo'` if the column is
  already `text`).
- Folding redundant parens.

Two `NormalizedExpr`s compare equal iff their `canonical_text`s match.
The `ast_hash` is a BLAKE3 hash of the canonical text, kept for fast
equality checks and as a stable identity for an expression.

### `NormalizedBody`

```rust
pub struct NormalizedBody {
    pub canonical_text: String,
    pub canonical_hash: [u8; 32],
}
```

`NormalizedBody` is the statement-scope counterpart to `NormalizedExpr`.
Where `NormalizedExpr` canonicalizes a single expression (e.g., a column
default or a CHECK predicate), `NormalizedBody` canonicalizes the body
of a body-bearing object — a view's `SELECT` statement, a function's
body, a trigger's action. The same canonicalization rules apply (keyword
case, redundant parens, etc.), plus one cross-version normalization:
for any `SelectStmt` whose `FROM` names exactly one relation, the
canonicalizer rewrites `ColumnRef [<from-alias>, <col>]` → `[<col>]`.
PG14's `pg_get_viewdef` keeps the redundant qualifier while PG17 strips
it; this pass makes both forms equal so the differ doesn't see a
phantom body change on PG14.

In v0.1 no objects carry a body; `NormalizedBody` is scaffolding for
v0.2 views and functions. It is kept in `pgevolve-core::parse::normalize_body`
so body-bearing objects added by v0.2 sub-specs can reuse the same
diffing semantics as `NormalizedExpr`.

## `Table`

```rust
pub struct Table {
    pub qname:        QualifiedName,
    pub columns:      Vec<Column>,
    pub constraints:  Vec<Constraint>,
    pub partition_by: Option<PartitionBy>,   // partitioned parent
    pub partition_of: Option<PartitionOf>,   // partition child
    pub comment:      Option<String>,

    // --- v0.3 cross-cutting state ---------------------------------
    pub owner:        Option<Identifier>,    // v0.3.1: None = unmanaged
    pub grants:       Vec<Grant>,            // v0.3.1: empty = no grants
    pub rls_enabled:  bool,                  // v0.3.2: PG default false
    pub rls_forced:   bool,                  // v0.3.2: PG default false
    pub policies:     Vec<Policy>,           // v0.3.2: RLS policies
    pub storage:      TableStorageOptions,   // v0.3.3: WITH (…)
}
```

The first six fields are the v0.1/v0.2 surface (structure, columns,
constraints, partitioning, comment). The last six are
[v0.3 cross-cutting state](./architecture.md#v03-cross-cutting-state) —
each one follows the lenient-drift convention: `Option<T>` / `Vec<T>` /
`bool` where the *absent* / *empty* / *false* state means "unmanaged."

`Index` and `MaterializedView` carry a subset of the same v0.3 fields:

- `Index` adds `storage: IndexStorageOptions`.
- `MaterializedView` adds `owner`, `grants`, and `storage`
  (`MaterializedViewStorageOptions = TableStorageOptions`).

### `Grant`, `Policy`, `*StorageOptions`

Defined in `crates/pgevolve-core/src/ir/{grant,policy,reloptions}.rs`.

- `Grant { grantee, privileges, with_grant_option, columns: Option<Vec<Identifier>> }`.
  `columns = Some(_)` is a column-level grant on a table/view/MV; `None`
  is object-level. Canonicalized: stable sort by `(grantee, privileges)`.
- `Policy { name, command, permissive, roles, using, with_check }`.
  USING / WITH CHECK reuse `NormalizedExpr` (same canon as CHECK
  constraints). Command-kind changes (e.g., `FOR SELECT` → `FOR UPDATE`)
  diff as DROP + CREATE.
- `TableStorageOptions` / `IndexStorageOptions` — typed `Option<T>`
  fields for the well-known reloption keys (fillfactor, autovacuum_*,
  parallel_workers, fastupdate, buffering, pages_per_range, …) plus
  `extra: BTreeMap<String, String>` for extension or unknown keys. f64
  fields wrap `NotNanF64` so `Eq`/`Hash`/`Ord` work. Per-AM fillfactor
  ranges are enforced at parse time (see [`docs/spec/reloptions.md`](../spec/reloptions.md)).

## `Column` attributes

```rust
pub struct Column {
    pub name:      Identifier,
    pub ty:        ColumnType,
    pub nullable:  bool,
    pub default:   Option<DefaultExpr>,
    pub identity:  Option<Identity>,
    pub generated: Option<Generated>,
    pub collation: Option<QualifiedName>,
    pub comment:   Option<String>,
}
```

- `nullable = false` corresponds to `NOT NULL` — modeled as a
  column-level boolean rather than as a `Constraint` because it's
  significantly cheaper to diff.
- `identity` — `GENERATED ALWAYS / BY DEFAULT AS IDENTITY` with the
  backing sequence options.
- `generated` — `GENERATED ALWAYS AS (expr) STORED`. Postgres doesn't
  yet support `VIRTUAL`.
- `collation` — only the **explicit** collation; the catalog reader
  normalizes `pg_catalog.default` to `None` so it doesn't appear as
  drift on every text column.

## `Constraint`

```rust
pub struct Constraint {
    pub qname:      QualifiedName,
    pub kind:       ConstraintKind,
    pub deferrable: Deferrable,
    pub comment:    Option<String>,
}

pub enum ConstraintKind {
    PrimaryKey { columns: Vec<Identifier>, include: Vec<Identifier> },
    Unique     { columns: Vec<Identifier>, include: Vec<Identifier>, nulls_distinct: bool },
    ForeignKey(ForeignKey),
    Check      { expression: NormalizedExpr, no_inherit: bool },
}
```

Constraints are paired by `qname.name` (within a table) during diff.
Two constraints with the same name but different bodies diff as a
"replace" — pgevolve emits `DROP CONSTRAINT` + `ADD CONSTRAINT`.

## `Index`

```rust
pub struct Index {
    pub qname:              QualifiedName,
    pub table:              QualifiedName,
    pub method:             IndexMethod,
    pub columns:            Vec<IndexColumn>,
    pub include:            Vec<Identifier>,
    pub unique:             bool,
    pub nulls_not_distinct: bool,
    pub predicate:          Option<NormalizedExpr>,
    pub tablespace:         Option<Identifier>,
    pub comment:            Option<String>,
}
```

Indexes are first-class IR objects (paired by their own qname, not by
their backing table). This makes "rename the index" or "change the
opclass on column 2" a single-row diff entry.

## `View` and `MaterializedView`

Added in v0.2. Source: `crates/pgevolve-core/src/ir/view.rs`.

```rust
pub struct View {
    pub qname:              QualifiedName,
    pub columns:            Vec<ViewColumn>,
    pub body_canonical:     NormalizedBody,
    pub body_dependencies:  Vec<DepEdge>,
    pub security_barrier:   Option<bool>,
    pub security_invoker:   Option<bool>,
    pub comment:            Option<String>,
    // raw_body: parser-internal sentinel; not serialized.
}

pub struct MaterializedView {
    pub qname:              QualifiedName,
    pub columns:            Vec<ViewColumn>,
    pub body_canonical:     NormalizedBody,
    pub body_dependencies:  Vec<DepEdge>,
    pub comment:            Option<String>,
    // raw_body: parser-internal sentinel; not serialized.
}
```

### `body_canonical: NormalizedBody`

The canonicalized SELECT body. `NormalizedBody::from_sql` (in
`parse/normalize_body.rs`) feeds the raw SQL through `pg_query`'s
parse + deparse cycle, strips redundant table-qualifier prefixes from
column refs whose FROM names a single relation (see `NormalizedBody`
above), and collapses whitespace. The same function is called on the
source side (T3/T4 parse pass) and the catalog side (T5 reader, which
calls `pg_get_viewdef`). Because both sides go through the same
normalization, the differ compares canonical texts directly without
knowing anything about SQL semantics.

`canonical_hash` (BLAKE3 of the text, domain-separated with
`pgevolve-normalized-body-v1\n`) is kept for fast equality checks and
stable identity.

### `body_dependencies: Vec<DepEdge>`

Dependency edges extracted from the body AST by the T4 AST canonicalization
pass (`parse/ast_canon.rs`). Each `DepEdge` has:

```rust
pub struct DepEdge {
    pub from:   NodeId,         // NodeId::View or NodeId::Mv
    pub to:     NodeId,         // NodeId::Table, NodeId::View, or NodeId::Mv
    pub source: DepSource,      // DepSource::AstExtracted
}
```

`body_dependencies` is what makes the planner's dependent-recreation walk
possible (see `plan/recreate_views.rs`). It is also what the
`view-body-references-unmanaged-schema` lint rule checks.

## `ViewColumn`

```rust
pub struct ViewColumn {
    pub name:        Identifier,
    pub column_type: ColumnType,
    pub comment:     Option<String>,
}
```

A single named column in a view or materialized view. When constructed
from the source parser (T3), `column_type` is set to
`ColumnType::Other { raw: "unresolved" }` as a sentinel; the T4 AST
canonicalization pass fills in the resolved type. When built from the
live catalog (T5), `column_type` is parsed from
`format_type(a.atttypid, a.atttypmod)`.

## `UserType`

```rust
pub struct UserType {
    pub qname:   QualifiedName,
    pub kind:    UserTypeKind,
    pub comment: Option<String>,
}

pub enum UserTypeKind {
    Enum      { values:     Vec<EnumValue> },
    Domain    { base: ColumnType, nullable: bool, default: Option<NormalizedExpr>,
                check_constraints: Vec<DomainCheck>, collation: Option<QualifiedName> },
    Composite { attributes: Vec<CompositeAttribute> },
}
```

`UserType`s live in `Catalog::types: Vec<UserType>`, sorted by `qname` after
`canonicalize()`. Source lives in `crates/pgevolve-core/src/ir/user_type.rs`.

### `EnumValue`

```rust
pub struct EnumValue {
    pub name:       String,
    pub sort_order: f32,   // mirrors pg_enum.enumsortorder
}
```

`sort_order` is `f32` (matching Postgres's `real4`) to enable byte-stable
round-trip. `Eq` and `Hash` are implemented using the IEEE 754 bit pattern.

### `DomainCheck`

```rust
pub struct DomainCheck {
    pub name:       Identifier,
    pub expression: NormalizedExpr,
}
```

Domain defaults and CHECK expressions use `NormalizedExpr` — the same
canonicalized-text representation as column defaults and inline CHECK
constraints. Two `NormalizedExpr`s compare equal iff their `canonical_text`s
match, making domain diffs insensitive to whitespace and keyword case.

### `CompositeAttribute`

```rust
pub struct CompositeAttribute {
    pub name:      Identifier,
    pub ty:        ColumnType,
    pub collation: Option<QualifiedName>,
}
```

## `Function`

Added in v0.2. Source: `crates/pgevolve-core/src/ir/function.rs`.

```rust
pub struct Function {
    pub qname:                 QualifiedName,
    pub args:                  Vec<FunctionArg>,
    pub arg_types_normalized:  NormalizedArgTypes,
    pub return_type:           ReturnType,
    pub language:              FunctionLanguage,
    pub body:                  NormalizedBody,
    pub body_dependencies:     Vec<DepEdge>,
    pub volatility:            Volatility,
    pub strict:                bool,
    pub security:              SecurityMode,
    pub parallel:              ParallelSafety,
    pub leakproof:             bool,
    pub cost:                  Option<f32>,
    pub rows:                  Option<f32>,
    pub comment:               Option<String>,
}
```

`Function`s live in `Catalog::functions: Vec<Function>`, sorted by `(qname, arg_types_normalized)` after `canonicalize()`.

### Identity rule

Function identity is `(qname, arg_types_normalized)`. `arg_types_normalized` covers the IN/INOUT/VARIADIC args only (mirrors Postgres's `proargtypes`), enabling overloads with the same name but different input signatures to coexist.

`NormalizedArgTypes` stores the canonical type list and a BLAKE3 hash of the comma-joined type strings for fast equality and ordering.

### `body_dependencies: Vec<DepEdge>`

Dependency edges extracted from the body AST by the T4 body parser (`parse/builder/plpgsql.rs`):

- SQL bodies: extracted from `RangeVar` nodes in the SQL AST (schema-qualified references only).
- PL/pgSQL bodies: extracted from static embedded SQL statements (`PLpgSQL_stmt_execsql`). Dynamic SQL (`EXECUTE`) edges must be declared explicitly via `-- @pgevolve dep: schema.name` directives; undeclared `EXECUTE` sites fire the `plpgsql-dynamic-sql` lint rule.

`DepEdge.source` is `DepSource::AstExtracted` for parsed edges and `DepSource::AstDeclared` for directive edges.

## `Procedure`

Added in v0.2. Source: `crates/pgevolve-core/src/ir/procedure.rs`.

```rust
pub struct Procedure {
    pub qname:             QualifiedName,
    pub args:              Vec<FunctionArg>,
    pub language:          FunctionLanguage,
    pub body:              NormalizedBody,
    pub body_dependencies: Vec<DepEdge>,
    pub security:          SecurityMode,
    pub commits_in_body:   bool,
    pub comment:           Option<String>,
}
```

`Procedure`s live in `Catalog::procedures: Vec<Procedure>`, sorted by `qname` after `canonicalize()`.

### Identity rule

Procedure identity is `qname` only (no arg-type disambiguation). pgevolve v0.2 deliberately restricts procedures to a single definition per qualified name. This simplifies the plan-format and intent model; overloading can be added in a future sub-spec.

### `commits_in_body`

Set to `true` by the PL/pgSQL body parser when it detects `PLpgSQL_stmt_commit` or `PLpgSQL_stmt_rollback` nodes anywhere in the body AST (including inside `IF`, `LOOP`, etc.). The planner uses this flag to emit the step with `TransactionConstraint::OutsideTransaction`, since a procedure containing `COMMIT`/`ROLLBACK` cannot run inside an outer `BEGIN … COMMIT` block.

## `Sequence`

```rust
pub struct Sequence {
    pub qname:     QualifiedName,
    pub data_type: ColumnType,    // SmallInt / Integer / BigInt
    pub start:     i64,
    pub increment: i64,
    pub min_value: Option<i64>,   // None == PG type-default
    pub max_value: Option<i64>,
    pub cache:     i64,
    pub cycle:     bool,
    pub owned_by:  Option<SequenceOwner>,
    pub comment:   Option<String>,
}
```

The catalog reader normalizes PG's per-type defaults for `min_value` /
`max_value` to `None` (the same reasoning as collation normalization).

## What's deliberately not in the IR

- **`NOT VALID` constraints.** The IR represents only fully-validated
  constraints; `NOT VALID` is an intermediate planner artifact.
- **Auto-generated index names.** All indexes must be named in
  source. Constraint-backing indexes are tied to the constraint, not
  modeled as separate `Index`es.
- **Row data.** pgevolve never reads or writes table contents.
- **`postgresql.conf` settings.** Roles *are* in scope as of v0.3.0
  via the separate `ClusterCatalog`; cluster-level GUCs and
  tablespaces remain out of scope.
- **`pg_catalog` / `information_schema`** — unmanaged schemas don't
  appear in the IR.

## How the diff walks the IR

`Catalog::diff(other) -> Vec<Difference>`:

1. Pair-by-key over each top-level collection (`schemas`, `tables`,
   `indexes`, `sequences`).
2. For paired objects, recurse into nested collections (columns,
   constraints).
3. For each unmatched-on-the-left key, emit `present → removed`.
4. For each unmatched-on-the-right key, emit `missing → added`.
5. For each matched pair, recurse into the per-field diff.

The output `Vec<Difference>` is **flat**: every leaf change has a
slash-or-dot path like `tables.app.users.columns.email.nullable`. The
linter uses this for findings; the differ converts it into a
`ChangeSet` of higher-level `Change` enum variants.
