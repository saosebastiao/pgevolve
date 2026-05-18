# pgevolve v0.2 sub-spec #2 — User-defined types

- **Status:** draft, awaiting review
- **Date:** 2026-05-18
- **Authors:** Daniel Toone
- **Builds on:**
  [arch-readiness](./2026-05-15-v0.2-architecture-review-design.md) (decisions 1, 4),
  [views/MVs sub-spec](./2026-05-11-views-and-materialized-views-design.md) (the dependent-recreation pattern).

## 1. Scope

Adds three user-defined type kinds to the managed surface:

- **Enums** (`CREATE TYPE x AS ENUM (...)`)
- **Domains** (`CREATE DOMAIN x AS <base_type> [NOT NULL] [DEFAULT ...] [CONSTRAINT ... CHECK (...)]`)
- **Composite types** (`CREATE TYPE x AS (a int, b text, ...)`)

All three live under a unified `UserType` IR record on `Catalog.types`. Columns already reference user types via the existing `ColumnType::UserDefined(QualifiedName)` variant; this sub-spec resolves those references and tracks the types they point at.

### In scope

| Feature | Notes |
|---|---|
| `CREATE TYPE x AS ENUM` | Ordered value list. |
| `ALTER TYPE x ADD VALUE [BEFORE \| AFTER]` | PG 14+ commits atomically. |
| `ALTER TYPE x RENAME VALUE 'old' TO 'new'` | Metadata-only. |
| `CREATE DOMAIN` | Base type, NOT NULL, DEFAULT, CHECK constraints, COLLATE. |
| `ALTER DOMAIN ... ADD CONSTRAINT ... CHECK` | Validating; runs against existing data. |
| `ALTER DOMAIN ... DROP CONSTRAINT` | Loosening — safe. |
| `ALTER DOMAIN ... SET DEFAULT` / `DROP DEFAULT` | Metadata-only. |
| `ALTER DOMAIN ... SET NOT NULL` / `DROP NOT NULL` | `SET NOT NULL` validates against existing rows. |
| `CREATE TYPE x AS (...)` | Composite — ordered attributes. |
| `ALTER TYPE ... ADD ATTRIBUTE` | Triggers table rewrite for tables holding the composite. |
| `ALTER TYPE ... DROP ATTRIBUTE` | Destructive — requires `[[intent]]`. |
| `ALTER TYPE ... ALTER ATTRIBUTE ... TYPE` | Triggers table rewrite — requires `[[intent]]`. |
| `COMMENT ON TYPE` / `COMMENT ON DOMAIN` | Reuses the existing comment-step machinery. |
| Cascade-recreate fallback | For changes PG cannot ALTER (enum value drop/reorder, composite attribute reorder, kind change), emit `DROP TYPE + CREATE TYPE` plus DROP+ADD for dependent columns. Each dropped column requires `[[intent]]` approval. |
| Column-side reference resolution | AST resolution pass verifies every `ColumnType::UserDefined(qname)` resolves to a declared `UserType`. |

### Deferred (future sub-specs)

| Feature | Reason |
|---|---|
| `RANGE TYPE` (`CREATE TYPE ... AS RANGE`) | `🔮 Future`. Lands with range-typed columns. |
| `BASE TYPE` (`CREATE TYPE ... ( INPUT = ..., OUTPUT = ... )`) | `⛔ Not planned`. Requires C-language functions. |
| Enum value reordering via temp-text-column dance | Too clever for v0.2. Differ surfaces it as `ReplaceWithCascade`; manual orchestration is the workaround. |
| `ALTER TYPE ... ADD VALUE ... IF NOT EXISTS` | pgevolve's diff is already idempotent. |
| Per-column drop-and-re-add semantics for `ReplaceWithCascade` | v0.2 drops dependent columns (data loss) under explicit `[[intent]]`. Preserving column data via shadow-column dance is out of scope. |

## 2. Key design decisions

| Decision | Choice | Rationale |
|---|---|---|
| **IR shape for user types** | One `UserType { qname, kind: UserTypeKind, comment }` with `UserTypeKind` discriminating enum/domain/composite. | Flat collection on `Catalog`, mirrors v0.2 readiness decision 1. Three separate top-level vectors would force every consumer (parser, differ, planner) to handle the same iteration shape three times. |
| **Identity** | `QualifiedName` only. | Arch readiness decision 2 — only functions need signature-keyed identity. Types are unique per qname within a schema. |
| **Reference resolution** | AST resolution pass (existing) gains a `UserDefined` resolver. | Mirrors FK resolution. Catches dangling references at parse time, before diff. |
| **Type-to-type cycles** | Detected by the planner's dep graph; surface as `PlanError::BodyCycle`. | The existing cycle-detection path already handles this — composite A embedding composite B that embeds A would otherwise produce an unbreakable ordering. |
| **Enum value sort order** | Float (matches PG's `pg_enum.enumsortorder`). | PG stores enum values with a float sort key so `ADD VALUE BEFORE x` can insert between existing values without renumbering. Source-side we assign 1.0, 2.0, ... in declaration order; catalog-side preserves whatever PG produced. |
| **Cascade-recreate trigger condition** | Differ-side compatibility predicates (`enum_can_alter_in_place`, `composite_can_alter_in_place`). | When false, emit `ReplaceWithCascade`. Predicate returns true when ALL needed changes are reachable via supported `ALTER` operations. |
| **Default destructiveness** | `Drop`, `DomainDropCheck`, `CompositeDropAttribute`, `ReplaceWithCascade` are destructive. `CompositeAlterAttributeType` requires approval (table rewrite). Everything else is safe. | Each destructive step gets an `[[intent]]` row. Mirrors how v0.1 column drops + v0.2 view drops work. |
| **NOT VALID for domain checks** | Not modeled — domain CHECKs are always validating when added. | Domain CHECKs are typically cheap (per-row validation on insert); a `NOT VALID` workflow would require tracking transient state in source, which arch decision 4 forbids. v0.3 can revisit if real users hit the wall. |
| **Online rewrite policy toggle** | None for v0.2. Cascade is unconditional when the predicate fails. | The user-asked-for behavior (per brainstorming). A `type_drop_create_dependents` toggle (mirroring views) could be added later if there's a real need. |

## 3. IR additions

`crates/pgevolve-core/src/ir/user_type.rs` (new):

```rust
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct UserType {
    pub qname: QualifiedName,
    pub kind: UserTypeKind,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserTypeKind {
    Enum { values: Vec<EnumValue> },
    Domain {
        base: ColumnType,
        nullable: bool,
        default: Option<NormalizedExpr>,
        check_constraints: Vec<DomainCheck>,
        collation: Option<QualifiedName>,
    },
    Composite { attributes: Vec<CompositeAttribute> },
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct EnumValue {
    pub name: String,
    /// PG's `pg_enum.enumsortorder` is a real4. Serialized as f32; bit-equal
    /// across runs because both source-side (1.0, 2.0, ...) and catalog-side
    /// (whatever PG produced) use exact representable values.
    pub sort_order: f32,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DomainCheck {
    pub name: Identifier,
    pub expression: NormalizedExpr,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CompositeAttribute {
    pub name: Identifier,
    pub ty: ColumnType,
    pub collation: Option<QualifiedName>,
}
```

`Catalog` gains `pub types: Vec<UserType>`. `canonicalize()` sorts by qname and rejects duplicates, same pattern as the other collections.

The existing `ColumnType::UserDefined(QualifiedName)` variant is unchanged. It's the IR's reference to a user type; this sub-spec adds the resolution machinery.

## 4. Source pipeline

### 4.1 Parser

Three new builders in `crates/pgevolve-core/src/parse/builder/`:

- `create_enum_stmt.rs` — handles `CreateEnumStmt`. Extracts qname, values list. Assigns `sort_order = 1.0, 2.0, ...` in source order. Rejects duplicate values via the existing `Catalog::canonicalize` duplicate check.

- `create_domain_stmt.rs` — handles `CreateDomainStmt`. Extracts qname, base type via `ColumnType::parse_from_pg_type_string`, default expression via `NormalizedExpr::from_sql`, CHECK constraint list. `NOT NULL` parsed from the statement's `constraints` array.

- `create_composite_type_stmt.rs` — handles `CompositeTypeStmt`. Extracts qname, attribute list (each with name + type + collation).

The dispatcher in `parse/statement.rs` routes each pg_query statement variant to the right builder. `ALTER TYPE` / `ALTER DOMAIN` in source are rejected as unsupported (same v0.1 invariant: source contains only CREATE statements).

### 4.2 AST resolution

`parse/ast_resolution.rs` gains a new resolver: `resolve_user_defined_column_types`.

Walks every:
- `Table.columns` — each column whose `ColumnType` is `UserDefined(qname)`
- `UserType` whose kind is `Domain { base, .. }` — when `base` is `UserDefined(qname)`
- `UserType` whose kind is `Composite { attributes, .. }` — each attribute whose type is `UserDefined(qname)`
- (Future: function argument and return types when functions land)

For each `UserDefined(qname)` reference, asserts the qname exists in `catalog.types`. Unresolved references surface as `AstResolutionError` with the source location of the referencing object.

### 4.3 Failure-mode tiers

| Tier | Triggered when | Example |
|---|---|---|
| Parse | `ALTER TYPE` or unknown statement variant appears in source | `ALTER TYPE app.status ADD VALUE 'pending'` |
| AST resolution | A column / attribute / domain base type references a `UserDefined(qname)` not declared in source | `CREATE TABLE app.orders (status app.order_status NOT NULL)` without a matching `CREATE TYPE app.order_status` |
| Order (planner) | A type cycle exists | Composite `A` has attribute of type composite `B` that has attribute of type composite `A` |
| Lint at plan | Column position drift on a table that uses a user type | Existing `column-position-drift` rule; unchanged |

## 5. Catalog reader

New file `crates/pgevolve-core/src/catalog/queries/types.rs` with four SELECT strings:

```sql
-- SELECT_USER_TYPES — enumerate
SELECT
    n.nspname  AS schema_name,
    t.typname  AS name,
    t.typtype  AS kind,                       -- 'e' | 'd' | 'c'
    obj_description(t.oid, 'pg_type') AS comment
FROM pg_type t
JOIN pg_namespace n ON t.typnamespace = n.oid
WHERE t.typtype IN ('e','d','c')
  AND n.nspname = ANY($1::text[])
  -- Exclude row types auto-generated for tables. A composite type backing
  -- an actual user-defined composite has its pg_class row at relkind='c';
  -- a row type for a table has relkind='r' or 'v'/'m'.
  AND NOT (t.typtype = 'c' AND EXISTS (
      SELECT 1 FROM pg_class c
      WHERE c.oid = t.typrelid AND c.relkind <> 'c'
  ))
ORDER BY n.nspname, t.typname;

-- SELECT_ENUM_VALUES — per-type detail
SELECT
    n.nspname            AS schema_name,
    t.typname            AS type_name,
    e.enumlabel          AS value_name,
    e.enumsortorder      AS sort_order
FROM pg_enum e
JOIN pg_type t ON e.enumtypid = t.oid
JOIN pg_namespace n ON t.typnamespace = n.oid
WHERE n.nspname = ANY($1::text[])
ORDER BY n.nspname, t.typname, e.enumsortorder;

-- SELECT_DOMAIN_DETAILS — base type, default, NOT NULL, collation
SELECT
    n.nspname                                         AS schema_name,
    t.typname                                         AS name,
    format_type(t.typbasetype, t.typtypmod)           AS base_type,
    t.typnotnull                                       AS not_null,
    t.typdefault                                       AS default_expr,
    coll_n.nspname                                     AS collation_schema,
    coll.collname                                      AS collation_name
FROM pg_type t
JOIN pg_namespace n ON t.typnamespace = n.oid
LEFT JOIN pg_collation coll ON t.typcollation = coll.oid AND t.typcollation <> 0
LEFT JOIN pg_namespace coll_n ON coll.collnamespace = coll_n.oid
WHERE t.typtype = 'd'
  AND n.nspname = ANY($1::text[]);

-- SELECT_DOMAIN_CHECKS — domain CHECK constraints
SELECT
    n.nspname                                  AS schema_name,
    t.typname                                  AS type_name,
    c.conname                                  AS constraint_name,
    pg_get_constraintdef(c.oid, true)          AS expression
FROM pg_constraint c
JOIN pg_type t ON c.contypid = t.oid
JOIN pg_namespace n ON t.typnamespace = n.oid
WHERE t.typtype = 'd'
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, t.typname, c.conname;

-- SELECT_COMPOSITE_ATTRIBUTES
SELECT
    n.nspname                                         AS schema_name,
    t.typname                                         AS type_name,
    a.attname                                         AS attribute_name,
    format_type(a.atttypid, a.atttypmod)              AS attribute_type,
    coll_n.nspname                                    AS collation_schema,
    coll.collname                                     AS collation_name,
    a.attnum                                          AS attnum
FROM pg_attribute a
JOIN pg_class c ON a.attrelid = c.oid
JOIN pg_type t ON c.reltype = t.oid
JOIN pg_namespace n ON t.typnamespace = n.oid
LEFT JOIN pg_collation coll ON a.attcollation = coll.oid AND a.attcollation <> 0
LEFT JOIN pg_namespace coll_n ON coll.collnamespace = coll_n.oid
WHERE t.typtype = 'c'
  AND c.relkind = 'c'                                 -- composite, not table-row
  AND a.attnum > 0
  AND NOT a.attisdropped
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, t.typname, a.attnum;
```

The assembler in `catalog/assemble.rs` runs all four queries, joins on `(schema, type_name)`, dispatches by `kind`, and builds `UserType` records. Base types and attribute types parse via the existing `ColumnType::parse_from_pg_type_string`. Domain CHECK expressions parse via `NormalizedExpr::from_sql` (same path as table-column defaults).

`read_catalog` still returns `(Catalog, DriftReport)`. Per the arch spec, `EnumValueUncommitted` would be a drift kind; PG 14+ commits enum values atomically so this state cannot occur on supported versions. No detection logic ships in v0.2; the variant is reserved for future use.

## 6. Differ

New file `crates/pgevolve-core/src/diff/types.rs`. Pair-by-qname over `catalog.types`. Kind mismatches (catalog `Enum` vs source `Composite` at the same qname) emit `ReplaceWithCascade` — never an in-place ALTER across kinds.

### 6.1 Change variants

```rust
pub enum Change {
    // ...existing...
    UserType(UserTypeChange),
}

pub enum UserTypeChange {
    Create(UserType),
    Drop(QualifiedName),

    // In-place ALTERs (PG can do these directly)
    EnumAddValue {
        qname: QualifiedName,
        value: String,
        before: Option<String>,
        after: Option<String>,
    },
    EnumRenameValue { qname: QualifiedName, from: String, to: String },

    DomainAddCheck { qname: QualifiedName, constraint: DomainCheck },
    DomainDropCheck { qname: QualifiedName, name: Identifier },
    DomainSetDefault { qname: QualifiedName, default: Option<NormalizedExpr> },
    DomainSetNotNull { qname: QualifiedName, not_null: bool },

    CompositeAddAttribute { qname: QualifiedName, attribute: CompositeAttribute },
    CompositeDropAttribute { qname: QualifiedName, name: Identifier },
    CompositeAlterAttributeType {
        qname: QualifiedName,
        attribute: Identifier,
        new_type: ColumnType,
    },

    SetComment { qname: QualifiedName, comment: Option<String> },

    // Cascade-recreate fallback
    ReplaceWithCascade { source: UserType, catalog: UserType },
}
```

### 6.2 Compatibility predicates

`enum_can_alter_in_place(catalog: &[EnumValue], source: &[EnumValue]) -> bool`:

- The source value-name set must be a non-shrinking superset of the catalog set.
- For values present on both sides, the *relative* order in source must match the relative order in catalog (PG can `ADD VALUE BEFORE x` to insert at any position, but cannot move existing values past each other).
- Rename detection is separate: a value present in catalog but not source AND a value present in source but not catalog AT THE SAME POSITION is treated as a rename. (This heuristic can be tightened in a later sub-spec; for v0.2 the user can always rename explicitly via the diff loop's manual override if the heuristic gets it wrong.)
- If predicate returns false, emit `ReplaceWithCascade`.

`composite_can_alter_in_place(catalog: &[CompositeAttribute], source: &[CompositeAttribute]) -> bool`:

- Source attributes can be reached via a sequence of `ADD ATTRIBUTE`, `DROP ATTRIBUTE`, `ALTER ATTRIBUTE TYPE` against the catalog list.
- Reordering attributes is NOT supported by PG; if the relative order of attributes present on both sides differs, returns false.

Domains have no compatibility predicate — every domain change PG actually supports is in-place. Kind change is the only `ReplaceWithCascade` trigger for domains.

### 6.3 Destructiveness

| Change | Tag |
|---|---|
| `Create`, `EnumAddValue`, `EnumRenameValue`, `DomainAddCheck`, `DomainSetDefault`, `DomainSetNotNull { not_null: false }`, `CompositeAddAttribute`, `SetComment` | `Safe` |
| `DomainDropCheck`, `DomainSetNotNull { not_null: true }` | `Safe` (validates against existing rows but data-preserving) |
| `Drop` | `RequiresApprovalAndDataLossWarning` (cascades to dependent columns) |
| `CompositeDropAttribute` | `RequiresApprovalAndDataLossWarning` (drops a field from rows that use the composite) |
| `CompositeAlterAttributeType` | `RequiresApproval` (table rewrite cost; data preserved) |
| `ReplaceWithCascade` | `RequiresApprovalAndDataLossWarning` (dependent columns get dropped) |

## 7. Planner

### 7.1 New step kinds

```rust
pub enum StepKind {
    // ...existing...
    CreateType,
    DropType,
    AlterTypeAddValue,
    AlterTypeRenameValue,
    AlterDomainAddConstraint,
    AlterDomainDropConstraint,
    AlterDomainSetDefault,
    AlterDomainSetNotNull,
    AlterTypeAddAttribute,
    AlterTypeDropAttribute,
    AlterTypeAlterAttributeType,
    CommentOnType,
}
```

### 7.2 SQL emission

`crates/pgevolve-core/src/plan/rewrite/types.rs` (new) — straightforward `format!` builders, one per UserTypeChange variant. Same pattern as `rewrite/views.rs`.

Emission notes:

- `CreateType` for Composite: `CREATE TYPE x AS (a int, b text, ...)`.
- `CreateType` for Enum: `CREATE TYPE x AS ENUM ('a', 'b', 'c')` — values in `sort_order`.
- `CreateType` for Domain: `CREATE DOMAIN x AS base [NOT NULL] [DEFAULT ...] [CONSTRAINT name CHECK (expr)] [, ...]`.
- `AlterTypeAddValue`: `ALTER TYPE x ADD VALUE 'new' [BEFORE 'existing' | AFTER 'existing']`.
- `ReplaceWithCascade`: never emitted as a single step — the planner expands it into the dependent-recreation walk (§7.3), which produces explicit drop/recreate steps for the type and every dependent column.

### 7.3 Dependent-recreation walk

`recreate_views::extend_with_dependent_recreations` is renamed `extend_with_dependent_recreations` (already generic enough). Triggers expand to include:

- `UserTypeChange::Drop` → trigger any object that depends on this type:
  - Tables with columns whose `ColumnType` is `UserDefined(qname)` → emit `Change::DropColumn` for each
  - Composites that embed this type → emit `UserTypeChange::CompositeDropAttribute` (and recursively trigger their dependents)
  - Domains using this type as base → emit `UserTypeChange::Drop` (recursively triggers)
  - Views/MVs whose `body_dependencies` reference this type → emit `ViewChange::Drop` (or recreate with new body)
- `UserTypeChange::ReplaceWithCascade` → same triggers, but after the recreate, re-add the dropped columns/attributes against the new type.
- `UserTypeChange::CompositeDropAttribute` → trigger views whose body_dependencies are column-level against the dropped attribute. (v0.2 uses object-level dep granularity; the trigger is conservative — any view depending on the composite gets recreated.)
- `UserTypeChange::CompositeAlterAttributeType` → same conservative trigger.

The walk is topologically ordered: drops cascade leaves-first; recreations roots-first.

### 7.4 Dependency graph

`crates/pgevolve-core/src/plan/edges.rs` gains:

- New `NodeId::Type(QualifiedName)` variant. All consumer match sites (lint, ordering, recreate_views, ast_canon, render_node) extended.
- Edge kinds:
  - Column → Type (when column's `ColumnType` is `UserDefined`)
  - Composite Attribute → Type (when attribute's type is `UserDefined`)
  - Domain → Base Type (when base is `UserDefined`)
  - View body_dependencies → Type (when body references a `UserDefined` column)

Type-to-type cycles are detected by the existing `PlanError::BodyCycle` machinery — no new error variant.

## 8. Lints

Four new universal rules in `crates/pgevolve-core/src/lint/universal.rs`:

| Rule | Severity | Trigger |
|---|---|---|
| `type-shadows-table` | Error | A `UserType` qname collides with a table, view, or MV in the same schema. PG uses a single namespace for relations and types; the collision is an error. |
| `enum-value-collision` | Error | Two enum values in the same enum share a name. Backed by `Catalog::canonicalize`'s duplicate-detection; the lint surfaces the error with a friendlier message. |
| `domain-check-references-unmanaged-type` | Warning | A domain's CHECK expression references a user type whose schema is not in `[managed].schemas`. Mirrors `view-body-references-unmanaged-schema`. |
| `composite-attribute-collision` | Error | Duplicate attribute names within a composite. Same shape as `enum-value-collision`. |

None are `LintAtPlan` severity — they all fire at parse time.

## 9. Documentation updates

After implementation:

- `docs/spec/objects.md`: flip ENUM, DOMAIN, COMPOSITE TYPE rows from 📋 to ✅. Add `change_kinds:` annotations.
- `docs/spec/lint-and-layout.md`: document the 4 new rules.
- `docs/user/plan-format.md`: document the 12 new step kinds.
- `docs/user/cookbook.md`: add "Managing user-defined types" section. Worked examples: add an enum value (`ADD VALUE`); rename it (`RENAME VALUE`); replace an enum that needs cascading column drops (`ReplaceWithCascade` workflow).
- `docs/system/ir.md`: document `UserType`, `UserTypeKind`, `EnumValue`, `DomainCheck`, `CompositeAttribute`.
- `docs/system/planner.md`: document the compatibility predicates and the extended dep graph.
- `CHANGELOG.md`: extend `[0.2.0]` section with the types entries.
- `README.md`: sub-spec table — flip #2 (types) to ✅.

## 10. Testing

### 10.1 Conformance fixtures (~18)

**Enums** (`objects/enums/`):
- `create-simple` — basic CREATE TYPE AS ENUM
- `add-value` — `ALTER TYPE ADD VALUE` at end
- `add-value-before` — `ALTER TYPE ADD VALUE 'x' BEFORE 'existing'`
- `add-value-after` — `ALTER TYPE ADD VALUE 'x' AFTER 'existing'`
- `rename-value`
- `drop` (intent-gated)
- `cascade-recreate-on-value-removal` (intent + cascade-drops dependent column)

**Domains** (`objects/domains/`):
- `create-simple`
- `add-check-constraint`
- `drop-check-constraint`
- `set-default`, `drop-default`
- `toggle-not-null`
- `drop` (intent-gated)

**Composites** (`objects/composites/`):
- `create-simple`
- `add-attribute`
- `drop-attribute` (intent-gated)
- `alter-attribute-type` (intent — table rewrite cost flagged)

**Scenarios** (`scenarios/`):
- `type-used-by-table-column` — drop a column whose type is `UserDefined(some_enum)`; confirm the cascade behavior matches expectations
- `composite-with-nested-composite` — composite A embeds composite B; ALTER B; A's storage updates correctly

### 10.2 Property tests (nightly, `#[ignore]`'d)

1. `enum_add_value_preserves_existing_values` — for a random initial enum and a random `ADD VALUE` operation, post-apply value list is a non-shrinking superset of the pre-apply list in the expected sort order.

2. `domain_constraint_round_trip` — for a random domain CHECK expression, source → apply → catalog round-trips byte-equal canonical form (mirrors the view body invariant).

### 10.3 Tier-3 catalog goldens

Add enum, domain, and composite fixtures to the existing tier-3 catalog-snapshot suite. Regenerable via `cargo xtask bless`. Verifies the catalog reader produces stable IR across PG 14/15/16/17.

## 11. Open questions

- **Enum value rename heuristic.** When the differ sees catalog `{a, b, c}` and source `{a, x, c}`, it could be a rename (`b` → `x`) or a drop-of-`b` + add-of-`x`. v0.2 uses position-pairing: same index, different name → rename. A more robust approach would consult source for explicit `-- @pgevolve rename: b -> x` directives; deferred to v0.3 if heuristic mis-pairs in practice.

- **Domain CHECK expression normalization across PG versions.** PG's `pg_get_constraintdef` formatting differs minimally between versions. `NormalizedExpr::from_sql` is meant to canonicalize away formatting differences; verify in the round-trip property test against PG 14/15/16/17.

- **`COMMENT ON TYPE` vs `COMMENT ON DOMAIN`.** PG uses different SQL forms (`COMMENT ON TYPE name` for enums and composites; `COMMENT ON DOMAIN name` for domains). The emitter dispatches on `UserTypeKind`.

## 12. Phasing (informational)

Implementation plan will follow the v0.2-views pattern: 12-14 tasks across IR, parser, AST resolution, catalog reader, differ, planner step kinds, dependent recreation, lints, conformance fixtures, documentation. Estimated 15-20 commits.
