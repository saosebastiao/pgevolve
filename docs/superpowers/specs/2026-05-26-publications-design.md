# PUBLICATION sub-spec — v0.3.4 design

**Status:** approved 2026-05-26. Successor to the v0.3 cross-cutting state series; first item from the post-v0.3.3 agreed roadmap.

**Goal:** Model Postgres `CREATE PUBLICATION` as a first-class IR object so that logical-replication source-side metadata is declarative under pgevolve.

**Non-goals:**
- `SUBSCRIPTION` (the consumer side) — separate v0.3.5 sub-spec; brings cross-server connection + secrets management that need their own design pass.
- Per-publication `GRANT` — PG has no privilege model on publications, only `OWNER`.
- `ALTER PUBLICATION … RENAME TO` — pgevolve never models renames; same policy as every other object kind.
- `publish_generated_columns` — PG 18+; beyond the supported PG range (14–17).

## IR shape

`Publication` is a top-level Catalog member, *not* schema-qualified. Postgres treats publications as a per-database global namespace.

```rust
pub struct Publication {
    pub name:                       Identifier,
    pub scope:                      PublicationScope,
    pub publish:                    PublishKinds,
    pub publish_via_partition_root: bool,
    pub owner:                      Option<Identifier>,   // v0.3.1 lenient
    pub comment:                    Option<String>,
}

pub enum PublicationScope {
    /// `CREATE PUBLICATION p FOR ALL TABLES`. Implicitly captures every
    /// current and future table in the database; mutually exclusive with
    /// per-table / per-schema selection.
    AllTables,

    /// `CREATE PUBLICATION p FOR TABLE …, TABLES IN SCHEMA …`. Either
    /// list may be empty (but not both — a publication must include at
    /// least one selector). Schema-scope is PG 15+ only.
    Selective {
        schemas: BTreeSet<Identifier>,
        tables:  Vec<PublishedTable>,
    },
}

pub struct PublishedTable {
    pub qname:      QualifiedName,
    pub row_filter: Option<NormalizedExpr>,        // PG 15+
    pub columns:    Option<Vec<Identifier>>,       // PG 15+
}

pub struct PublishKinds {
    pub insert:   bool,
    pub update:   bool,
    pub delete:   bool,
    pub truncate: bool,
}
```

`Catalog::publications: Vec<Publication>` — sorted by `name` in canon. Single new module: `crates/pgevolve-core/src/ir/publication.rs`.

**Lenient-drift granularity:** at the *whole-publication* level. A publication in source is "managed"; one in catalog but not in source is "unmanaged" and surfaces via `unmanaged-publication` lint. Internal fields (`scope`, `publish`, `publish_via_partition_root`) are concrete — when a publication is managed, every field is managed. Only `owner` follows the per-field `Option` lenient pattern, identical to v0.3.1.

**Type-level invariant:** `PublicationScope` enforces mutual exclusion of `AllTables` vs `Selective` (constitution §3: make illegal states unrepresentable). PG's `puballtables` boolean cannot coexist with `pg_publication_rel` / `pg_publication_namespace` rows; the IR mirrors that.

**Canon validation (`ir/canon/publications.rs`):**
- `Selective` with empty `schemas` AND empty `tables` → `IrError::EmptyPublication`.
- Sort `schemas` (already `BTreeSet`); sort `tables` by `qname`.
- Sort `columns: Vec<Identifier>` per `PublishedTable` (and reject duplicates).

## Source surface

Publications get their own slot in every layout profile.

| Profile         | Path                              |
|-----------------|-----------------------------------|
| `schema-mirror` | `schema/publications/<name>.sql`  |
| `kind-grouped`  | `schema/publications/<name>.sql`  |
| `feature-grouped` | any file containing the CREATE  |
| `free-form`     | anywhere                          |

Three syntactic forms:

```sql
-- Form 1: explicit list with PG15+ filters and column lists.
CREATE PUBLICATION replication_main
    FOR TABLE app.orders (id, customer_id, placed_at) WHERE (status = 'active'),
        app.customers,
        billing.invoices
    WITH (publish = 'insert, update', publish_via_partition_root = true);

-- @pgevolve owner: app_publisher
COMMENT ON PUBLICATION replication_main IS 'main replication stream';
```

```sql
-- Form 2: FOR ALL TABLES.
CREATE PUBLICATION audit_all
    FOR ALL TABLES
    WITH (publish = 'insert, update, delete, truncate');
```

```sql
-- Form 3: schema-scope (PG 15+), optionally mixed with explicit tables.
CREATE PUBLICATION per_schema
    FOR TABLES IN SCHEMA app, billing
    WITH (publish = 'insert, update, delete');

ALTER PUBLICATION per_schema ADD TABLE legacy.archived;
```

The parser folds inline `WITH (...)` and any subsequent `ALTER PUBLICATION` operations into one canonical `Publication` IR — same model as v0.3.3 reloptions where `CREATE TABLE WITH (...)` and `ALTER TABLE SET (...)` unified.

**Source-side rejections (parse-time):**
- `ALTER PUBLICATION p RENAME TO p2` — pgevolve never models renames.
- `CREATE PUBLICATION p` with no scope clause (PG syntax allows it; defaults to empty `Selective`) — pgevolve requires an explicit scope.
- `FOR ALL TABLES` combined with `FOR TABLE` or `FOR TABLES IN SCHEMA` — PG itself rejects this; we double-check at parse time for a cleaner error.

## Catalog reader

Three `pg_catalog` tables, joined per-version:

| Table                          | Columns                                                                                       | PG version  |
|--------------------------------|-----------------------------------------------------------------------------------------------|-------------|
| `pg_publication`               | `oid`, `pubname`, `pubowner`, `puballtables`, `pubinsert`, `pubupdate`, `pubdelete`, `pubtruncate`, `pubviaroot` | all (`pubviaroot` PG 13+) |
| `pg_publication_rel`           | `prpubid`, `prrelid`, `prqual` (row filter), `prattrs` (column list)                          | all (`prqual`/`prattrs` PG 15+) |
| `pg_publication_namespace`     | `pnpubid`, `pnnspid`                                                                          | PG 15+ only |

Joined with `pg_authid` for owner name, `pg_class` + `pg_namespace` for table qnames, `pg_namespace` for schema-scope names, and `pg_description.objsubid = 0` for the publication comment.

On PG 14:
- `pg_publication_namespace` query is skipped; `PublicationScope::Selective.schemas` is always empty.
- `prqual` / `prattrs` are not read; `PublishedTable.row_filter` / `columns` always `None`.

Row filter decoding: `pg_get_expr(prqual, prrelid)` returns the SQL text; we pipe through `NormalizedExpr::from_sql` (same canon as CHECK / USING / WITH CHECK), giving whitespace and keyword-case insensitivity for free.

Column list decoding: `prattrs` is `int2vector` of `pg_attribute.attnum` values; join to `pg_attribute` to resolve to `Identifier`s, then sort.

## Differ

Pair publications by `name`. Per-publication cases:

| Source | Target  | Emits                                                                                         |
|--------|---------|-----------------------------------------------------------------------------------------------|
| present | absent | `Change::CreatePublication(Publication)` (Safe)                                               |
| absent  | present | no auto-drop; fires `unmanaged-publication` warning lint                                      |
| both    | both    | granular diff (see below)                                                                     |

**Granular diff when both present:**

- **Mode mismatch** (`AllTables` ↔ `Selective`): `Change::ReplacePublication { from, to }` — DROP + CREATE. PG has no in-place mode swap, and trying to ALTER between modes is one of the few legitimate `RequiresApproval` operations because the destination set semantically changes.
- **Inside `Selective`** (both source and target are `Selective`):
  - Tables added in source: `Change::AlterPublicationAddTable { publication, table }` per row.
  - Tables removed from source: `Change::AlterPublicationDropTable { publication, qname }` per row.
  - Per-table row-filter or column-list change: `Change::AlterPublicationSetTable { publication, table }` — uses PG's `ALTER PUBLICATION p SET TABLE x (cols) WHERE (filter)` which replaces just that one table's spec.
  - Schemas added: `Change::AlterPublicationAddSchema { publication, schema }`.
  - Schemas removed: `Change::AlterPublicationDropSchema { publication, schema }`.
- **`publish` bitset diff**: `Change::AlterPublicationSetPublish { publication, kinds }`.
- **`publish_via_partition_root` diff**: `Change::AlterPublicationSetViaRoot { publication, value }`.
- **Owner diff**: standard v0.3.1 `Change::AlterObjectOwner` with `kind: OwnerObjectKind::Publication`.
- **Comment diff**: `Change::CommentOnPublication { name, comment }`.

Granular ALTERs (vs. one big `SET (publish = ..., publish_via_partition_root = ...)`) so each transition is auditable in `plan.sql` and each step is independently rollback-safe.

## Planner step kinds

```rust
pub enum StepKind {
    // ... existing variants ...
    CreatePublication,
    DropPublication,                      // destructive (intent required)
    ReplacePublication,                   // destructive (intent required) — mode swap
    AlterPublicationAddTable,
    AlterPublicationDropTable,
    AlterPublicationSetTable,
    AlterPublicationAddSchema,            // PG 15+
    AlterPublicationDropSchema,           // PG 15+
    AlterPublicationSetPublish,
    AlterPublicationSetViaRoot,
    CommentOnPublication,
}
```

All transactional (`TransactionConstraint::InTransaction`). Destructiveness:

| StepKind                       | Destructive | Reason                                                       |
|--------------------------------|-------------|--------------------------------------------------------------|
| `CreatePublication`            | no          |                                                              |
| `DropPublication`              | yes         | data-loss-free but irreversible decision                     |
| `ReplacePublication`           | yes         | mode swap; subscribers may break                             |
| `AlterPublicationDropTable`    | no          | replication of the table stops; no data loss                 |
| `AlterPublicationDropSchema`   | no          | same                                                         |
| `AlterPublicationSetTable`     | no          | row-filter / column-list shape change                        |
| (everything else)              | no          |                                                              |

## Lint rules

Three new rules, all warnings, all waivable via `[[lint_waiver]]`:

- **`unmanaged-publication`** — catalog reports a publication source doesn't declare. Standard v0.3.x lenient-drift pattern. Mirrors `unmanaged-grant` / `unmanaged-policy` / `unmanaged-reloption`. Lives in `crates/pgevolve-core/src/lint/rules/unmanaged_publication.rs`.
- **`publication-captures-unmanaged-table`** — a `FOR ALL TABLES` or `FOR TABLES IN SCHEMA s` publication implicitly captures every current and future table in scope. Lint surfaces every catalog-reported published table whose qname falls outside the source IR's managed set (out of `[managed].schemas` or out of source). Helps catch "I added a table and it silently started replicating."
- **`publication-row-filter-references-unmanaged-column`** — row filter references a column that source doesn't declare on the target table. Same drift class as the existing `view-body-references-unmanaged-schema` rule. Implementation walks the `NormalizedExpr` AST extracting `ColumnRef` nodes (the `parse::ast_canon` machinery already does this for view bodies; reuse the walker).

## PG-version gating

Source projects declare their minimum supported PG via a new `[managed].min_pg_version` config key (default `14`, the workspace minimum). For each Publication, after parse:

- `PublicationScope::Selective { schemas, .. }` where `!schemas.is_empty()` → require `min_pg_version >= 15`.
- Any `PublishedTable` with `row_filter.is_some()` or `columns.is_some()` → require `min_pg_version >= 15`.

If the project declares `min_pg_version = 14` and the source uses a PG 15+ feature, fail at **lint time** (not at apply) with a clear "this feature requires PG 15+; raise `[managed].min_pg_version` or remove the row filter" message. New lint rule: `publication-feature-requires-pg-version` (Error, not waivable — using PG15+ syntax on a project declaring PG14 is genuine misconfiguration).

`[managed].min_pg_version` is reusable for future PG-version-gated features; not unique to publications.

## Dependency graph

`NodeId::Publication(Identifier)` joins the existing enum.

Edges added during source-side dep-graph build:

- `Publication → Table` for every entry in `Selective.tables` (publication created *after* the tables it references).
- `Publication → Schema` for every entry in `Selective.schemas`.
- `PublicationScope::AllTables` adds no explicit edges; the planner orders publications strictly *after* all table creates in the same plan via a tier rule.

Drop order: reverse — publications drop before the tables they reference, even though PG technically allows the reverse (publications reference tables by OID and the system catalog handles cascade). Reverse-topo keeps the audit log clean.

## Conformance fixtures

Tier-C coverage target: **12 fixtures** under `crates/pgevolve-conformance/tests/cases/objects/publications/`.

| Fixture                                  | Covers                                                          | PG min |
|------------------------------------------|-----------------------------------------------------------------|--------|
| `for-table-list/`                        | explicit table list, two tables                                 | 14     |
| `for-table-with-filter/`                 | row filter                                                      | 15     |
| `for-table-with-columns/`                | column list                                                     | 15     |
| `for-table-with-filter-and-columns/`     | combo                                                           | 15     |
| `for-all-tables/`                        | match-everything                                                | 14     |
| `for-tables-in-schema/`                  | schema-scope                                                    | 15     |
| `mixed-tables-and-schema/`               | `Selective` with both tables and schemas                        | 15     |
| `alter-add-table/`                       | modify an existing publication                                  | 14     |
| `alter-drop-table/`                      | remove a table from a publication                               | 14     |
| `alter-set-publish/`                     | change `publish` bitset                                         | 14     |
| `mode-swap-replaces/`                    | `AllTables` → `Selective` emits `ReplacePublication`            | 14     |
| `lint/unmanaged-publication/`            | catalog has publication source doesn't                          | 14     |

Each fixture follows the established structure: `before.sql`, `after.sql`, `fixture.toml`, blessed `expected/plan.sql`.

## Render

New module: `crates/pgevolve-core/src/plan/rewrite/publications.rs`. SQL helpers:

```rust
pub fn create_publication(p: &Publication) -> String;
pub fn drop_publication(name: &Identifier) -> String;
pub fn replace_publication(from: &Publication, to: &Publication) -> [String; 2];
pub fn alter_publication_add_table(pname: &Identifier, t: &PublishedTable) -> String;
pub fn alter_publication_drop_table(pname: &Identifier, qname: &QualifiedName) -> String;
pub fn alter_publication_set_table(pname: &Identifier, t: &PublishedTable) -> String;
pub fn alter_publication_add_schema(pname: &Identifier, schema: &Identifier) -> String;
pub fn alter_publication_drop_schema(pname: &Identifier, schema: &Identifier) -> String;
pub fn alter_publication_set_publish(pname: &Identifier, k: PublishKinds) -> String;
pub fn alter_publication_set_via_root(pname: &Identifier, value: bool) -> String;
pub fn comment_on_publication(name: &Identifier, comment: Option<&str>) -> String;
```

`PublishKinds` renders as a comma-separated list inside `publish = '...'`. Empty bitset is illegal at the IR level (canon rejects).

## Property tests

Extend `crates/pgevolve-testkit/src/ir_generator.rs`:

```rust
fn arb_publish_kinds() -> impl Strategy<Value = PublishKinds>;
fn arb_publication_scope(tables: &[QualifiedName], schemas: &[Identifier]) -> BoxedStrategy<PublicationScope>;
fn arb_publication(tables: &[QualifiedName], schemas: &[Identifier]) -> BoxedStrategy<Publication>;
```

Plumbed into `arbitrary_catalog` to generate 0–2 publications per catalog. Row filters use a small fixed strategy (`Just("true")`, `Just("col1 IS NOT NULL")`) to keep canon expectations stable. Column lists draw from the target table's actual column names.

## File / module additions

```
crates/pgevolve-core/src/
├── ir/
│   ├── publication.rs                   NEW
│   └── canon/
│       └── publications.rs              NEW
├── catalog/
│   ├── publications.rs                  NEW (per-version SQL + decoder)
│   └── assemble/
│       └── publications.rs              NEW
├── parse/
│   └── builder/
│       └── publication_stmt.rs          NEW
├── diff/
│   └── publications.rs                  NEW
├── plan/
│   └── rewrite/
│       └── publications.rs              NEW
└── lint/
    └── rules/
        ├── unmanaged_publication.rs                       NEW
        ├── publication_captures_unmanaged_table.rs        NEW
        └── publication_row_filter_references_unmanaged_column.rs  NEW

crates/pgevolve-core/src/diff/change.rs   MODIFY (11 new variants)
crates/pgevolve-core/src/plan/raw_step.rs MODIFY (11 new StepKind variants)
crates/pgevolve-core/src/ir/catalog.rs    MODIFY (publications field)
crates/pgevolve-core/src/ir/canon/mod.rs  MODIFY (publications pass)
crates/pgevolve/src/config.rs             MODIFY ([managed].min_pg_version)

crates/pgevolve-conformance/tests/cases/objects/publications/  NEW (12 fixtures)

docs/spec/objects.md                      MODIFY (publication rows ✅ Supported)
docs/spec/publications.md                 NEW (capability page, like reloptions.md)
docs/spec/cli.md                          MODIFY (any new CLI surface? none expected)
CHANGELOG.md                              MODIFY ([0.3.4] section)
```

## Release

v0.3.4. Standard `docs/RELEASING.md` flow. Tag signed.

`unmanaged-publication` wires into `run_drift_lints` alongside `unmanaged-grant` / `unmanaged-policy` / `unmanaged-reloption` (same dispatcher entry point used by every other v0.3.x cross-cutting drift lint).
