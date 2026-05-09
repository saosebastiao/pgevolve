# Phase 3 — Catalog reader

**Goal:** Introspect a live Postgres database via `pg_catalog` and produce the same `Catalog` IR that the parser produces. Wire ephemeral Postgres provisioning into `pgevolve-testkit`. Build the Tier-3 round-trip golden harness (per supported PG major).

**Spec coverage:** §6.2 (catalog reader), §14 Tier 3, §15 (PG version targets).

**Depends on:** Phase 1 (IR), Phase 2 (parser — needed because Tier-3 fixtures often start from source SQL applied to ephemeral PG).

**Exit criteria:**

- `pgevolve_core::catalog::CatalogQuerier` trait exists with a clean async-free abstraction (the trait accepts already-fetched rows; the binary's `tokio-postgres` adapter does the I/O).
- For each PG major (14, 15, 16, 17): a complete query set produces correct IR for tables, columns, constraints, indexes, sequences, comments.
- `Catalog::canonicalize()` is applied to catalog output so source-IR and catalog-IR have identical sort/dedup invariants.
- The `pgevolve` schema, schemas not in the managed list, and explicitly ignored objects are filtered out *before* IR construction.
- `pgevolve-testkit::EphemeralPostgres` can spin up containers for each major version (Docker-required tests gated behind an env var so `cargo test` works without Docker).
- Tier-3 golden snapshot harness compares introspected IR to a checked-in YAML/JSON snapshot per fixture. `cargo xtask bless` regenerates goldens.
- For at least one fixture per supported PG major, the pipeline `parse(source.sql) → apply via psql to ephemeral PG → introspect → assert equal IR` succeeds.

---

## File structure introduced this phase

```
crates/pgevolve-core/src/
└── catalog/
    ├── mod.rs                          # CatalogQuerier trait + read_catalog entry point
    ├── error.rs                        # CatalogError
    ├── version.rs                      # PgVersion enum + detection
    ├── filter.rs                       # CatalogFilter (managed/ignored/system)
    ├── rows.rs                         # row types per query
    └── queries/
        ├── mod.rs
        ├── shared.rs                   # SQL strings used across versions
        ├── pg14.rs
        ├── pg15.rs
        ├── pg16.rs
        └── pg17.rs

crates/pgevolve-testkit/src/
├── ephemeral_pg.rs                     # EphemeralPostgres (testcontainers wrapper)
└── catalog_snapshotter.rs              # IR → YAML, snapshot diffing

crates/pgevolve-core/tests/
└── catalog_round_trip.rs               # Tier-3 harness driver

crates/pgevolve-core/tests/fixtures/catalog/
├── pg14/
│   ├── 0001-tables-and-columns/{source.sql, expected.yaml}
│   └── ...
├── pg15/
├── pg16/
└── pg17/

xtask/                                  # workspace member added in this phase
├── Cargo.toml
└── src/main.rs                         # `cargo xtask bless` regenerates goldens
```

---

### Task 3.1: `CatalogQuerier` trait + row types

**Files:**
- Create: `crates/pgevolve-core/src/catalog/mod.rs`
- Create: `crates/pgevolve-core/src/catalog/error.rs`
- Create: `crates/pgevolve-core/src/catalog/rows.rs`
- Modify: `crates/pgevolve-core/src/lib.rs` (add `pub mod catalog;`)
- Modify: `crates/pgevolve-core/src/error.rs` (add `Catalog` variant)

**Design:** the trait is pure-data. The caller (binary) executes parameterized SQL via `tokio-postgres` and returns `Vec<Row>` for each query name. This keeps `pgevolve-core` free of any async/Postgres-driver dependency.

```rust
pub trait CatalogQuerier {
    fn fetch(&self, query: CatalogQuery) -> Result<Vec<Row>, CatalogError>;
}

pub enum CatalogQuery {
    PgVersion,
    Schemas,
    Tables,
    Columns,
    Constraints,
    Indexes,
    Sequences,
    Comments,
    Dependencies,
}

pub struct Row {
    cols: HashMap<String, Value>,
}
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Text(String),
    TextArray(Vec<String>),
    IntegerArray(Vec<i64>),
    SmallInt(i16),
    // add more as needed
}
```

Tests: smoke instantiation, value casting helpers.

Commit: `feat(core): CatalogQuerier trait and row types`

---

### Task 3.2: `PgVersion` detection

**File:** `crates/pgevolve-core/src/catalog/version.rs`

```rust
pub enum PgVersion { Pg14, Pg15, Pg16, Pg17 }

impl PgVersion {
    pub fn detect(querier: &dyn CatalogQuerier) -> Result<Self, CatalogError> {
        let row = querier.fetch(CatalogQuery::PgVersion)?.into_iter().next()
            .ok_or(CatalogError::MissingResult { query: CatalogQuery::PgVersion })?;
        let server_version_num: i64 = row.get_int("server_version_num")?;
        match server_version_num / 10000 {
            14 => Ok(Self::Pg14),
            15 => Ok(Self::Pg15),
            16 => Ok(Self::Pg16),
            17 => Ok(Self::Pg17),
            v  => Err(CatalogError::UnsupportedPgVersion(v as u32)),
        }
    }
}
```

Query name `PgVersion` returns `SHOW server_version_num` (or `SELECT current_setting('server_version_num')::int as server_version_num`).

Tests: with a mock querier that returns a fake row.

Commit: `feat(core): detect Postgres major version from catalog`

---

### Task 3.3: `EphemeralPostgres` in testkit

**File:** `crates/pgevolve-testkit/src/ephemeral_pg.rs`

Wraps `testcontainers` to provide:

```rust
pub struct EphemeralPostgres {
    container: testcontainers::ContainerAsync<GenericImage>,
    dsn: String,
}

impl EphemeralPostgres {
    pub async fn start(version: PgVersion) -> anyhow::Result<Self> { ... }
    pub fn dsn(&self) -> &str { &self.dsn }
    pub async fn exec_sql(&self, sql: &str) -> anyhow::Result<()> { ... }
    pub async fn connect(&self) -> anyhow::Result<tokio_postgres::Client> { ... }
}
```

Image tags:
- PG 14: `postgres:14-alpine`
- PG 15: `postgres:15-alpine`
- PG 16: `postgres:16-alpine`
- PG 17: `postgres:17-alpine`

Env: `POSTGRES_PASSWORD=pgevolve`, `POSTGRES_USER=pgevolve`, `POSTGRES_DB=pgevolve`.

Wait-for-ready: poll `pg_isready` (or via the readiness port of testcontainers).

**Important:** gate Docker-required tests behind `PGEVOLVE_DISABLE_DOCKER_TESTS` env var so `cargo test` works on machines without Docker. Provide a helper:

```rust
pub fn docker_available() -> bool {
    std::env::var("PGEVOLVE_DISABLE_DOCKER_TESTS").is_err()
        && std::process::Command::new("docker").arg("info").output()
            .map(|o| o.status.success()).unwrap_or(false)
}
```

Tests in testkit that require Docker start with:

```rust
#[tokio::test]
async fn smoke_pg16() {
    if !pgevolve_testkit::ephemeral_pg::docker_available() { return; }
    let pg = EphemeralPostgres::start(PgVersion::Pg16).await.unwrap();
    pg.exec_sql("CREATE TABLE foo (id integer);").await.unwrap();
}
```

Commit: `feat(testkit): EphemeralPostgres wrapper for testcontainers`

---

### Task 3.4: Schema query (`pg_namespace`)

**File:** `crates/pgevolve-core/src/catalog/queries/shared.rs`

```sql
-- Q: Schemas
SELECT
  n.oid          AS oid,
  n.nspname      AS name,
  d.description  AS comment
FROM pg_catalog.pg_namespace n
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = n.oid
 AND d.classoid = 'pg_catalog.pg_namespace'::regclass
 AND d.objsubid = 0
WHERE n.nspname NOT IN ('pg_catalog','pg_toast','information_schema','pgevolve')
  AND n.nspname NOT LIKE 'pg\\_temp\\_%' ESCAPE '\\'
  AND n.nspname NOT LIKE 'pg\\_toast\\_temp\\_%' ESCAPE '\\'
  AND n.nspname = ANY($1::text[])  -- managed schemas filter
ORDER BY n.nspname;
```

Function `read_schemas(querier, managed: &[Identifier]) -> Result<Vec<Schema>, CatalogError>`.

Tests with `EphemeralPostgres`: create a few schemas + comments → introspect → assert IR.

Commit: `feat(core): catalog query for schemas`

---

### Task 3.5: Tables and columns queries

**File:** `crates/pgevolve-core/src/catalog/queries/shared.rs`

Two queries:

- **Q: Tables** — joins `pg_class` (relkind='r') with `pg_namespace`; filter to managed schemas. Returns `(oid, schema, name, comment)`.
- **Q: Columns** — joins `pg_attribute` (`attnum > 0`, `not attisdropped`) with `pg_attrdef`, `pg_type`, `pg_collation`. Returns one row per column with: `(table_oid, attnum, name, type_oid, typmod, atttypid::regtype::text AS pg_type_string, attnotnull, default_expr, identity_kind, generated_kind, generated_expr, collation_schema, collation_name, comment)`.

Build `Column` from the row:
- `ty = ColumnType::parse_from_pg_type_string(pg_type_string)`. The `pg_type_string` from `regtype::text` already includes typmod (e.g., `varchar(50)`, `numeric(10,2)`).
- `default = parse_default_expr(default_expr, &ty)?` — calls phase-2's `NormalizedExpr::from_pg_node` after parsing the default text via `pg_query`.
- Identity: pre-PG14 lacks `attidentity`; from PG14 we have `'a'` for ALWAYS, `'d'` for BY DEFAULT.
- Generated: `attgenerated = 's'` means STORED.

Test: a table with mixed-type columns including `serial`, `numeric(10,2)`, `varchar(50)`, `timestamptz NOT NULL DEFAULT now()` → introspect → IR matches expectations.

Commit: `feat(core): catalog queries for tables and columns`

---

### Task 3.6: Constraints query (`pg_constraint`)

**File:** `crates/pgevolve-core/src/catalog/queries/shared.rs`

Joins:

```sql
SELECT
  c.oid           AS oid,
  c.conname       AS name,
  n.nspname       AS schema,
  cl.relname      AS table_name,
  cln.nspname     AS table_schema,
  c.contype       AS kind,         -- p=primary key, u=unique, f=fk, c=check, x=exclusion
  c.condeferrable AS deferrable,
  c.condeferred   AS deferred,
  c.conkey        AS columns,      -- int2[] of attnums
  c.confkey       AS fk_columns,   -- int2[] of fk attnums on referenced table
  fcl.relname     AS fk_table,
  fcln.nspname    AS fk_schema,
  c.confupdtype   AS on_update,
  c.confdeltype   AS on_delete,
  c.confmatchtype AS match_type,
  c.connoinherit  AS no_inherit,
  pg_catalog.pg_get_constraintdef(c.oid, true) AS constraint_def,
  d.description   AS comment
FROM pg_catalog.pg_constraint c
JOIN pg_catalog.pg_namespace n  ON n.oid  = c.connamespace
JOIN pg_catalog.pg_class     cl ON cl.oid = c.conrelid
JOIN pg_catalog.pg_namespace cln ON cln.oid = cl.relnamespace
LEFT JOIN pg_catalog.pg_class     fcl  ON fcl.oid  = c.confrelid
LEFT JOIN pg_catalog.pg_namespace fcln ON fcln.oid = fcl.relnamespace
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = c.oid
 AND d.classoid = 'pg_catalog.pg_constraint'::regclass
WHERE c.contype IN ('p','u','f','c')
  AND cln.nspname = ANY($1::text[])
ORDER BY n.nspname, c.conname;
```

Building `ConstraintKind::Check`: parse `pg_get_constraintdef` text (which is `CHECK ((expr))`) — extract the expression, run through `NormalizedExpr::from_pg_node`.

`columns` (`conkey`) is an array of `attnum`s — convert via the column query's table-wide attnum→name map.

Tests: table with PK, UNIQUE, FK with CASCADE, CHECK. PG-15-and-up `nulls_not_distinct` for unique constraints (look up via `pg_index.indnullsnotdistinct`).

Commit: `feat(core): catalog query for constraints (PK, UNIQUE, FK, CHECK)`

---

### Task 3.7: Indexes query (`pg_index`)

**File:** `crates/pgevolve-core/src/catalog/queries/shared.rs`

```sql
SELECT
  c.oid              AS oid,
  c.relname          AS name,
  n.nspname          AS schema,
  tc.relname         AS table_name,
  tn.nspname         AS table_schema,
  am.amname          AS method,
  i.indisunique      AS unique,
  i.indnullsnotdistinct AS nulls_not_distinct,  -- PG 15+
  i.indkey           AS column_attnums,
  i.indexprs         AS expression_tree,
  i.indpred          AS predicate_tree,
  i.indoption        AS option_per_column,      -- bit field per column
  i.indcollation     AS collation_per_column,   -- collation oid per column
  i.indclass         AS opclass_per_column,     -- opclass oid per column
  i.indnatts         AS total_columns,
  i.indnkeyatts      AS key_columns,            -- columns before INCLUDE
  pg_catalog.pg_get_indexdef(c.oid, 0, true) AS indexdef,
  d.description      AS comment
FROM pg_catalog.pg_index i
JOIN pg_catalog.pg_class     c  ON c.oid  = i.indexrelid
JOIN pg_catalog.pg_namespace n  ON n.oid  = c.relnamespace
JOIN pg_catalog.pg_class     tc ON tc.oid = i.indrelid
JOIN pg_catalog.pg_namespace tn ON tn.oid = tc.relnamespace
JOIN pg_catalog.pg_am        am ON am.oid = c.relam
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = c.oid
 AND d.classoid = 'pg_catalog.pg_class'::regclass
WHERE n.nspname = ANY($1::text[])
  -- Skip indexes that back constraints (we'll get those from pg_constraint).
  AND NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_constraint cc
    WHERE cc.conindid = i.indexrelid
  )
ORDER BY n.nspname, c.relname;
```

The trickiest part: parsing `indkey` (column attnums where 0 means "expression at index N in `indexprs`"), `indoption` (bit flags for ASC/DESC, NULLS FIRST/LAST per column), `indcollation`, `indclass`. Use `pg_get_indexdef(idx, col_no, true)` per-column to get clean per-column SQL strings as a fallback for complex cases.

PG 15+ exposes `indnullsnotdistinct`. For PG 14, the column doesn't exist — branch in query selection (Task 3.12).

Tests cover bare btree, partial, INCLUDE, expression index, opclass, sort/nulls order.

Commit: `feat(core): catalog query for indexes`

---

### Task 3.8: Sequences query

**File:** `crates/pgevolve-core/src/catalog/queries/shared.rs`

```sql
SELECT
  c.oid          AS oid,
  c.relname      AS name,
  n.nspname      AS schema,
  s.seqtypid     AS data_type_oid,
  s.seqstart     AS start,
  s.seqincrement AS increment,
  s.seqmin       AS min_value,
  s.seqmax       AS max_value,
  s.seqcache     AS cache,
  s.seqcycle     AS cycle,
  d.description  AS comment
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_sequence  s ON s.seqrelid = c.oid
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = c.oid
 AND d.classoid = 'pg_catalog.pg_class'::regclass
WHERE c.relkind = 'S'
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, c.relname;
```

Tests: standalone `CREATE SEQUENCE`; `BIGSERIAL` column → introspect both the sequence and the column → see them paired by phase 3.9 (dependencies).

Commit: `feat(core): catalog query for sequences`

---

### Task 3.9: SERIAL/IDENTITY linkage via `pg_depend`

**File:** `crates/pgevolve-core/src/catalog/queries/shared.rs`

```sql
SELECT
  c.relname           AS sequence_name,
  cn.nspname          AS sequence_schema,
  refclass.relname    AS owner_table,
  refn.nspname        AS owner_schema,
  a.attname           AS owner_column
FROM pg_catalog.pg_depend dep
JOIN pg_catalog.pg_class     c  ON c.oid  = dep.objid
JOIN pg_catalog.pg_namespace cn ON cn.oid = c.relnamespace
JOIN pg_catalog.pg_class     refclass ON refclass.oid = dep.refobjid
JOIN pg_catalog.pg_namespace refn     ON refn.oid     = refclass.relnamespace
JOIN pg_catalog.pg_attribute a
  ON a.attrelid = dep.refobjid
 AND a.attnum   = dep.refobjsubid
WHERE c.relkind = 'S'
  AND dep.classid    = 'pg_catalog.pg_class'::regclass
  AND dep.refclassid = 'pg_catalog.pg_class'::regclass
  AND dep.deptype    = 'a'  -- automatic dependency
  AND cn.nspname = ANY($1::text[])
ORDER BY cn.nspname, c.relname;
```

Apply: for each row, set the owning column's identity/default reference to this sequence, and set the sequence's `owned_by`. This produces the desugared form that matches phase 2's `desugar_serial`.

Tests: `id serial` table → `Sequence.owned_by = Some(...)`, `Column.default = Some(Sequence(...))`.

Commit: `feat(core): wire SERIAL/IDENTITY ownership via pg_depend`

---

### Task 3.10: Comments (already inlined; verify and snapshot)

Comments come back from each per-object query above (`pg_description`). No separate query needed — but verify that schema, table, column, constraint, index, sequence comments all round-trip.

Add a fixture covering all six.

Commit: `test(core): comment round-trip across all v0.1 object kinds`

---

### Task 3.11: `CatalogFilter` (managed/ignored/system)

**File:** `crates/pgevolve-core/src/catalog/filter.rs`

```rust
pub struct CatalogFilter {
    managed_schemas: Vec<Identifier>,
    ignore_globs:    Vec<glob::Pattern>,  // matches against rendered qname
}

impl CatalogFilter {
    pub fn new(managed: Vec<Identifier>, ignores: Vec<String>) -> Result<Self, CatalogError> { ... }

    pub fn managed_schemas_param(&self) -> Vec<&str> {
        // for `$1::text[]` parameter
        self.managed_schemas.iter().map(|i| i.as_str()).collect()
    }

    pub fn allows(&self, qname: &QualifiedName, kind_name: &str) -> bool {
        let rendered = format!("{kind_name}:{qname}");
        // Reject anything matching an ignore glob:
        if self.ignore_globs.iter().any(|p| p.matches(&format!("{qname}")) || p.matches(&rendered)) {
            return false;
        }
        true
    }
}
```

Always exclude `pg_catalog`, `information_schema`, `pg_toast`, `pgevolve` (the metadata schema). Even if the user lists them in `managed`, they're rejected with `CatalogError::CannotManageReservedSchema`.

Tests: glob matching, reserved-schema rejection.

Commit: `feat(core): CatalogFilter combining managed schemas and ignore globs`

---

### Task 3.12: Per-version query branches

**File:** `crates/pgevolve-core/src/catalog/queries/{pg14,pg15,pg16,pg17}.rs`

Most queries are identical across PG 14–17. The known divergences:

- `pg_index.indnullsnotdistinct` exists from PG 15. For PG 14, the query omits the column and the IR field defaults to `false`.
- `pg_constraint` no-inherit handling: same since PG 14.
- (Track future divergences as they land.)

Pattern:

```rust
// pg14.rs
pub const TABLES_QUERY: &str = SHARED_TABLES_QUERY;
pub const INDEXES_QUERY: &str = INDEXES_QUERY_NO_NULLSNOTDISTINCT;
// ...
```

`mod.rs` dispatches on `PgVersion`:

```rust
pub fn query_for(version: PgVersion, q: CatalogQuery) -> &'static str {
    match (version, q) {
        (PgVersion::Pg14, CatalogQuery::Indexes) => pg14::INDEXES_QUERY,
        (_, CatalogQuery::Indexes)               => shared::INDEXES_QUERY,
        // ... and so on
    }
}
```

Tests: each version dispatches the expected query string.

Commit: `feat(core): per-PG-major catalog query dispatch`

---

### Task 3.13: `read_catalog` entry point

**File:** `crates/pgevolve-core/src/catalog/mod.rs`

```rust
pub fn read_catalog(
    querier: &dyn CatalogQuerier,
    filter: &CatalogFilter,
) -> Result<Catalog, CatalogError> {
    let version = PgVersion::detect(querier)?;
    let schemas      = read_schemas(querier, filter)?;
    let tables_meta  = read_tables(querier, filter)?;
    let columns      = read_columns(querier, filter, version)?;
    let constraints  = read_constraints(querier, filter, version)?;
    let indexes      = read_indexes(querier, filter, version)?;
    let sequences    = read_sequences(querier, filter)?;
    let dependencies = read_dependencies(querier, filter)?;

    let mut catalog = assemble_catalog(schemas, tables_meta, columns, constraints, indexes, sequences, dependencies)?;
    catalog.canonicalize()?;
    Ok(catalog)
}
```

`assemble_catalog` is the gluing logic that turns raw row collections into the IR — pairs columns with their tables, constraints with their tables, sequences with their owning columns, etc.

Tests: integration test using `EphemeralPostgres` + a hand-crafted SQL fixture, check the resulting `Catalog` matches expectations.

Commit: `feat(core): read_catalog entry point assembling all queries into IR`

---

### Task 3.14: `tokio-postgres` adapter in the binary

**File:** `crates/pgevolve/src/querier_pg.rs`

```rust
pub struct PgCatalogQuerier {
    client: tokio_postgres::Client,
}

impl PgCatalogQuerier {
    pub async fn connect(dsn: &str) -> anyhow::Result<Self> { ... }
}

impl CatalogQuerier for PgCatalogQuerier {
    fn fetch(&self, q: CatalogQuery) -> Result<Vec<Row>, CatalogError> {
        // Run synchronously by blocking on the runtime's current_thread handle.
        // Since `pgevolve-core` is sync-API, the binary uses block_in_place + a
        // shared runtime. Document this clearly.
        let sql = catalog::queries::query_for(self.version, q);
        let params = self.params_for(q);
        let rows = self.runtime.block_on(self.client.query(sql, &params))?;
        Ok(rows.into_iter().map(Row::from_pg_row).collect())
    }
}
```

(Alternative: make `CatalogQuerier::fetch` async and require an `async fn` everywhere. v0.1 keeps it sync to make `pgevolve-core` runtime-agnostic — the binary owns the async runtime.)

Tests: connect to `EphemeralPostgres`, run each query, verify rows convert.

Commit: `feat(cli): tokio-postgres CatalogQuerier adapter`

---

### Task 3.15: `xtask` workspace member for `bless` and other dev tooling

**Files:**
- Modify: `Cargo.toml` (add `"xtask"` to workspace members)
- Create: `xtask/Cargo.toml`
- Create: `xtask/src/main.rs`

```rust
fn main() -> anyhow::Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "bless" => bless::run(),  // regenerates Tier-3 goldens
        _ => {
            eprintln!("usage: cargo xtask <bless>");
            std::process::exit(2);
        }
    }
}
```

Add `[alias.xtask]` in `.cargo/config.toml`:

```toml
[alias]
xtask = "run --quiet --package xtask --"
```

`cargo xtask bless`: walks `tests/fixtures/catalog/<pgN>/`, applies each `source.sql` to a fresh `EphemeralPostgres` of the matching version, runs `read_catalog`, serializes the IR to YAML, writes `expected.yaml`.

Commit: `feat(xtask): add xtask binary with `bless` for tier-3 goldens`

---

### Task 3.16: Tier-3 round-trip golden harness

**File:** `crates/pgevolve-core/tests/catalog_round_trip.rs`

Walks `tests/fixtures/catalog/<pgN>/`. For each fixture:

1. Skip if `!docker_available()`.
2. Start `EphemeralPostgres::start(Pg<N>)`.
3. Execute `source.sql`.
4. Connect a `PgCatalogQuerier` (note: this means the test depends on the `pgevolve` binary crate; alternatively factor `PgCatalogQuerier` into a small library `pgevolve-pg-driver` shared between binary and tests — recommended).
5. Run `read_catalog`.
6. Serialize to canonical YAML.
7. Compare to `expected.yaml`. On `cargo xtask bless`, overwrite; otherwise assert equal.

Seed fixtures (one per PG version, replicated across versions where behavior matches):

- `0001-tables-and-columns/source.sql` — all in-scope column types.
- `0002-constraints/source.sql` — PK, UNIQUE, FK with CASCADE, CHECK.
- `0003-indexes/source.sql` — btree, unique, partial, INCLUDE, expression.
- `0004-serial-and-identity/source.sql` — `SERIAL`, `IDENTITY ALWAYS`, `IDENTITY BY DEFAULT`.
- `0005-multi-schema/source.sql` — objects across two managed schemas.
- `0006-comments/source.sql` — comments on every kind.
- `0007-pg15-nulls-not-distinct/source.sql` — pg15+ only; pg14 fixture file omits this case.

Commit: `test(core): tier-3 catalog round-trip golden harness with seed fixtures`

---

### Task 3.17: Phase 3 self-review

- [ ] `cargo test -p pgevolve-core` passes (Tier 1 + 2 always; Tier 3 if Docker available).
- [ ] CI workflow now optionally runs the PG matrix — uncomment the `pg-matrix` job in `.github/workflows/ci.yml` and confirm it passes for at least PG 16 (the others may be enabled progressively as fixtures are added).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Run `cargo xtask bless` once and confirm goldens are stable across reruns.

Phase 3 complete.
