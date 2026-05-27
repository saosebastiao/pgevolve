# Publications

pgevolve models `CREATE PUBLICATION` as a first-class declarative IR object.
A publication is a per-database global namespace object (not schema-qualified)
that controls which tables and schemas are replicated via Postgres logical
replication.

## Source surface

All five Postgres syntactic forms are supported:

```sql
-- Form 1: FOR ALL TABLES — captures every current and future table
CREATE PUBLICATION pub_all FOR ALL TABLES;

-- Form 2: explicit FOR TABLE list (PG 14+)
CREATE PUBLICATION pub_tables
    FOR TABLE app.orders, app.items
    WITH (publish = 'insert, update, delete');

-- Form 3: FOR TABLES IN SCHEMA (PG 15+; requires min_pg_version = 15)
CREATE PUBLICATION pub_schema
    FOR TABLES IN SCHEMA app, billing;

-- Form 4: row filter on a table (PG 15+; requires min_pg_version = 15)
CREATE PUBLICATION pub_filtered
    FOR TABLE app.orders WHERE (status = 'active');

-- Form 5: explicit column list (PG 15+; requires min_pg_version = 15)
CREATE PUBLICATION pub_columns
    FOR TABLE app.orders (id, status, created_at);

-- Mixed: schemas + tables in one publication (PG 15+)
CREATE PUBLICATION pub_mixed
    FOR TABLE app.users, TABLES IN SCHEMA billing
    WITH (publish = 'insert, update', publish_via_partition_root = true);
```

The `WITH` clause parameters are:

| Parameter | Values | Default |
|---|---|---|
| `publish` | comma-separated list of `insert`, `update`, `delete`, `truncate` | all four |
| `publish_via_partition_root` | `true` / `false` | `false` |

**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/publication_stmt.rs`; tier-C: `objects/publications/`.

## Semantics — lenient at the publication grain

Unlike field-level leniency (as in reloptions), pgevolve applies leniency at
the **whole-publication** level:

| source | catalog | differ action |
|---|---|---|
| Publication absent | Publication absent | no-op |
| Publication absent | Publication present | **no-op** — surface as `unmanaged-publication` lint warning |
| Publication present | Publication absent | `CREATE PUBLICATION` |
| Publication present, same | Publication present, same | no-op |
| Publication present, scope differs | Publication present, scope differs | granular ALTER steps |
| Publication present, publish/via-root differs | Publication present, differs | `ALTER PUBLICATION … SET (publish = …)` / `SET (publish_via_partition_root = …)` |

A publication in source is fully managed (all fields tracked). A publication
absent from source is left alone and surfaces via lint. This mirrors the
v0.3.1 owner/grants pattern.

## Mode-swap: ReplacePublication

When the scope mode changes between `AllTables` and `Selective`, no safe
in-place `ALTER PUBLICATION` path exists in Postgres. pgevolve emits
`DROP PUBLICATION` + `CREATE PUBLICATION` (a `ReplacePublication` logical
operation), which is destructive and requires intent approval.

**Tests:** tier-C: `objects/publications/replace-all-tables-to-selective`.

## PG-version gating via `[managed].min_pg_version`

Three source features require Postgres 15+:

- `FOR TABLES IN SCHEMA` (schema-scope)
- Row filters (`WHERE (...)` on table entries)
- Explicit column lists

Using any of these features when `[managed].min_pg_version` is below 15
(the default is 14) is a lint error (`publication-feature-requires-pg-version`,
Error severity, not waivable). This surfaces the version dependency at lint
time rather than at apply time with an opaque Postgres syntax error.

```toml
# pgevolve.toml
[managed]
min_pg_version = 15
```

**Tests:** tier-C: `objects/publications/lint-pg-version-schema-scope`,
`lint-pg-version-row-filter`, `lint-pg-version-column-list`.

## Supported `publish` options

| Field | IR name | PG catalog column | Default |
|---|---|---|---|
| `insert` | `PublishKinds::insert` | `pg_publication.pubinsert` | `true` |
| `update` | `PublishKinds::update` | `pg_publication.pubupdate` | `true` |
| `delete` | `PublishKinds::delete` | `pg_publication.pubdelete` | `true` |
| `truncate` | `PublishKinds::truncate` | `pg_publication.pubtruncate` | `true` |

At least one kind must be enabled; an all-false `PublishKinds` is rejected at
canon time.

## `publish_via_partition_root`

When `true`, `INSERT`/`UPDATE`/`DELETE` operations on partition children are
reported using the partition root's row identity (the parent table's replica
identity). This is PG 13+ behavior; pgevolve tracks it as a plain `bool` on
`Publication`. No version gate is applied (PG 13 is below our minimum of PG 14).

## 11 StepKind variants

| Step kind | SQL emitted |
|---|---|
| `CreatePublication` | `CREATE PUBLICATION …` |
| `DropPublication` | `DROP PUBLICATION name` (destructive; intent required) |
| `ReplacePublication` | `DROP PUBLICATION name` + `CREATE PUBLICATION …` (mode-swap; destructive) |
| `AlterPublicationAddTable` | `ALTER PUBLICATION name ADD TABLE …` |
| `AlterPublicationDropTable` | `ALTER PUBLICATION name DROP TABLE …` |
| `AlterPublicationSetTable` | `ALTER PUBLICATION name SET TABLE …` (full table-list replacement) |
| `AlterPublicationAddSchema` | `ALTER PUBLICATION name ADD TABLES IN SCHEMA …` (PG 15+) |
| `AlterPublicationDropSchema` | `ALTER PUBLICATION name DROP TABLES IN SCHEMA …` (PG 15+) |
| `AlterPublicationSetPublish` | `ALTER PUBLICATION name SET (publish = '…')` |
| `AlterPublicationSetViaRoot` | `ALTER PUBLICATION name SET (publish_via_partition_root = …)` |
| `CommentOnPublication` | `COMMENT ON PUBLICATION name IS '…'` |

## 4 lint rules

| Rule | Severity | Condition | Waivable? |
|---|---|---|---|
| `unmanaged-publication` | Warning | A publication is in the catalog but not in source | Yes |
| `publication-captures-unmanaged-table` | Warning | A `Selective` publication references a table whose schema is not in `[managed].schemas` | Yes |
| `publication-row-filter-references-unmanaged-column` | Warning | A row filter references a column not present in the IR for the table (because the table is unmanaged or the column was dropped) | Yes |
| `publication-feature-requires-pg-version` | Error | A PG 15+ feature (schema-scope, row filter, column list) is used but `min_pg_version` < 15 | No |

**Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/unmanaged_publication.rs::tests`,
`publication_captures_unmanaged_table.rs::tests`,
`publication_row_filter_references_unmanaged_column.rs::tests`,
`publication_feature_requires_pg_version.rs::tests`; tier-C: `objects/publications/lint-*`.

## Out of scope

- **`SUBSCRIPTION`** — consumer side; implemented as of v0.3.5. See [`subscriptions.md`](./subscriptions.md) for the full surface.
- **`GRANT` on publications** — Postgres does not support object-level grants on publications; they have no ACL. Out of scope by PG design.
- **`ALTER PUBLICATION … RENAME TO`** — not supported. Rename is treated as Drop + Create (old name disappears, new name appears).
- **Replication slots and origins** — cluster-level admin objects outside the schema-management remit.
