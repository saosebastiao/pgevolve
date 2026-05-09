# Phase 2 — Source parser & loader

**Goal:** Convert directories of `CREATE`-style DDL into a fully populated `Catalog` IR using `pg_query.rs`. Produce a Tier-2 fixture corpus harness so future parser bugs become test cases.

**Spec coverage:** §6.1 (source loader), §5.3 (DefaultExpr / NormalizedExpr — full implementation), §5.5 (SERIAL desugaring), §14 (Tier-2).

**Depends on:** Phase 1 complete.

**Exit criteria:**

- `pgevolve_core::parse::parse_directory(path: &Path) -> Result<Catalog, ParseError>` produces a fully populated IR for a multi-file DDL tree.
- Every AST node kind in §6.1's whitelist has a working builder.
- SERIAL / BIGSERIAL desugar into `(Column { default: Sequence(...) }, Sequence { owned_by: Some(...) })` pairs that match what the catalog reader will produce in phase 3.
- `-- @pgevolve schema=<name>` directives are recognized.
- `tests/fixtures/parser/equivalent_pairs/`, `different_pairs/`, `parse_errors/` corpora are wired with a harness that runs every fixture as a parameterized test.
- All non-MVP CREATEs (views, functions, types, triggers, etc.) produce a clear `ParseError::UnsupportedObjectKind` with phase-2 message.

---

## File structure introduced this phase

```
crates/pgevolve-core/src/
├── lib.rs                              # add `pub mod parse;`
└── parse/
    ├── mod.rs                          # re-exports + parse_directory entry point
    ├── error.rs                        # ParseError + SourceLocation
    ├── directives.rs                   # -- @pgevolve directive parser
    ├── normalize_expr.rs               # NormalizedExpr full impl (replaces phase-1 stub)
    ├── statement.rs                    # Statement classification dispatch
    └── builder/
        ├── mod.rs
        ├── create_schema_stmt.rs
        ├── create_stmt.rs              # CREATE TABLE
        ├── create_seq_stmt.rs
        ├── index_stmt.rs
        ├── alter_table_stmt.rs
        ├── comment_stmt.rs
        ├── desugar_serial.rs           # SERIAL → integer + Sequence + default
        └── shared.rs                   # qualified-name resolution, type parsing helpers

crates/pgevolve-core/tests/
└── parser_corpus.rs                    # Tier-2 harness driver

crates/pgevolve-core/tests/fixtures/parser/
├── equivalent_pairs/
│   ├── 0001-int-aliases/{a.sql,b.sql,note.txt}
│   ├── 0002-serial-desugar/{a.sql,b.sql,note.txt}
│   └── ...
├── different_pairs/
│   ├── 0001-varchar-len/{a.sql,b.sql,expected.txt}
│   └── ...
└── parse_errors/
    ├── 0001-unsupported-view.sql
    └── 0001-unsupported-view.expected.txt
```

---

### Task 2.1: `ParseError`, `SourceLocation`, smoke test against `pg_query.rs`

**Files:**
- Create: `crates/pgevolve-core/src/parse/mod.rs`
- Create: `crates/pgevolve-core/src/parse/error.rs`
- Modify: `crates/pgevolve-core/src/lib.rs` (add `pub mod parse;`)
- Modify: `crates/pgevolve-core/src/error.rs` (add `Parse` variant)

- [ ] **Step 1: Write `ParseError`**

`crates/pgevolve-core/src/parse/error.rs`:

```rust
//! Errors raised by the source parser.

use std::path::PathBuf;

use thiserror::Error;

/// Position within a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// Path to the file (relative to the source root, when available).
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column.
    pub column: usize,
}

impl SourceLocation {
    /// Construct.
    pub fn new(file: PathBuf, line: usize, column: usize) -> Self {
        Self { file, line, column }
    }
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.column)
    }
}

/// Errors raised by the source parser.
#[derive(Debug, Error)]
pub enum ParseError {
    /// I/O error while reading a source file.
    #[error("I/O error reading {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// `pg_query` rejected the SQL.
    #[error("pg_query parse error at {location}: {message}")]
    PgQuery {
        /// Source location.
        location: SourceLocation,
        /// Message from pg_query.
        message: String,
    },

    /// CREATE was for an object kind not supported in v0.1.
    #[error("{location}: {kind} is not supported in pgevolve v0.1 — see docs §2 for the v0.1 object-kind list; expected to land in a later phase")]
    UnsupportedObjectKind {
        /// Source location.
        location: SourceLocation,
        /// Object kind name (e.g., "CREATE VIEW").
        kind: &'static str,
    },

    /// CREATE was missing required schema qualification and no `-- @pgevolve schema=...` directive applied.
    #[error("{location}: object name must be schema-qualified, or the file must declare `-- @pgevolve schema=<name>`")]
    UnqualifiedName {
        /// Source location.
        location: SourceLocation,
    },

    /// Generic structural error during AST → IR conversion.
    #[error("{location}: {message}")]
    Structural {
        /// Source location.
        location: SourceLocation,
        /// Diagnostic message.
        message: String,
    },

    /// IR construction failed (e.g., invalid identifier in source).
    #[error("{location}: {source}")]
    Ir {
        /// Source location.
        location: SourceLocation,
        /// Underlying error.
        #[source]
        source: crate::ir::IrError,
    },

    /// A directive was malformed.
    #[error("{location}: invalid pgevolve directive: {message}")]
    InvalidDirective {
        /// Source location.
        location: SourceLocation,
        /// Diagnostic.
        message: String,
    },

    /// Two definitions of the same object qname were found.
    #[error("duplicate object {qname} defined at {first} and {second}")]
    DuplicateObject {
        /// Object qname (rendered).
        qname: String,
        /// First definition location.
        first: SourceLocation,
        /// Second definition location.
        second: SourceLocation,
    },
}
```

- [ ] **Step 2: Add `parse/mod.rs`**

```rust
//! Source-side parser: SQL bytes → IR.

pub mod error;

pub use error::{ParseError, SourceLocation};

/// Smoke test: parse a single statement string.
#[cfg(test)]
pub(crate) fn smoke_parse(sql: &str) -> Result<pg_query::ParseResult, pg_query::Error> {
    pg_query::parse(sql)
}
```

- [ ] **Step 3: Add `Parse` to top-level `Error`**

`crates/pgevolve-core/src/error.rs`:

```rust
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Ir(#[from] crate::ir::IrError),
    #[error(transparent)]
    Parse(#[from] crate::parse::ParseError),
}
```

- [ ] **Step 4: Wire `parse` module**

`crates/pgevolve-core/src/lib.rs` — add:

```rust
pub mod parse;
```

- [ ] **Step 5: Smoke test**

Add to `parse/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_query_round_trips_a_create_table() {
        let sql = "CREATE TABLE app.users (id integer);";
        let result = smoke_parse(sql).expect("pg_query parses");
        // Smoke check: the parse tree contains at least one statement.
        assert!(!result.protobuf.stmts.is_empty());
    }

    #[test]
    fn pg_query_reports_syntax_errors() {
        let sql = "CREATE TABLE !bad!;";
        assert!(smoke_parse(sql).is_err());
    }
}
```

- [ ] **Step 6: Run**

```bash
cargo test -p pgevolve-core --lib parse
```

Expected: 2 passing tests. (You may need to consult the actual `pg_query` 5.x API for `ParseResult` / `protobuf.stmts` field names; adjust to match.)

- [ ] **Step 7: Commit**

```bash
git add crates/pgevolve-core
git commit -m "feat(core): wire pg_query.rs and add ParseError type

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2.2: `Statement` classifier + dispatch

**File:** `crates/pgevolve-core/src/parse/statement.rs`

Define a `Statement` enum that classifies a `pg_query::NodeEnum` into the v0.1 whitelist. Returns `ParseError::UnsupportedObjectKind` for non-whitelisted kinds with a friendly object-kind name.

```rust
pub enum Statement {
    CreateSchema(pg_query::protobuf::CreateSchemaStmt),
    CreateTable(pg_query::protobuf::CreateStmt),
    CreateSequence(pg_query::protobuf::CreateSeqStmt),
    CreateIndex(pg_query::protobuf::IndexStmt),
    AlterTable(pg_query::protobuf::AlterTableStmt),
    Comment(pg_query::protobuf::CommentStmt),
}

impl Statement {
    pub fn classify(node: pg_query::NodeEnum, location: SourceLocation) -> Result<Self, ParseError> {
        use pg_query::NodeEnum::*;
        match node {
            CreateSchemaStmt(s) => Ok(Self::CreateSchema(s)),
            CreateStmt(s)        => Ok(Self::CreateTable(s)),
            CreateSeqStmt(s)     => Ok(Self::CreateSequence(s)),
            IndexStmt(s)         => Ok(Self::CreateIndex(s)),
            AlterTableStmt(s)    => Ok(Self::AlterTable(s)),
            CommentStmt(s)       => Ok(Self::Comment(s)),
            // Hard-error on every non-MVP CREATE kind:
            ViewStmt(_)              => Err(unsupported(location, "CREATE VIEW")),
            CreateFunctionStmt(_)    => Err(unsupported(location, "CREATE FUNCTION/PROCEDURE")),
            CreateTrigStmt(_)        => Err(unsupported(location, "CREATE TRIGGER")),
            CreateEnumStmt(_)        => Err(unsupported(location, "CREATE TYPE ... AS ENUM")),
            CreateRangeStmt(_)       => Err(unsupported(location, "CREATE TYPE ... AS RANGE")),
            CompositeTypeStmt(_)     => Err(unsupported(location, "CREATE TYPE ... AS (...)")),
            CreateDomainStmt(_)      => Err(unsupported(location, "CREATE DOMAIN")),
            CreateExtensionStmt(_)   => Err(unsupported(location, "CREATE EXTENSION")),
            CreatePolicyStmt(_)      => Err(unsupported(location, "CREATE POLICY")),
            CreateForeignTableStmt(_)=> Err(unsupported(location, "CREATE FOREIGN TABLE")),
            CreateFdwStmt(_)         => Err(unsupported(location, "CREATE FOREIGN DATA WRAPPER")),
            CreateRoleStmt(_)        => Err(unsupported(location, "CREATE ROLE")),
            GrantStmt(_)             => Err(unsupported(location, "GRANT/REVOKE")),
            // Anything else — fall through with a generic message.
            other => Err(unsupported(location, leak_node_name(&other))),
        }
    }
}

fn unsupported(location: SourceLocation, kind: &'static str) -> ParseError {
    ParseError::UnsupportedObjectKind { location, kind }
}

fn leak_node_name(_n: &pg_query::NodeEnum) -> &'static str {
    "this statement kind"
}
```

> Note: pg_query's `NodeEnum` variant names may differ from the above sketch. Use `cargo doc --open -p pg_query` or the crate's source to find the exact names. If something is named differently (e.g., `CreateForeignServerStmt`), update accordingly.

Tests in `tests` module:
- `CREATE TABLE` classifies as `CreateTable`.
- `CREATE VIEW` returns `UnsupportedObjectKind`.
- `CREATE FUNCTION` returns `UnsupportedObjectKind`.

Commit: `feat(core): classify pg_query AST into v0.1 whitelist; reject other kinds`

---

### Task 2.3: `-- @pgevolve` directive parser

**File:** `crates/pgevolve-core/src/parse/directives.rs`

Directives are SQL line-comments of the form `-- @pgevolve <key>=<value>[ <key>=<value>...]`.

For phase 2 we need just one directive: `schema=<name>` for file-level default schemas. Other directives (`step=`, `group=`, etc.) are emitted by the planner in phase 7 — but since the parser must *ignore* them in source files (they're for plans, not source), we recognize them and silently skip them here when in source-parsing mode.

```rust
pub struct FileDirectives {
    pub schema: Option<Identifier>,
}

pub fn extract_file_directives(sql: &str, file: &Path) -> Result<FileDirectives, ParseError> {
    let mut out = FileDirectives { schema: None };
    for (line_no, raw_line) in sql.lines().enumerate() {
        let trimmed = raw_line.trim();
        // Stop at the first non-empty, non-comment line — directives must be in the header block.
        if trimmed.is_empty() { continue; }
        let Some(rest) = trimmed.strip_prefix("--") else { break; };
        let rest = rest.trim();
        let Some(payload) = rest.strip_prefix("@pgevolve") else { continue; };
        let payload = payload.trim();
        for kv in payload.split_whitespace() {
            let Some((k, v)) = kv.split_once('=') else {
                return Err(ParseError::InvalidDirective {
                    location: SourceLocation::new(file.into(), line_no + 1, 1),
                    message: format!("expected `key=value`, got {kv:?}"),
                });
            };
            match k {
                "schema" => {
                    let id = Identifier::from_unquoted(v).map_err(|e| ParseError::InvalidDirective {
                        location: SourceLocation::new(file.into(), line_no + 1, 1),
                        message: format!("invalid schema identifier: {e}"),
                    })?;
                    out.schema = Some(id);
                }
                // Plan-format directives — ignored when reading source.
                "plan" | "step" | "group" | "kind" | "destructive" | "intent_id"
                | "version" | "created" | "source_rev" | "target" | "intents_required"
                | "transactional" | "targets" => {}
                _ => return Err(ParseError::InvalidDirective {
                    location: SourceLocation::new(file.into(), line_no + 1, 1),
                    message: format!("unknown directive key: {k}"),
                }),
            }
        }
    }
    Ok(out)
}
```

Tests:
- `-- @pgevolve schema=app` → `schema = Some("app")`.
- Header with multiple comment lines, last one has the directive → recognized.
- Directive after the first non-comment line → ignored (parser stops scanning).
- `-- @pgevolve schema=` → `InvalidDirective`.
- `-- @pgevolve schema=Foo` → recognized as `Identifier` (lowercased).
- `-- @pgevolve unknown=x` → `InvalidDirective`.

Commit: `feat(core): parse -- @pgevolve file directives`

---

### Task 2.4: Finalize `NormalizedExpr` against pg_query AST

**File:** `crates/pgevolve-core/src/parse/normalize_expr.rs` (new file; replaces phase-1 stub in `ir/default_expr.rs`)

Implement these passes over a pg_query `Node`:

1. **Strip redundant casts to a target type.** If the AST is `TypeCast { arg, type_name }` and `type_name` matches the column's own type, replace with `arg`.
2. **Fold parens.** pg_query represents parens as nested `A_Expr` nodes — collapse trivial nesting.
3. **Sort commutative operators.** For `+`, `*`, `AND`, `OR`, sort children by their canonical text form.
4. **Lowercase reserved keywords.** Apply when computing canonical text.
5. **Compute canonical text via `pg_query.deparse`.** Then compute `BLAKE3(canonical_text.as_bytes())` for the hash.

```rust
pub struct NormalizedExpr {
    pub canonical_text: String,
    pub ast_hash: [u8; 32],
}

impl NormalizedExpr {
    pub fn from_pg_node(node: &pg_query::NodeEnum, target_type: Option<&ColumnType>) -> Result<Self, ParseError> {
        let normalized_node = normalize(node.clone(), target_type);
        let canonical_text = pg_query::deparse(/* wrap in expr context */)?;
        let ast_hash = blake3::hash(canonical_text.as_bytes()).into();
        Ok(Self { canonical_text, ast_hash })
    }

    pub fn from_canonical_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let ast_hash = blake3::hash(text.as_bytes()).into();
        Self { canonical_text: text, ast_hash }
    }
}

impl PartialEq for NormalizedExpr {
    fn eq(&self, other: &Self) -> bool { self.ast_hash == other.ast_hash }
}
impl Eq for NormalizedExpr {}
```

The actual normalization functions are non-trivial. v0.1 acceptable scope:
- Implement (1) cast stripping for the column's own type.
- Implement (4) keyword lowercasing on the canonical text post-deparse.
- (2), (3) are deferred to phase-2 follow-up issues; they only affect equivalence-detection sensitivity, not correctness — without them, some equivalent expressions will diff. Document this in a code comment with a follow-up issue link.

Tests:
- `42 :: integer` for an integer column → equivalent to `42`.
- `'foo' :: text` for a text column → equivalent to `'foo'`.
- `LOWER('FOO')` → canonical text uses `lower('FOO')` (lowercased keyword).

Commit: `feat(core): NormalizedExpr with cast stripping and keyword lowercasing`

---

### Task 2.5: `CreateStmt` → `Table` builder

**File:** `crates/pgevolve-core/src/parse/builder/create_stmt.rs`

Walks the pg_query `CreateStmt`. Extracts:

- Schema-qualified `qname` (validate qualification per directive rules).
- Each column from `tableElts`:
  - `name` (`Identifier`)
  - `ty` (string-form via `pg_query`'s `TypeName` deparse, then `ColumnType::parse_from_pg_type_string`)
  - `nullable` (default true; flips false on `NOT NULL` constraint or PK)
  - `default` from any `DEFAULT` constraint
  - Inline `PRIMARY KEY`, `UNIQUE`, `REFERENCES`, `CHECK` move into `Table.constraints`
  - Inline `IDENTITY` moves into `Column.identity`
  - `GENERATED ... AS (...) STORED` moves into `Column.generated`
- Standalone constraint clauses (`CONSTRAINT name PRIMARY KEY (...)`) into `Table.constraints`.

Tests cover:
- Single-column `id integer` table.
- Composite `PRIMARY KEY (a, b)`.
- Inline `REFERENCES`.
- Inline `CHECK`.
- `NOT NULL` flipping `nullable`.
- `DEFAULT now()` producing `DefaultExpr::Expr`.
- `DEFAULT 0` producing `DefaultExpr::Literal(Integer(0))`.
- `DEFAULT nextval('app.seq1')` producing `DefaultExpr::Sequence`.

Commit: `feat(core): build Table IR from CreateStmt with inline constraints, defaults, and identity`

---

### Task 2.6: `IndexStmt` → `Index` builder

**File:** `crates/pgevolve-core/src/parse/builder/index_stmt.rs`

Extract:
- Index qname (use the table's schema if not specified — index names share the table's schema in Postgres).
- Method (`USING btree` etc.).
- Each `IndexColumn` with `expr` (column name or expression), `collation`, `opclass`, `sort_order`, `nulls_order`.
- `INCLUDE` columns.
- `unique`.
- `nulls_not_distinct` (PG 15+).
- `WHERE` predicate as `NormalizedExpr`.

Tests cover: bare btree, unique, partial, INCLUDE, expression index (`lower(email)`), opclass (`text_pattern_ops`), nulls-not-distinct unique.

Commit: `feat(core): build Index IR from IndexStmt`

---

### Task 2.7: `CreateSeqStmt` → `Sequence` builder

**File:** `crates/pgevolve-core/src/parse/builder/create_seq_stmt.rs`

Extract `qname`, `data_type`, `start`, `increment`, `min_value`, `max_value`, `cache`, `cycle`, `owned_by`. Sequence option defaults match Postgres defaults (`start=1`, `increment=1`, `cache=1`, etc.).

Commit: `feat(core): build Sequence IR from CreateSeqStmt`

---

### Task 2.8: `CreateSchemaStmt` → `Schema` builder

**File:** `crates/pgevolve-core/src/parse/builder/create_schema_stmt.rs`

Trivial — schema name + optional comment.

Commit: `feat(core): build Schema IR from CreateSchemaStmt`

---

### Task 2.9: `AlterTableStmt` builder (FK forward-reference whitelist)

**File:** `crates/pgevolve-core/src/parse/builder/alter_table_stmt.rs`

Source SQL is declarative — ALTERs are usually an error. The one allowed case is `ALTER TABLE ... ADD CONSTRAINT <name> FOREIGN KEY (...)` for forward-referencing FKs (the pattern shown in spec §6.4 cycle handling: when two tables FK each other, you can't define both inline).

Behavior:
- If the alter is `AddConstraint(ForeignKey)`: append the constraint to the target `Table`'s `constraints` list. Order doesn't matter — the IR is canonicalized later.
- Any other alter subtype: `ParseError::Structural` with message about source-as-declarative.

Tests:
- Allowed: `ALTER TABLE app.invoices ADD CONSTRAINT invoices_customer_fk FOREIGN KEY (customer_id) REFERENCES app.customers(id);`
- Rejected: `ALTER TABLE app.users DROP COLUMN email;` → structural error.
- Rejected: `ALTER TABLE app.users ADD COLUMN email text;` → structural error with message pointing to source-as-declarative.

Commit: `feat(core): allow ALTER TABLE ADD CONSTRAINT for forward-ref FKs only`

---

### Task 2.10: `CommentStmt` builder

**File:** `crates/pgevolve-core/src/parse/builder/comment_stmt.rs`

`COMMENT ON TABLE app.users IS 'foo';` → look up the table in the partial Catalog and set its `comment`. Also handles `COMMENT ON COLUMN`, `COMMENT ON INDEX`, `COMMENT ON SCHEMA`, `COMMENT ON SEQUENCE`, `COMMENT ON CONSTRAINT`.

Tests for each kind.

Commit: `feat(core): apply CommentStmt to target IR object`

---

### Task 2.11: SERIAL desugaring

**File:** `crates/pgevolve-core/src/parse/builder/desugar_serial.rs`

When a column is declared as `serial`, `serial4`, `bigserial`, `serial8`, `smallserial`, or `serial2`:

1. Replace the column's type with `Integer` / `BigInt` / `SmallInt`.
2. Set `nullable = false`.
3. Synthesize a `Sequence` named `<table>_<col>_seq` in the same schema (Postgres's exact naming convention); add it to the partial Catalog.
4. Set `Column.default = Some(DefaultExpr::Sequence(synthesized_seq_qname))`.
5. Set `Sequence.owned_by = Some(SequenceOwner { table, column })`.

The catalog reader (phase 3) will produce identical IR from `pg_class` + `pg_attribute` + `pg_depend`, so both sides converge. **This is critical for the diff property: the same logical schema written in either form must produce identical IR.**

Tests:
- `id serial PRIMARY KEY` produces `(Column { ty: Integer, nullable: false, default: Sequence(...) }, Sequence { ... owned_by: Some(...) })`.
- `id bigserial` produces `BigInt`.
- `id smallserial` produces `SmallInt`.
- Equivalence: `id serial` and `id integer NOT NULL DEFAULT nextval('users_id_seq')` plus a separate `CREATE SEQUENCE users_id_seq OWNED BY users.id;` produce equivalent IR. This becomes a Tier-2 fixture in `equivalent_pairs/`.

Commit: `feat(core): desugar SERIAL into integer + sequence + default to match catalog form`

---

### Task 2.12: `parse_directory` entry point

**File:** `crates/pgevolve-core/src/parse/mod.rs`

Public function:

```rust
pub fn parse_directory(root: &Path, ignores: &[glob::Pattern]) -> Result<Catalog, ParseError>;
```

Steps:
1. Walk `root` recursively, collecting every `*.sql` file path (deterministic order via `walkdir` + sort by path).
2. For each file: read bytes, extract `FileDirectives`, run `pg_query::parse`, classify each statement, dispatch to the appropriate builder (with `directives.schema` as default schema).
3. Accumulate into a partial `Catalog`. Track each object's source location for duplicate detection.
4. After all files: run `Catalog::canonicalize()` (sorts vecs, returns `DuplicateObject` error if any qname collides).
5. Return.

Tests use `tempfile::tempdir()` to build a small directory and verify the resulting `Catalog` has the expected objects.

Add `walkdir` and `glob` to `pgevolve-core` dependencies; `tempfile` to dev-dependencies.

Commit: `feat(core): parse_directory recursively walks and parses .sql files into Catalog IR`

---

### Task 2.13: Tier-2 fixture corpus harness

**File:** `crates/pgevolve-core/tests/parser_corpus.rs`

A single integration test driver that walks `tests/fixtures/parser/` and runs every fixture as a sub-test using `libtest_mimic` (or a manual `#[test]`-per-fixture approach with `include_dir!`).

Three fixture kinds:

**`equivalent_pairs/<NNNN>-<slug>/`:**
- `a.sql` and `b.sql` — two snippets that should produce identical IR.
- `note.txt` — human description of why they're equivalent.
- Test: parse both, assert `a_ir.canonical_eq(&b_ir)`. If different, dump the structured diff.

**`different_pairs/<NNNN>-<slug>/`:**
- `a.sql` and `b.sql` — two snippets that should produce *different* IR.
- `expected.txt` — substring(s) that must appear in the rendered `Difference` output.
- Test: parse both, assert at least one `Difference`, assert each `expected.txt` substring is present in the diff output.

**`parse_errors/<NNNN>-<slug>.sql`:**
- One file containing input that should fail to parse.
- `<NNNN>-<slug>.expected.txt` — substring(s) that must appear in the error message.
- Test: parse, assert `Err`, assert each substring is present.

Seed corpus (15-20 fixtures to bootstrap):

`equivalent_pairs/`:
- `0001-int-aliases` — `int` vs `integer` vs `int4`.
- `0002-serial-desugar` — `id serial` vs explicit integer + sequence + default.
- `0003-varchar-aliases` — `varchar(50)` vs `character varying(50)`.
- `0004-timestamp-tz` — `timestamptz` vs `timestamp with time zone`.
- `0005-default-cast-strip` — `DEFAULT 'foo'` vs `DEFAULT 'foo'::text` for a text column.
- `0006-pk-inline-vs-table-constraint` — `id integer PRIMARY KEY` vs `id integer, PRIMARY KEY (id)`.
- `0007-not-null-via-pk` — implicit NOT NULL on PK column.

`different_pairs/`:
- `0001-varchar-len` — `varchar(50)` vs `varchar(100)` → `Varchar.len 50 vs 100`.
- `0002-text-vs-varchar` — `text` vs `varchar` (no length) — explicitly different per spec.
- `0003-on-delete` — `ON DELETE CASCADE` vs `ON DELETE NO ACTION`.
- `0004-unique-not-distinct` — `UNIQUE NULLS NOT DISTINCT` vs default.
- `0005-pk-column-order` — `PK (a, b)` vs `PK (b, a)`.

`parse_errors/`:
- `0001-create-view` — produces `UnsupportedObjectKind`, mentions "v0.1".
- `0002-create-function` — same.
- `0003-alter-add-column` — `Structural`, mentions declarative.
- `0004-unqualified-name-no-directive` — `UnqualifiedName`.
- `0005-duplicate-table` — two CREATE TABLE for the same qname → `DuplicateObject`.

Add `walkdir` to dev-dependencies for the harness.

Commit: `test(core): tier-2 fixture corpus harness with seed fixtures for parser equivalence/diff/error cases`

---

### Task 2.14: Phase 2 self-review

- [ ] Re-run gauntlet (`build / test / clippy / fmt`).
- [ ] Confirm `parse_directory` integration test passes against a hand-built fixture project.
- [ ] Confirm all seed fixtures pass.
- [ ] Confirm `cargo test -p pgevolve-core` runs the full corpus.
- [ ] Confirm CI is green on `main`.

Phase 2 complete.
