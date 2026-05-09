# pgevolve — Design Spec

- Status: draft, awaiting review
- Date: 2026-05-09
- Authors: Daniel Toone
- Scope: v0.1 design

## 1. Overview

`pgevolve` is a Postgres-specific declarative schema-management CLI written in Rust. It maintains a directory tree of DDL `.sql` files that collectively describe the desired state of one or more Postgres schemas, introspects a live database to derive its current state, and computes ordered, dependency-aware migration plans that bring the database to the desired state. It refuses to lose data unless the user has explicitly authorized the loss in a per-plan intent file.

The design uses [`pganalyze/pg_query.rs`](https://github.com/pganalyze/pg_query.rs) for SQL parsing (the official Postgres parser bindings) and the Postgres system catalogs for live-database introspection. Both inputs are translated into a single normalized in-memory IR; all downstream logic — diffing, dependency analysis, planning, online-rewrite handling — operates over IR, never over raw SQL strings.

## 2. Goals and non-goals

### v0.1 goals

1. **Declarative deploy.** Take a directory of `CREATE`-style DDL files and deploy them to a clean (or matching) Postgres database.
2. **Catalog introspection.** Build the same IR from a live database via `pg_catalog`, supporting Postgres 14, 15, 16, and 17.
3. **Diff and plan.** Compare source IR to catalog IR; produce an ordered, dependency-correct migration plan with online-friendly rewrites for the common-case slow operations.
4. **Safe destructive changes.** Refuse to drop or narrow without explicit, file-based per-plan approval.
5. **Audit-able apply.** Maintain a small metadata schema in the target database so every apply attempt and every step of every apply is recorded.
6. **Comprehensive test coverage.** Ship a property-based test harness capable of verifying correctness across a wide generative space of (initial, final) schema pairs.

### Object kinds in scope for v0.1

Schemas, tables, columns, indexes, constraints (PK, UNIQUE, FK, CHECK, NOT NULL), sequences. References to user-defined types (enum, domain, composite) are recognized in column type positions but the types themselves are not managed by v0.1.

### Explicitly out of scope for v0.1

- **Object kinds:** views, materialized views, functions, procedures, triggers, custom types (composite, enum, domain, range), extensions, RLS policies, GRANTs and other ACLs, partitions and partition trees, foreign tables and FDWs, publications and subscriptions, custom operators / operator classes / aggregates, event triggers, text search configurations.
- **Operations:** `revert` / `rollback`; rename detection (renames appear as drop+create); plan squashing; auto-format; auto-fix of layout-lint violations.
- **Policy surface:** per-environment online-rewrite policy parameterization is plumbed but only the global `[planner].strategy` value is read in v0.1.
- **Operational:** multi-database orchestration; concurrent apply against a single DB (refused via advisory lock); cloud-provider-specific helpers; self-update.
- **Workflow:** preview/branch environments; snapshot/restore management; CI-action templates beyond examples in docs.

## 3. Key design decisions

These decisions, made in the brainstorming session, frame everything else.

| Decision | Choice | Rationale |
|---|---|---|
| **MVP scope** | Full deploy → introspect → diff → plan → apply loop, restricted to tables / columns / indexes / constraints / sequences. | Exercises the entire architectural surface against the most-used object kinds; remaining kinds slot into the same machinery. |
| **Ownership boundary** | Schema-scoped with explicit per-object ignore list. `pgevolve.toml` lists managed schemas; `ignore_objects` excepts specific qnames or globs within them. | Matches how teams actually organize Postgres; avoids the brittle marker-comment approach; ignore list handles legitimate exceptions. |
| **Destructive change authorization** | Per-plan intent file. `pgevolve plan` writes a plan directory containing `plan.sql`, `intent.toml`, and `manifest.toml`; the user edits `intent.toml` to mark each destructive operation `approved = true`; `pgevolve apply` enforces those approvals. | The plan artifact mirrors `terraform plan` / `terraform apply`; the intent file is git-blame-able evidence of who authorized which destructive change. |
| **Online migration ambition** | Single-transaction default with a small set of well-known online rewrites (CONCURRENTLY for non-unique indexes, NOT VALID + VALIDATE for FK and CHECK adds, the CHECK-based pattern for SET NOT NULL on populated columns). Designed to extend later to per-environment policy overrides. | Covers the 80% case for prod-friendly migrations without doubling the planner's test surface in v0.1. |
| **Target-DB metadata** | Minimal `pgevolve` schema with three tables: `apply_log`, `plan_steps`, `lock`. Source of truth for *what's deployed* remains the catalog; this schema records *what pgevolve has done*. | Audit history pays for itself the first time partial-failure recovery comes up. Adding it post-hoc forces a backfill story. |
| **IR strategy** | Strongly-typed Rust IR. Both source-side parser (via `pg_query.rs`) and catalog-side introspection produce the same IR; diff/plan operates over IR only. Shadow-Postgres verification mode is a future addition. | The cleanest correctness story for the MVP-scope object kinds; equivalence rules for tables/columns/indexes/constraints are tractable and exhaustively testable. |
| **Crate layout** | Cargo workspace: `pgevolve-core` (library, no I/O beyond passed-in queries), `pgevolve` (binary, CLI + executor + filesystem + connection), `pgevolve-testkit` (dev-only test infrastructure). | Library/binary split makes the diff engine testable and embeddable; testkit isolates ephemeral-PG and property-test machinery. |
| **Plan format** | Directory per plan: `plan.sql` with structured `-- @pgevolve` comments per step; `intent.toml` for destructive approvals; `manifest.toml` for plan metadata. | Human-reviewable as plain SQL while still machine-readable; intent file is reviewable in PRs. |
| **File layout in source** | Parser-driven (object identity comes from the SQL, not the path). Layout linting is profile-driven with multiple built-in profiles plus a custom-profile mechanism. | Different teams have different conventions; pgevolve must support them rather than impose one. |

## 4. System architecture

### 4.1 Crate layout

- **`pgevolve-core`** (library) — parser frontend, IR types, source loader, catalog reader, diff engine, dependency analyzer, planner, online-rewrite pass. No filesystem I/O beyond accepting bytes. No async runtime opinions. No Postgres driver dependency — accepts a `CatalogQuerier` trait and returns plan steps as data.
- **`pgevolve`** (binary) — CLI argument parsing, `pgevolve.toml` loading, filesystem walking, connection management (`tokio-postgres`), executor, plan/intent file I/O, terminal output formatting.
- **`pgevolve-testkit`** (library, `dev-dependency` only) — ephemeral Postgres helpers, IR generators and mutators, equivalence asserters, end-to-end harnesses for property and chaos testing.

### 4.2 Pipeline

```
Source SQL files ──► Loader ──► pg_query.rs parse ──► Source IR
                                                        │
                                                        ▼
Live Postgres ──► Catalog reader ──► Catalog IR ──► Differ ──► ChangeSet
                                                        │
                                                        ▼
                              Dependency analyzer ──► Topological order
                                                        │
                                                        ▼
                              Online-rewrite pass ──► Plan (groups + steps)
                                                        │
                                                        ▼
                              Plan serializer ──► plan.sql + intent.toml + manifest.toml
                                                        │
                                                        ▼
                              Executor (with apply_log) ──► Live Postgres
```

The same Source IR / Catalog IR types and the same Differ are used by every command. Commands differ only in which suffix of the pipeline they execute and what they emit.

### 4.3 Isolation properties

- IR is the only data type that crosses module boundaries inside `pgevolve-core`.
- The diff engine is unit-testable with an in-memory mock catalog.
- The planner is unit-testable with no DB.
- The executor is the only component that requires a live Postgres connection.

## 5. IR design

### 5.1 Top-level shape

```rust
struct Catalog {
    schemas: Vec<Schema>,
    tables: Vec<Table>,
    indexes: Vec<Index>,
    sequences: Vec<Sequence>,
    // phase 2: views, functions, types, domains, triggers, policies, ...
}

struct Table {
    qname: QualifiedName,         // (schema, name)
    columns: Vec<Column>,         // ordered; logical position is meaningful
    constraints: Vec<Constraint>, // PK, UNIQUE, FK, CHECK; NOT NULL is on the column
    comment: Option<String>,
}

struct Column {
    name: Identifier,
    ty: ColumnType,
    nullable: bool,
    default: Option<DefaultExpr>,
    identity: Option<Identity>,
    generated: Option<Generated>,
    collation: Option<QualifiedName>,
    comment: Option<String>,
}
```

### 5.2 `ColumnType`

`ColumnType` is the single most important enum in the IR. It is the canonical normalized form across both source and catalog inputs.

- **Builtin scalars** with collapsed names: `int` / `integer` / `int4` → `Integer`; `bool` / `boolean` → `Boolean`; bare `varchar` → `Varchar { len: None }` (unbounded; *not* equivalent to `Text`); `varchar(N)` → `Varchar { len: Some(N) }`.
- **Parameterized**: `Numeric { precision, scale }`, `Time { precision, with_tz }`, `Timestamp { precision, with_tz }`, `Bit { len, varying }`, `Interval { fields, precision }`, etc.
- **Array**: `Array { element: Box<ColumnType>, dims: u8 }`.
- **User-defined**: `UserDefined(QualifiedName)` — structure not introspected in v0.1, but the reference is captured.
- **Catch-all**: `Other { raw: String }` — for types pgevolve doesn't yet model. Diff treats `Other` strictly: equal iff `raw` strings match exactly. Prevents crashes on unsupported syntax at the cost of conservative diffs.

### 5.3 `DefaultExpr`

Defaults are a major source of false-positive diffs. They are normalized:

```rust
enum DefaultExpr {
    Literal(LiteralValue),    // type-aware: 'foo'::text and 'foo' for a text col both → Literal(Text("foo"))
    Sequence(QualifiedName),  // nextval('schema.seq')
    Expr(NormalizedExpr),     // anything else; pg_query AST through a normalization pass
}
```

`NormalizedExpr` is a pg_query AST passed through a normalization pass: redundant casts to the column's own type are stripped, parens are folded, commutative operands are sorted, keywords are lowercased. Two `NormalizedExpr` values are equivalent iff their normalized form is structurally equal. pgevolve does not attempt semantic equivalence beyond syntactic normalization (it will not claim `1+1 ≡ 2`); this is documented as part of the IR contract.

### 5.4 Constraints and indexes

`Constraint` and `Index` carry their own qualified names because Postgres treats them as first-class objects with independent lifecycles. NOT NULL is *not* a `Constraint` — it lives on `Column.nullable`, matching `pg_attribute`. Anything that is a row in `pg_constraint` is a `Constraint` in IR.

### 5.5 SERIAL / BIGSERIAL desugaring

`SERIAL` is desugared on both sides to `integer NOT NULL DEFAULT nextval(...)` plus a sibling `Sequence` plus an `OWNED BY` linkage. The catalog reader builds the same shape from `pg_class` + `pg_attribute` + `pg_depend`. Both sides converge on the same IR. Documented as part of the source-IR contract: writing either form is identical to the diff engine.

### 5.6 Equivalence

Each IR type implements:

- `canonical_eq(&self, &other) -> bool` — semantic equivalence.
- `Diff::diff(&self, &other) -> Vec<Difference>` — structured difference list. The diff engine consumes these `Difference` values to build the change set.

`PartialEq` / `Eq` derives are reserved for tests and `HashMap` keying, where structural equality is what's wanted. Equivalence ≠ structural equality.

Equivalence is exhaustively tested via the `equivalent_pairs/` and `different_pairs/` fixture corpus described in §10.

## 6. Diff and plan pipeline

### 6.1 Source loader

- Walks the source directory recursively, honoring `pgevolve.toml` ignores.
- Reads each `.sql` file as one or more statements via `pg_query.rs::parse()`.
- Dispatches each parsed statement to a builder by AST node kind:
  - `CreateStmt` → `Table`
  - `CreateSeqStmt` → `Sequence`
  - `IndexStmt` → `Index`
  - `AlterTableStmt` → narrow whitelist for `ADD CONSTRAINT` of forward-referencing FKs only; other ALTERs in source are an error
  - `CommentStmt` → attached to its target object
  - Any other CREATE statement (views, functions, etc.) → hard error in v0.1 with a clear "phase 2" message
- **Schema qualification rule:** every CREATE must be schema-qualified. An optional file-level directive `-- @pgevolve schema=<schema_name>` lets a file be implicitly scoped, but qualification is the canonical form.
- **Schemas are declared in source.** Every schema named in `[managed].schemas` must have a matching `CREATE SCHEMA <name>;` somewhere in the source tree. The `[managed].schemas` config declares the *boundary* (what pgevolve owns); the source SQL declares the *content* (including the schema objects themselves). A schema listed in `[managed]` but missing from source is a lint error; a schema present in source but missing from `[managed]` is a lint error. This keeps source the single declarative source-of-truth and prevents accidental scope creep via config.
- **Determinism:** the loader returns objects in qname-sorted order, never filesystem order. File path influences nothing about semantics; only the lint layer cares about paths.

### 6.2 Catalog reader

- Connects via the user's `pgevolve.toml` connection string or `--db-url`.
- Runs a fixed set of queries against `pg_catalog` (primary) and `information_schema` (only where `pg_catalog` is awkward). Versioned per Postgres major; the testkit verifies each supported version.
- Filtered to managed schemas. The `pgevolve` metadata schema is always implicitly excluded. The `ignore_objects` list is applied last.
- Queries cover: `pg_namespace`, `pg_class` (relkind in `r`, `i`, `S`), `pg_attribute` + `pg_attrdef`, `pg_constraint`, `pg_index`, `pg_collation`, `pg_type`, `pg_depend` (for SERIAL/IDENTITY linkage and ownership), `obj_description` / `col_description`.
- Output: `Catalog IR` — same types as Source IR. Interchangeable from this point on.

### 6.3 Differ

```rust
enum Change {
    CreateSchema(Schema),
    DropSchema(QualifiedName),
    CreateTable(Table),
    DropTable(QualifiedName),
    AlterTable { qname: QualifiedName, ops: Vec<TableOp> },
    CreateIndex(Index),
    DropIndex(QualifiedName),
    ReplaceIndex { from: Index, to: Index }, // when not in-place alterable
    CreateSequence(Sequence),
    DropSequence(QualifiedName),
    AlterSequence { qname: QualifiedName, ops: Vec<SequenceOp> },
}

enum TableOp {
    AddColumn(Column),
    DropColumn(Identifier),
    AlterColumnType { name, from, to, using: Option<Expr> },
    SetColumnNullable { name, nullable: bool },
    SetColumnDefault { name, default: Option<DefaultExpr> },
    AddConstraint(Constraint),
    DropConstraint(Identifier),
    SetComment(...),
    // ...
}
```

Each `Change` and `TableOp` carries a `Destructiveness` tag: `Safe`, `RequiresApproval { reason }`, `RequiresApprovalAndDataLossWarning { reason }`. The intent file consumes these tags.

The differ is purely structural over IR — no SQL strings flow through it. Pure-function, no I/O, easy to fuzz.

### 6.4 Dependency analyzer and ordering

Builds two graphs: one over source-side objects (used for create/modify ordering) and one over catalog-side objects (used for drop ordering).

Edge sources for v0.1:

- schema ⟵ table ⟵ column ⟵ default-using-sequence
- table ⟵ index
- FK constraint ⟵ both endpoints
- generated column expression ⟵ columns it references

Three-phase ordering for the change set:

1. **Creates and additive ops** in dependency order (schemas → tables → columns → indexes → constraints).
2. **Modify-in-place ops** (column type changes, constraint definition changes).
3. **Drops** in *reverse* dependency order (constraints → indexes → columns → tables → schemas).

Cycle handling: if a cycle is detected (e.g., circular FKs at creation time), the planner emits the FKs as a separate post-pass step, using `ALTER TABLE ... ADD CONSTRAINT`. This is the only legitimate use of an ALTER in a source-derived plan.

### 6.5 Online-rewrite pass

Operates over the ordered change list. Each rewrite is gated on a policy switch so per-environment overrides can be added later without rewriting the rule set.

v0.1 rewrites:

- `CreateIndex` (non-unique, on existing table) → `CREATE INDEX CONCURRENTLY` in its own non-transactional step group.
- `AddConstraint(ForeignKey)` on existing table → two steps: `ADD CONSTRAINT ... NOT VALID` (cheap, in-tx) followed by `VALIDATE CONSTRAINT` (slow, in its own step group).
- `AddConstraint(Check)` on existing table → same `NOT VALID` / `VALIDATE` pattern.
- `SetColumnNullable { nullable: false }` on a populated existing column → multi-step CHECK-NOT-NULL pattern: `ADD CHECK (col IS NOT NULL) NOT VALID` → `VALIDATE CONSTRAINT` → `SET NOT NULL` (cheap once validated) → `DROP CONSTRAINT`. Marked destructive-requires-approval because validation can fail.

Step grouping: the planner partitions the final step list into **transaction groups**. Within a group, steps run inside a single `BEGIN…COMMIT`. Steps that cannot run in a transaction (CONCURRENTLY, certain ALTER TYPE) form singleton groups.

### 6.6 `Plan` shape

```rust
struct Plan {
    id: PlanId,
    groups: Vec<TransactionGroup>,
    intents: Vec<DestructiveIntent>,
    metadata: PlanMetadata,
}
```

`PlanId` is a hash of (Source IR, Target IR, pgevolve version, planner ruleset version). The serialized form (SQL + intent.toml + manifest.toml) is a presentation of `Plan`; the in-memory `Plan` is the canonical form. The executor consumes `Plan` directly when applying without round-tripping through SQL text.

## 7. Plan format on disk

A plan is a directory:

```
plans/
  2026-05-09-abc12345/
    plan.sql           # SQL + structured -- @pgevolve directives
    intent.toml        # destructive approvals
    manifest.toml      # plan id, hash, source rev, target db identity, pgevolve version, timestamps
```

`plan.sql` is the canonical artifact for code review. It runs cleanly under `psql -f plan.sql` if a user really wants to bypass the executor. pgevolve's executor extracts the structured `-- @pgevolve` directives to drive transactions, ordering, and per-step logging.

### 7.1 Step directive format

Every directive is in a SQL comment (invisible to Postgres, parsed by pgevolve):

```sql
-- @pgevolve plan id=abc12345 version=0.1.0 created=2026-05-09T18:42:11Z
-- @pgevolve source_rev=git:c0ffeeabc target=db:host_a/app/oid_12345
-- @pgevolve intents_required=2

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.invoices
CREATE TABLE app.invoices ( ... );
-- @pgevolve step=2 kind=add_constraint_not_valid destructive=false targets=app.invoices.invoices_customer_fk
ALTER TABLE app.invoices ADD CONSTRAINT invoices_customer_fk
  FOREIGN KEY (customer_id) REFERENCES app.customers(id) NOT VALID;
COMMIT;

-- @pgevolve group id=2 transactional=false
-- @pgevolve step=3 kind=create_index_concurrent destructive=false targets=app.invoices.invoices_customer_idx
CREATE INDEX CONCURRENTLY invoices_customer_idx ON app.invoices (customer_id);

-- @pgevolve group id=3 transactional=true
BEGIN;
-- @pgevolve step=4 kind=validate_constraint destructive=false targets=app.invoices.invoices_customer_fk
ALTER TABLE app.invoices VALIDATE CONSTRAINT invoices_customer_fk;
-- @pgevolve step=5 kind=drop_column destructive=true intent_id=1 targets=app.users.legacy_email
ALTER TABLE app.users DROP COLUMN legacy_email;
COMMIT;
```

### 7.2 `intent.toml`

```toml
plan_id = "abc12345"

[[intent]]
id = 1
step = 5
kind = "drop_column"
target = "app.users.legacy_email"
reason = "destructive: column has data; default loss"
approved = false   # user edits this to true to authorize

[[intent]]
id = 2
step = 11
kind = "drop_table"
target = "billing.old_invoices"
reason = "destructive: drops table with rows (table has 124382 rows at plan time)"
approved = false
```

### 7.3 `manifest.toml`

Contains:

- `plan_id`, `plan_hash`
- `pgevolve_version`, `planner_ruleset_version`
- `source_rev` (git revision when available)
- `target_identity` — hash of `(host, port, dbname, system_identifier)`; never includes credentials
- Embedded pre-image of `Catalog IR` (snapshot of the live DB at plan time, used by the apply-time drift check)
- Timestamps

## 8. Executor

`pgevolve apply <plan-dir>` flow:

1. Read `plan.sql`, `intent.toml`, `manifest.toml`. Verify plan hash matches the serialized form.
2. Connect; acquire `pg_advisory_lock(<pgevolve magic constant>)`. If contended, fail fast: another apply is in progress.
3. Verify target identity matches `manifest.target_identity`. An override flag exists for legitimate reuse (e.g., restored snapshot) but is loud.
4. Re-introspect the live DB and rebuild Catalog IR. Compare to the snapshot in `manifest.toml`. If they differ, abort: drift detected since the plan was produced — re-run `pgevolve plan`.
5. Verify all intents: every `intent_id` referenced in `plan.sql` must have `approved = true` in `intent.toml`. Any unapproved → abort, listing them.
6. Insert an `apply_log` row with `status='running'` and one `plan_steps` row per step with `status='pending'`.
7. Execute groups in order:
   - `transactional=true`: open one transaction; execute steps; mark each `running` then `succeeded`; commit. On any step error: rollback the transaction, mark all steps in this group `rolled_back`, set `apply_log.status='failed'`, exit non-zero.
   - `transactional=false`: execute steps as autocommit. On error: stop, mark this step `failed`, set `apply_log.status='failed'`, exit non-zero. Do not undo earlier steps in the group — they're already committed.
8. On success: `apply_log.status='succeeded'`, set `finished_at`, release advisory lock, exit zero.

### 8.1 Resume / partial failure

pgevolve does **not** support continuing a failed plan from step *N*. The recovery path is to re-run `pgevolve plan`. Because the source-of-truth for what's deployed is the catalog, the new plan reflects partial state — for example, an `INVALID` index from a half-finished `CREATE INDEX CONCURRENTLY` appears as drift the new plan reconciles. This eliminates an entire class of "did this step actually finish" bugs and removes the need for sophisticated resume machinery in v0.1. `pgevolve status` shows the last apply's outcome and which step failed so users know where to look.

## 9. Metadata schema

The `pgevolve` schema, owned and managed by pgevolve, present in every managed database. Three tables; no triggers or stored procedures.

```sql
CREATE SCHEMA pgevolve;

CREATE TABLE pgevolve.apply_log (
  apply_id          uuid        PRIMARY KEY,
  plan_id           text        NOT NULL,
  plan_hash         text        NOT NULL,
  pgevolve_version  text        NOT NULL,
  source_rev        text,
  target_identity   text        NOT NULL,
  actor             text,
  started_at        timestamptz NOT NULL DEFAULT now(),
  finished_at       timestamptz,
  status            text        NOT NULL CHECK (status IN ('running','succeeded','failed','aborted')),
  error_message     text
);
CREATE INDEX apply_log_started_at_idx ON pgevolve.apply_log (started_at DESC);

CREATE TABLE pgevolve.plan_steps (
  apply_id      uuid         NOT NULL REFERENCES pgevolve.apply_log(apply_id) ON DELETE CASCADE,
  step_no       int          NOT NULL,
  group_no      int          NOT NULL,
  kind          text         NOT NULL,
  destructive   boolean      NOT NULL,
  targets       text[]       NOT NULL,
  sql_text      text         NOT NULL,
  started_at    timestamptz,
  finished_at   timestamptz,
  status        text         NOT NULL CHECK (status IN ('pending','running','succeeded','failed','rolled_back','skipped')),
  error_message text,
  PRIMARY KEY (apply_id, step_no)
);

CREATE TABLE pgevolve.lock (
  singleton         boolean     PRIMARY KEY DEFAULT true CHECK (singleton),
  held_by           text,
  held_since        timestamptz,
  pgevolve_version  text
);
INSERT INTO pgevolve.lock (singleton) VALUES (true);
```

Concurrency is enforced by `pg_advisory_lock(<fixed-constant>)`. The `pgevolve.lock` row is purely audit ("who currently holds it, since when") for investigating stuck runs.

The schema is created and upgraded by an internal bootstrap migration that pgevolve owns. Bootstrap runs idempotently at the start of every command that needs it. Bootstrap migrations have their own version table so future pgevolve versions can evolve this schema without colliding with user migrations. The `pgevolve` schema is automatically excluded from the diff engine's view of the world; users will never see drift against it.

## 10. CLI surface (v0.1)

| Command | What it does | Touches DB | Writes files |
|---|---|---|---|
| `pgevolve init` | Scaffold project: `pgevolve.toml`, `schema/`, `plans/`, `.gitignore` | no | yes |
| `pgevolve lint` | Parse source; run lint profile rules | no | no |
| `pgevolve validate` | Parse + build source IR; with `--shadow`, round-trip through ephemeral PG | shadow only | no |
| `pgevolve diff --db <env>` | Build source + catalog IR; print change set (`--format=human|json|sql`) | read-only | no |
| `pgevolve plan --db <env> [-o <dir>]` | Full pipeline; write plan directory | read-only | yes |
| `pgevolve apply <plan-dir> --db <env>` | Execute plan | read+write | no |
| `pgevolve status --db <env>` | Show recent applies and per-step state from `pgevolve.apply_log` | read-only | no |
| `pgevolve dump --db <env> -o <dir>` | Introspect live DB and write source SQL in configured layout (adoption path for existing DBs) | read-only | yes |
| `pgevolve bootstrap --db <env>` | Explicitly install/upgrade the `pgevolve` metadata schema (also auto-run by other commands) | read+write | no |

### 10.1 Output

- Default: human-readable, color-on-tty, hierarchical change summaries (group → step → diff lines).
- `--format=json`: stable schema for CI/automation.
- `--format=sql` (on `diff` only): naive ALTER SQL with no online rewrites; users who want the real thing run `plan`.
- Standard `-v / -vv / --quiet` for log verbosity. Logs go to stderr; data goes to stdout.

### 10.2 Connection precedence (mirrors `psql`)

`[environments.<env>].url` → `[environments.<env>].url_env` → `PGEVOLVE_DATABASE_URL` → libpq env vars (`PGHOST`, `PGUSER`, …) → `~/.pgpass`.

## 11. Configuration: `pgevolve.toml`

```toml
[project]
name           = "myapp"
schema_dir     = "schema"
plan_dir       = "plans"
layout_profile = "schema-mirror"  # built-in name OR path to a custom profile file

[managed]
schemas        = ["app", "billing", "audit"]
ignore_objects = ["app.legacy_etl_table", "billing.audit_*"]   # qname or glob

[planner]
strategy = "online"               # v0.1 reads "atomic" | "online"

[planner.online_rewrites]
create_index_concurrent     = true
fk_not_valid_then_validate  = true
not_null_via_check_pattern  = true

[environments.dev]
url      = "postgres://localhost/myapp_dev"
strategy = "atomic"               # overrides [planner].strategy for --db=dev

[environments.prod]
url_env  = "DATABASE_URL_PROD"    # read DSN from env var rather than embed it
strategy = "online"

[shadow]                          # used by `pgevolve validate --shadow`
provider         = "testcontainers"
postgres_version = "16"
```

## 12. Layout profiles

Layout linting is a function `(SourceIR, LayoutProfile) → Vec<LintFinding>`. Four built-in profiles ship with v0.1; teams whose conventions don't match any can write a fifth.

1. **`schema-mirror`** — `schema/<schema_name>/<object_kind>/<object_name>.sql`. One object per file, path mirrors qname and kind. Strictest; great for very large repos.
2. **`kind-grouped`** — `schema/<object_kind>/<schema_name>.<object_name>.sql`. Top-level dirs are `tables/`, `indexes/`, `sequences/`. One object per file.
3. **`feature-grouped`** — `schema/<feature>/<freeform>.sql`. Files grouped by domain/feature; each file may contain multiple related objects. Lint enforces no cross-feature-dir overlap and no orphan objects.
4. **`free-form`** — no path constraints. Lint runs only the universal rules.
5. **`custom`** — referenced as `layout_profile = "./pgevolve-layout.toml"`. Declares which universal rules to apply plus path-pattern rules using regex with named captures (`schema`, `kind`, `name`) and assertions. v0.1 supports declarative regex+assertion rules; embedded scripting is deferred until clear demand.

### 12.1 Universal rules (all profiles, including `free-form`)

- Every statement parses cleanly under `pg_query.rs`.
- Every CREATE is schema-qualified or has a file-level `-- @pgevolve schema=...` directive.
- No object qname appears twice across the whole tree.
- No non-MVP object kinds in source (in v0.1).
- No `ALTER` outside the FK-forward-reference whitelist.
- Every referenced FK target / index column / type exists in the source IR (closed-world check).

## 13. Error handling

- **Layered error types.** `pgevolve-core` uses `thiserror`-derived enums per phase (`ParseError`, `IrError`, `CatalogError`, `DiffError`, `PlanError`); `pgevolve` (binary) wraps in an `anyhow`-style chain for display.
- **Source location preserved.** Source errors carry `file:line:col`; catalog errors carry the catalog query name and the qname being introspected; apply errors carry step number, group number, target qname, and the underlying SQLSTATE.
- **Error message contract.** Every user-facing error answers, in order: *what* failed, *where* (file:line or step N), *why* (SQLSTATE or validation rule), *what to do next*. Tested via golden-file assertions on representative failures.
- **Exit codes.**
  - `0` — success
  - `1` — lint or validation error
  - `2` — drift or pre-flight mismatch (target identity, plan-vs-current divergence)
  - `3` — apply error
  - `4` — config or CLI input error
- **No silent partial success.** Apply either reports succeeded for the whole plan or reports the exact failed step and persists the SQLSTATE in `pgevolve.apply_log`.
- **Logging.** `tracing` crate; structured fields on every span (`apply_id`, `step_no`, `qname`). stderr only. stdout is reserved for command data output (e.g., JSON diff results).

## 14. Testing strategy

Comprehensive testing is structured in seven tiers. Each tier catches a different class of bug.

### Tier 1 — Unit tests (no DB)

Pure-function tests of: IR `canonical_eq` / `diff`, `ColumnType` normalization, `DefaultExpr` normalization, the differ over synthetic IR pairs, dependency graph construction, planner ordering, online-rewrite pass.

### Tier 2 — Parser fixture corpus (no DB)

- `equivalent_pairs/` — pairs of SQL snippets that *must* produce identical Source IR.
- `different_pairs/` — pairs that *must* produce a specific named difference.
- `parse_errors/` — bad inputs with expected error messages.

This is the primary regression net for "did we accidentally collapse two distinct things or split one." Adding a fixture is the standard way to file a parser bug.

### Tier 3 — Catalog round-trip golden tests (ephemeral PG per supported version)

For each supported Postgres major: a corpus of "starting state" SQL files. The harness applies the SQL to an ephemeral PG container, runs the catalog reader, and compares the resulting Catalog IR to a checked-in golden snapshot. Catches catalog-query bugs and surfaces version-specific behavior changes immediately. Goldens are regenerated under a single `cargo xtask bless` command so updates are deliberate.

### Tier 4 — End-to-end migration fixtures (ephemeral PG)

Each fixture is `(initial_source, final_source, expected_change_summary)`. The harness:

1. Fresh PG.
2. `pgevolve plan && apply` from empty → `initial_source`. Verify Catalog IR ≡ Source IR.
3. Switch to `final_source`. `plan && apply`. Verify Catalog IR ≡ `final_source` IR.
4. Verify recorded change summary matches expected.
5. Optional data assertions if the fixture seeds rows between steps.

Hundreds of these, hand-authored to cover: every IR object kind in scope, every `Change` / `TableOp` variant, every online-rewrite path, named edge cases (column reorder, FK self-reference, circular FKs, multi-column unique with INCLUDE, partial indexes, generated columns, identity columns, collation changes).

### Tier 5 — Property-based / generative tests (ephemeral PG)

The breadth comes from this tier.

- **Generators.** `IRGenerator` (a proptest strategy) produces random valid `Catalog` IRs with parameterizable knobs (table count, columns per table, type distribution, constraint mix, index mix). `IRMutator` produces random valid mutations of an IR.
- **Properties verified against a real PG (per supported version):**
  1. **Round-trip.** For random IR `S`: `apply(S)` then introspect → IR equivalent to `S`.
  2. **End-to-end equivalence.** For random `(initial, final)`: `apply(initial)` → `plan(final)` → `apply` → catalog IR equivalent to `final`.
  3. **Idempotency.** Applying any plan twice is a no-op the second time.
  4. **DAG invariant.** Source-IR dependency graph is acyclic except for documented FK-forward-reference cycles.
  5. **Determinism.** Same input → byte-identical plan across runs.
  6. **Drift recovery.** SIGKILL the executor mid-apply at a random step; re-plan; re-apply; final state ≡ `final`.
- proptest's shrinker reduces failures to minimal repros; minimal repros become Tier-4 fixtures.

### Tier 6 — Multi-PG-version matrix

Tiers 3, 4, 5 run against every supported Postgres major in CI: PG 14, 15, 16, 17 for v0.1.

### Tier 7 — Soak

Long-running CI job runs property tests with thousands of iterations and the SIGKILL-at-random-step variant. Catches rare scheduling and state bugs that fast property runs miss.

### `pgevolve-testkit` public surface

- `EphemeralPostgres { version, init_sql }` — start/stop a containerized or local PG.
- `IRGenerator`, `IRMutator` — proptest strategies.
- `EquivalenceAsserter` — produces human-readable IR diffs on failure.
- `MigrationFixture` — TOML-described declarative loader for Tier-4 fixtures.
- `CatalogSnapshotter` — used by Tier-3 goldens.
- `ApplyHarness` — end-to-end driver: source → plan → apply → introspect → assert.
- `ChaosApplyHarness` — Tier-5 property #6 (kill-mid-apply).

The testkit is published as a workspace crate but consumed only as a `dev-dependency`.

## 15. Postgres version targets

v0.1 supports Postgres **14, 15, 16, 17**. Anything older has enough catalog-shape differences (e.g., pre-PG 14 lacks `pg_class.relkind = 'I'` for partitioned indexes) that supporting it pays for itself only with real demand.

## 16. Future work (post-v0.1)

- Additional object kinds: views, materialized views, functions, procedures, triggers, types/domains/enums, extensions, RLS policies, GRANTs, partitions, FDWs, publications/subscriptions, and the rest of the OOS list in §2.
- Shadow-Postgres verification mode for IR round-trip on object kinds whose source-IR equivalence rules are too complex to encode in Rust (functions especially).
- `revert` command with well-defined semantics.
- Rename detection via `-- @pgevolve rename from=old to=new` directives.
- Per-environment online-rewrite policy parameterization (the planner-side hooks already exist; the config surface and wiring don't).
- Additional online-rewrite rules.
- Plan squash, format, auto-fix.
- Cloud-provider-specific helpers (RDS, Aurora, Cloud SQL).

## 17. References

- [`pganalyze/pg_query.rs`](https://github.com/pganalyze/pg_query.rs) — Rust bindings to the official Postgres parser.
- Postgres documentation: [system catalogs](https://www.postgresql.org/docs/current/catalogs.html).
- Prior-art declarative schema tools whose tradeoffs informed this design: Atlas, Skeema, migra, sqitch, schemahero. None of their tradeoffs are adopted directly; they're cited so reviewers can see what's deliberately the same and what's deliberately different.
