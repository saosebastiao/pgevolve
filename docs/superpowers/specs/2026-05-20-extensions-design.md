# v0.2 sub-spec #3: Extensions — design

**Status:** Approved 2026-05-20. Implementation plan to follow.

## Goal

First-class management of Postgres extensions: `CREATE EXTENSION`,
`ALTER EXTENSION ... UPDATE`, `DROP EXTENSION CASCADE`. Source declares
the extension prerequisite in SQL; the differ + planner pipeline emits
the minimal apply sequence. Objects installed by extensions
(operators, functions, types, etc.) are excluded from managed scope so
they never appear as drift.

Validates **Decision 20** of the v0.2 arch-readiness spec: extension
management surfaces the prerequisite, not the workflow. Internal
objects an extension installs are *owned by the extension*, not
managed by pgevolve.

## Non-goals

- A `[extensions]` block in `pgevolve.toml`. Extensions declare in SQL
  files identically to every other v0.2 object kind. Per the
  brainstorm decision: SQL is the single source of truth.
- `ALTER EXTENSION ADD/DROP MEMBER` (managing custom objects inside an
  extension). Out of scope for v0.2.
- Per-version update scripts. PG owns those; we just emit
  `ALTER EXTENSION ... UPDATE TO 'v'`.
- `ALTER EXTENSION SET SCHEMA` (relocatable extensions). Schema
  changes always become DROP+CREATE (destructive). Avoids needing to
  read the `extrelocatable` flag at plan time and avoids per-extension
  branching.
- Reverse dep edges from managed objects to extensions. v0.2 simplifies
  by ordering all extensions after schemas and before all other
  objects; PG enforces the per-object dependency at apply time.

## IR

A new flat struct in `pgevolve_core::ir::extension`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Extension {
    /// Extension name (e.g., `pgcrypto`, `pg_trgm`).
    pub name: Identifier,
    /// Target schema. `None` means "use the extension's default schema"
    /// (matches omitting `WITH SCHEMA` in the SQL).
    pub schema: Option<Identifier>,
    /// Pinned version string. `None` means "any installed version is fine"
    /// (matches omitting `VERSION` in the SQL).
    pub version: Option<String>,
    /// Optional `COMMENT ON EXTENSION` text.
    #[diff(via_debug)]
    pub comment: Option<String>,
}
```

Lives in `Catalog::extensions: Vec<Extension>`. `Catalog::canonicalize`
adds the collection to `sort_and_dedupe`'s pass — sorted by `name`,
duplicates raise `IrError::InvalidIdentifier("duplicate extension: …")`.

### Symmetry rules

Source IR can carry `schema = None` and `version = None`; catalog
reader always populates both (`pg_extension.extversion`,
`pg_namespace.nspname`). The differ honors source-side `None` as
"don't care" — see the differ section below.

## Source parser

New whitelist entry: `CREATE EXTENSION`. Builder at
`parse/builder/create_extension_stmt.rs`:

```sql
CREATE EXTENSION [IF NOT EXISTS] name
    [WITH] [SCHEMA s] [VERSION 'v']
    [CASCADE];
```

- `IF NOT EXISTS` and the `WITH` keyword are noise at IR level — both
  discarded.
- `CASCADE` clause is rejected with `ParseError::UnsupportedClause`.
  CASCADE in source SQL means "automatically install dependent
  extensions"; pgevolve requires every extension to be explicitly
  declared.
- `FROM template_name` (deprecated PG syntax) and `NO RESTART` (PG 17+
  flag) are rejected with `UnsupportedClause`.
- `ALTER EXTENSION ...` and `DROP EXTENSION ...` in source files are
  rejected — source is the desired state, not a migration script.
  Matches existing v0.2 behavior for other object kinds.

## Catalog reader

### Reading extensions

New query in `catalog/queries/extensions.rs`:

```sql
SELECT
    e.extname        AS name,
    n.nspname        AS schema,
    e.extversion     AS version,
    d.description    AS comment
FROM pg_catalog.pg_extension e
JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
LEFT JOIN pg_catalog.pg_description d
    ON d.objoid = e.oid
   AND d.classoid = 'pg_catalog.pg_extension'::regclass
ORDER BY e.extname;
```

Catalog-side `Extension { schema: Some(_), version: Some(_), … }` —
both fields always concrete.

### Filtering extension-owned objects

Every other catalog query (tables, indexes, sequences, views, MVs,
functions, procedures, user types, constraints) gains a new
`NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d WHERE d.objid = c.oid AND d.deptype = 'e')`
clause against `pg_class.oid` / `pg_proc.oid` / `pg_type.oid` as
appropriate.

This is the load-bearing change for Decision 20: an extension that
installs into a managed schema (e.g., `CREATE EXTENSION pg_trgm WITH
SCHEMA app`) creates operators and functions in that schema. Without
the filter they'd appear as drift on every plan. With the filter they
never enter the catalog IR.

Silent skip — no separate `DriftReport` entry. Matches PG's own
`pg_dump --extension` behavior of not dumping extension-owned
objects.

## Differ

```rust
pub enum ExtensionChange {
    Create(Extension),
    Drop(Identifier),
    AlterUpdate {
        name: Identifier,
        to_version: String,
    },
    ReplaceWithCascade(Extension),
    CommentOn {
        name: Identifier,
        comment: Option<String>,
    },
}
```

Implemented in `diff/extensions.rs`. Pair-by-name semantics. For each
pair:

1. **Name in source but not catalog** → `Create(source)`.
2. **Name in catalog but not source** → `Drop(name)`.
3. **Schema mismatch** (source `Some(s)` ≠ catalog `Some(t)`) →
   `ReplaceWithCascade(source)`. Source-`None` matches any catalog.
4. **Version mismatch** (source `Some(v)` ≠ catalog `Some(w)`) →
   `AlterUpdate { name, to_version: v }`. Source-`None` skips the
   comparison entirely.
5. **Comment mismatch** → `CommentOn { name, comment: source.comment }`.

Variants 3 and 4 are mutually exclusive at the diff layer (schema
takes precedence — a schema change supersedes any version change
because the DROP+CREATE will pick up the new version anyway).

`Destructiveness::Loss` is set on `Drop` and `ReplaceWithCascade`. The
`destructive_reason` carries the cascade context: "DROP EXTENSION
CASCADE removes all objects owned by extension `name`."

## Planner

### Step kinds

Five new variants in `StepKind`:

| Variant | Destructive | Transactional | SQL emitted |
|---|---|---|---|
| `CreateExtension` | no | InTransaction | `CREATE EXTENSION IF NOT EXISTS "name" [WITH SCHEMA "schema"] [VERSION 'v'];` |
| `DropExtension` | yes (intent required) | InTransaction | `DROP EXTENSION "name" CASCADE;` |
| `AlterExtensionUpdate` | no | InTransaction | `ALTER EXTENSION "name" UPDATE TO 'v';` |
| `CommentOnExtension` | no | InTransaction | `COMMENT ON EXTENSION "name" IS '…';` / `IS NULL` |
| (no separate variant for `ReplaceWithCascade`) | — | — | emits `DropExtension` + `CreateExtension` with linked intent |

### Dep graph

New `NodeId::Extension(Identifier)` in `plan/edges.rs`. Single edge
kind:

- `Extension → Schema(s)` when `schema = Some(s)`. Forces the schema
  to exist (in source order) before the extension is created. Drops
  reverse: extension is dropped before its schema.

No reverse edges from managed objects to extensions. PG enforces the
per-column / per-function dependency at apply time; the explicit
schema → extension → everything-else order is enough for the planner
to produce a valid sequence.

### Rewrite

New file `plan/rewrite/emit/extension.rs` alongside the 11 existing
`emit/*` dispatchers. Exposes five `pub fn`s, one per step kind.

New file `plan/rewrite/extensions.rs` alongside `sql.rs`, `functions.rs`,
`views.rs`, `types.rs`. Holds the SQL-string emission helpers
(`create_extension`, `drop_extension`, `alter_extension_update`,
`comment_on_extension`).

The top-level dispatcher `emit_change` in `plan/rewrite/mod.rs` gains
one new arm:

```rust
Change::Extension(ec) => emit::extension::emit(ec, destructive, destructive_reason, out),
```

## Lints

Two new rules in `lint/rules/`:

### `extension-version-unpinned` (Warning)

`CREATE EXTENSION foo;` without a `VERSION` clause. Rationale: an
unpinned extension can shift under the user's feet between dev and
prod. Suggested fix: pin to the version currently installed in dev.

The lint fires at parse time (no DB introspection required).

### `extension-references-unmanaged-schema` (Error)

`CREATE EXTENSION foo WITH SCHEMA gis;` but `gis` isn't in the
project's managed schemas. Without the target schema being managed,
the planner can't guarantee creation order. Suggested fix: add the
schema to `[managed].schemas` or drop the `WITH SCHEMA` clause.

Fires at AST-resolution time after the source catalog is fully built.

## Testing

### ~12 conformance fixtures

Under `crates/pgevolve-conformance/tests/cases/`:

- `objects/extensions/create-simple` — `CREATE EXTENSION pgcrypto;`
- `objects/extensions/create-with-schema` — `CREATE EXTENSION pg_trgm WITH SCHEMA app;`
- `objects/extensions/create-with-version` — `CREATE EXTENSION pgcrypto VERSION '1.3';`
- `objects/extensions/drop-simple` — drop extension (intent required; verifies destructive_reason wording)
- `objects/extensions/update-version` — version 1.3 → 1.4 (AlterUpdate path)
- `objects/extensions/replace-schema` — schema `public` → `app` (ReplaceWithCascade; intent required)
- `objects/extensions/comment-on` — set / change / clear comment
- `objects/extensions/version-pin-noop` — source pinned to current version → empty plan
- `objects/extensions/version-unpinned-noop` — source unpinned matches any catalog version → empty plan
- `scenarios/extension-owned-objects-ignored` — install pg_trgm into managed schema `app`; verify catalog reader skips the operators/functions pg_trgm installs (no drift)
- `scenarios/create-order-schema-first` — `CREATE SCHEMA app; CREATE EXTENSION pg_trgm WITH SCHEMA app;` — verify schema step precedes extension step in dep order
- `lint/extension-version-unpinned` — lint warning fires; passes through L2

### Unit tests

Co-located with each new module:

- `ir/extension.rs` — canonicalization, dedupe, comparison rules.
- `diff/extensions.rs` — every `ExtensionChange` variant path, plus
  source-`None` symmetry rules.
- `parse/builder/create_extension_stmt.rs` — every clause combination;
  rejection of CASCADE / FROM template.
- `plan/rewrite/emit/extension.rs` — every step-kind emission.
- Lint rules — both rules with positive and negative fixtures.

### Property tests

None added in this sub-spec — extensions are simple enough that
fixtures cover the surface adequately. No round-trip property worth
proving beyond what the conformance suite already verifies via L4.

## Files

### Created

- `crates/pgevolve-core/src/ir/extension.rs`
- `crates/pgevolve-core/src/parse/builder/create_extension_stmt.rs`
- `crates/pgevolve-core/src/catalog/queries/extensions.rs`
- `crates/pgevolve-core/src/diff/extensions.rs`
- `crates/pgevolve-core/src/plan/rewrite/extensions.rs`
- `crates/pgevolve-core/src/plan/rewrite/emit/extension.rs`
- `crates/pgevolve-core/src/lint/rules/extension_version_unpinned.rs`
- `crates/pgevolve-core/src/lint/rules/extension_references_unmanaged_schema.rs`
- ~12 conformance fixtures under `crates/pgevolve-conformance/tests/cases/objects/extensions/` and `scenarios/`

### Modified

- `crates/pgevolve-core/src/ir/catalog.rs` — `pub extensions: Vec<Extension>` field, canonicalize integration.
- `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs` — sort + dedupe pass for extensions.
- `crates/pgevolve-core/src/ir/mod.rs` — `pub mod extension;`.
- `crates/pgevolve-core/src/parse/builder/mod.rs` — register the new builder.
- `crates/pgevolve-core/src/parse/mod.rs` — dispatch `CreateExtensionStmt` to the new builder.
- `crates/pgevolve-core/src/catalog/mod.rs` — read extensions; add the `pg_depend deptype='e'` filter to every existing object query.
- `crates/pgevolve-core/src/catalog/queries/{tables,indexes,sequences,functions,user_types,views,constraints}.rs` (or wherever those SQL strings live) — add the new WHERE clause.
- `crates/pgevolve-core/src/diff/mod.rs` — wire the extensions diff into `Catalog::diff`.
- `crates/pgevolve-core/src/diff/change.rs` — add `Change::Extension(ExtensionChange)`.
- `crates/pgevolve-core/src/plan/edges.rs` — `NodeId::Extension(Identifier)`.
- `crates/pgevolve-core/src/plan/raw_step.rs` — five new `StepKind` variants.
- `crates/pgevolve-core/src/plan/ordering.rs` — emit `Extension → Schema` edges; place extensions in the create / drop buckets in the correct order.
- `crates/pgevolve-core/src/plan/rewrite/mod.rs` — new `Change::Extension(ec)` arm calling into `emit::extension::emit`.
- `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs` — `pub(super) mod extension;` declaration.
- `crates/pgevolve-core/src/lint/rules/mod.rs` — register the two new rules.

## Open questions

None — design is closed.
