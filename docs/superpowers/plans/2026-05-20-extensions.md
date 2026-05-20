# v0.2 sub-spec #3: Extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** First-class management of Postgres extensions: parse `CREATE EXTENSION` from source SQL, read `pg_extension` from the catalog, diff with version-update + cascade-replace semantics, and emit `CREATE / ALTER ... UPDATE / DROP ... CASCADE` planner steps. Objects owned by extensions (deptype='e' in pg_depend) are filtered out of every other catalog query so they never appear as drift.

**Architecture:** New `ir/extension.rs` + `Catalog::extensions: Vec<Extension>` flat collection. Source builder, catalog reader, differ, planner, and two new lint rules follow the established v0.2 patterns (mirrors what types/functions/views did). Five new `StepKind` variants. One new `NodeId` variant. One new arm in the `emit_change` dispatcher. ~12 conformance fixtures.

**Tech Stack:** No new dependencies. Pattern: same shape as v0.2 sub-spec #2 (types) — a single non-body-bearing object kind read from a single `pg_catalog` table.

**Reference design:** `docs/superpowers/specs/2026-05-20-extensions-design.md`.

---

## Deviations from the spec

The spec mentions `lint/rules/` for the new lint rules. The actual codebase has no such directory — all lints live in `crates/pgevolve-core/src/lint/universal.rs`. This plan follows the established pattern and adds both rules to `universal.rs`.

---

## File structure

**Created:**
- `crates/pgevolve-core/src/ir/extension.rs` — `Extension` struct + tests.
- `crates/pgevolve-core/src/parse/builder/create_extension_stmt.rs` — source parser.
- `crates/pgevolve-core/src/catalog/queries/extensions.rs` — SQL for `pg_extension` query.
- `crates/pgevolve-core/src/diff/extensions.rs` — `ExtensionChange` differ.
- `crates/pgevolve-core/src/plan/rewrite/extensions.rs` — SQL string emission helpers.
- `crates/pgevolve-core/src/plan/rewrite/emit/extension.rs` — dispatcher.
- ~12 conformance fixtures under `crates/pgevolve-conformance/tests/cases/objects/extensions/` and `scenarios/`.

**Modified:**
- `crates/pgevolve-core/src/ir/mod.rs` — `pub mod extension;`.
- `crates/pgevolve-core/src/ir/catalog.rs` — `pub extensions: Vec<Extension>` field.
- `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs` — sort + dedupe pass for extensions.
- `crates/pgevolve-core/src/parse/builder/mod.rs` — `pub mod create_extension_stmt;`.
- `crates/pgevolve-core/src/parse/mod.rs` — dispatch `CreateExtensionStmt` to the new builder; reject `AlterExtensionStmt` / `DropExtensionStmt`.
- `crates/pgevolve-core/src/catalog/mod.rs` — `CatalogQuery::Extensions` variant; wire into `read_catalog`.
- `crates/pgevolve-core/src/catalog/queries/mod.rs` — register the new query.
- `crates/pgevolve-core/src/catalog/queries/shared.rs` (and `pg14.rs`) — add the `NOT EXISTS pg_depend deptype='e'` filter to existing object queries.
- `crates/pgevolve-core/src/catalog/queries/{functions,types,views}.rs` — same filter.
- `crates/pgevolve-core/src/catalog/assemble.rs` — assemble `Extension` rows.
- `crates/pgevolve-core/src/diff/change.rs` — `Change::Extension(ExtensionChange)` variant + `ExtensionChange` enum.
- `crates/pgevolve-core/src/diff/mod.rs` — call `extensions::diff_extensions` in `diff()`; re-export `ExtensionChange`.
- `crates/pgevolve-core/src/plan/edges.rs` — `NodeId::Extension(Identifier)` + `Extension → Schema` edges.
- `crates/pgevolve-core/src/plan/raw_step.rs` — 4 new `StepKind` variants.
- `crates/pgevolve-core/src/plan/ordering.rs` — bucket placement for extensions.
- `crates/pgevolve-core/src/plan/rewrite/mod.rs` — new `Change::Extension(ec)` arm in `emit_change`.
- `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs` — `pub mod extension;` declaration.
- `crates/pgevolve-core/src/lint/universal.rs` — two new rules.

---

## Task 1: Extension IR

**Files:**
- Create: `crates/pgevolve-core/src/ir/extension.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs`
- Modify: `crates/pgevolve-core/src/ir/catalog.rs`
- Modify: `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs`

- [ ] **Step 1: Create the Extension struct**

Create `crates/pgevolve-core/src/ir/extension.rs`:

```rust
//! `Extension` — a Postgres extension declared via `CREATE EXTENSION`.
//!
//! Source IR can carry `schema = None` and `version = None` to mean
//! "any" — the differ treats source-None as "don't care". Catalog IR
//! always populates both fields.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;

/// A Postgres extension.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, DiffMacro)]
pub struct Extension {
    /// Extension name (e.g. `pgcrypto`, `pg_trgm`).
    pub name: Identifier,
    /// Target schema. `None` = "use extension's default schema"
    /// (matches omitting `WITH SCHEMA` in source SQL).
    #[diff(via_debug)]
    pub schema: Option<Identifier>,
    /// Pinned version. `None` = "any installed version is fine"
    /// (matches omitting `VERSION` in source SQL).
    #[diff(via_debug)]
    pub version: Option<String>,
    /// Optional `COMMENT ON EXTENSION` text.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn ext(name: &str) -> Extension {
        Extension {
            name: id(name),
            schema: None,
            version: None,
            comment: None,
        }
    }

    #[test]
    fn identical_extensions_diff_empty() {
        let a = ext("pgcrypto");
        let b = ext("pgcrypto");
        assert!(a.canonical_eq(&b));
    }

    #[test]
    fn different_versions_diff_reports_version() {
        let a = ext("pgcrypto");
        let mut b = ext("pgcrypto");
        b.version = Some("1.4".into());
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "version"));
    }

    #[test]
    fn different_schemas_diff_reports_schema() {
        let a = ext("pgcrypto");
        let mut b = ext("pgcrypto");
        b.schema = Some(id("app"));
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "schema"));
    }
}
```

- [ ] **Step 2: Register the module**

Edit `crates/pgevolve-core/src/ir/mod.rs`. Add `pub mod extension;` in alphabetical order (between `default_expr` and `function` looks right; verify the existing layout and insert appropriately).

- [ ] **Step 3: Add the field to `Catalog`**

Edit `crates/pgevolve-core/src/ir/catalog.rs`. Add `use crate::ir::extension::Extension;` to the imports at the top. Then add a field to the `Catalog` struct right after `schemas`:

```rust
pub extensions: Vec<Extension>,
```

Update `Catalog::empty()` (or its `Default` impl, whichever exists) to initialize `extensions: Vec::new()`.

- [ ] **Step 4: Add extensions to the sort_and_dedupe canon pass**

Edit `crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs`. Find the existing per-collection blocks (each is a `sort_by(|a, b| ...)` + `first_duplicate(...)` pair). Add an extensions block AFTER the `schemas` block and BEFORE the `tables` block (so extensions come second in the canonical order, after schemas):

```rust
    cat.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(dupe) = first_duplicate(cat.extensions.iter().map(|e| e.name.as_str())) {
        return Err(IrError::InvalidIdentifier(format!(
            "duplicate extension: {dupe}"
        )));
    }
```

- [ ] **Step 5: Run tests + clippy**

```
cargo test -p pgevolve-core --lib ir::extension
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: ir::extension tests pass (3 new tests). Full suite still green; test count rises by 3.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/ir/extension.rs crates/pgevolve-core/src/ir/mod.rs crates/pgevolve-core/src/ir/catalog.rs crates/pgevolve-core/src/ir/canon/sort_and_dedupe.rs
git commit -m "$(cat <<'EOF'
feat(ir): add Extension IR for v0.2 sub-spec #3

Flat Extension struct with name + optional schema + optional version
+ optional comment. Catalog::extensions: Vec<Extension>. canon
sort_and_dedupe rejects duplicate extension names.

Source IR carries None for schema/version to mean "any"; catalog IR
always populates both. Differ asymmetry rules land in T5.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Source parser for `CREATE EXTENSION`

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/create_extension_stmt.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs`
- Modify: `crates/pgevolve-core/src/parse/mod.rs`

- [ ] **Step 1: Write the builder**

Create `crates/pgevolve-core/src/parse/builder/create_extension_stmt.rs`:

```rust
//! Source parser for `CREATE EXTENSION` statements.

use pg_query::protobuf::{CreateExtensionStmt, DefElem};

use crate::identifier::Identifier;
use crate::ir::extension::Extension;
use crate::parse::error::{ParseError, SourceLocation};

/// Build an [`Extension`] from a parsed `CreateExtensionStmt` AST node.
///
/// Accepts:
/// - `CREATE EXTENSION [IF NOT EXISTS] name`
/// - `[WITH] SCHEMA s`
/// - `[WITH] VERSION 'v'`  (string literal or unquoted identifier)
///
/// Rejects (with `ParseError::UnsupportedClause`):
/// - `CASCADE`
/// - `FROM old_version`
/// - `NO RESTART` (PG 17+)
pub fn build_extension(
    stmt: &CreateExtensionStmt,
    location: &SourceLocation,
) -> Result<Extension, ParseError> {
    let name = Identifier::from_unquoted(&stmt.extname).map_err(|e| {
        ParseError::InvalidIdentifier {
            location: location.clone(),
            message: e.to_string(),
        }
    })?;

    let mut schema: Option<Identifier> = None;
    let mut version: Option<String> = None;

    for option_node in &stmt.options {
        let de = match option_node.node.as_ref() {
            Some(pg_query::NodeEnum::DefElem(de)) => de,
            _ => continue,
        };
        match de.defname.as_str() {
            "schema" => {
                schema = Some(string_value(de, "schema", location)?.parse_as_identifier(location)?);
            }
            "new_version" => {
                version = Some(string_value(de, "new_version", location)?.into_inner());
            }
            "cascade" => {
                return Err(ParseError::UnsupportedClause {
                    location: location.clone(),
                    message: format!(
                        "{}: CREATE EXTENSION ... CASCADE is not supported. Declare every extension explicitly.",
                        name
                    ),
                });
            }
            "old_version" => {
                return Err(ParseError::UnsupportedClause {
                    location: location.clone(),
                    message: format!(
                        "{}: CREATE EXTENSION ... FROM <old_version> is not supported.",
                        name
                    ),
                });
            }
            other => {
                return Err(ParseError::UnsupportedClause {
                    location: location.clone(),
                    message: format!(
                        "{}: CREATE EXTENSION option '{}' is not supported.",
                        name, other,
                    ),
                });
            }
        }
    }

    Ok(Extension {
        name,
        schema,
        version,
        comment: None,
    })
}

// --- helpers ---

struct StringValue(String);

impl StringValue {
    fn into_inner(self) -> String {
        self.0
    }

    fn parse_as_identifier(self, location: &SourceLocation) -> Result<Identifier, ParseError> {
        Identifier::from_unquoted(&self.0).map_err(|e| ParseError::InvalidIdentifier {
            location: location.clone(),
            message: e.to_string(),
        })
    }
}

fn string_value(
    de: &DefElem,
    field: &str,
    location: &SourceLocation,
) -> Result<StringValue, ParseError> {
    let arg = de.arg.as_ref().and_then(|n| n.node.as_ref()).ok_or_else(|| {
        ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE EXTENSION {field}: missing value"),
        }
    })?;
    match arg {
        pg_query::NodeEnum::String(s) => Ok(StringValue(s.sval.clone())),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE EXTENSION {field}: expected string value"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_stmt(sql: &str) -> CreateExtensionStmt {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let stmt_node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .expect("at least one statement")
            .stmt
            .expect("stmt set")
            .node
            .expect("node set");
        match stmt_node {
            pg_query::NodeEnum::CreateExtensionStmt(s) => s,
            other => panic!("not a CreateExtensionStmt: {other:?}"),
        }
    }

    fn loc() -> SourceLocation {
        SourceLocation::new(std::path::PathBuf::from("<test>"), 1, 1)
    }

    #[test]
    fn parses_bare_create_extension() {
        let stmt = parse_stmt("CREATE EXTENSION pgcrypto;");
        let ext = build_extension(&stmt, &loc()).expect("build_extension");
        assert_eq!(ext.name.as_str(), "pgcrypto");
        assert!(ext.schema.is_none());
        assert!(ext.version.is_none());
    }

    #[test]
    fn parses_with_schema() {
        let stmt = parse_stmt("CREATE EXTENSION pg_trgm WITH SCHEMA app;");
        let ext = build_extension(&stmt, &loc()).expect("build_extension");
        assert_eq!(ext.schema.as_ref().map(|i| i.as_str()), Some("app"));
    }

    #[test]
    fn parses_with_version() {
        let stmt = parse_stmt("CREATE EXTENSION pgcrypto VERSION '1.3';");
        let ext = build_extension(&stmt, &loc()).expect("build_extension");
        assert_eq!(ext.version.as_deref(), Some("1.3"));
    }

    #[test]
    fn parses_if_not_exists_with_schema_and_version() {
        let stmt = parse_stmt(
            "CREATE EXTENSION IF NOT EXISTS postgis WITH SCHEMA gis VERSION '3.4';",
        );
        let ext = build_extension(&stmt, &loc()).expect("build_extension");
        assert_eq!(ext.name.as_str(), "postgis");
        assert_eq!(ext.schema.as_ref().map(|i| i.as_str()), Some("gis"));
        assert_eq!(ext.version.as_deref(), Some("3.4"));
    }

    #[test]
    fn rejects_cascade() {
        let stmt = parse_stmt("CREATE EXTENSION postgis CASCADE;");
        let err = build_extension(&stmt, &loc()).expect_err("CASCADE must reject");
        assert!(err.to_string().contains("CASCADE"), "got {err}");
    }

    #[test]
    fn rejects_from_clause() {
        let stmt = parse_stmt("CREATE EXTENSION pgcrypto FROM '1.0';");
        let err = build_extension(&stmt, &loc()).expect_err("FROM must reject");
        assert!(err.to_string().contains("FROM"), "got {err}");
    }
}
```

If `ParseError::InvalidIdentifier` / `Structural` / `UnsupportedClause` variant names don't exist (verify by grepping `crates/pgevolve-core/src/parse/error.rs`), adapt to the actual variant names. Use the same error shapes already used by `create_function_stmt.rs`.

- [ ] **Step 2: Register the builder module**

Edit `crates/pgevolve-core/src/parse/builder/mod.rs`. Add `pub mod create_extension_stmt;` in alphabetical order.

- [ ] **Step 3: Dispatch from the statement classifier**

Edit `crates/pgevolve-core/src/parse/mod.rs`. Find the existing match arm that dispatches `CreateFunctionStmt` (or similar — `grep -n "CreateFunctionStmt\|NodeEnum::Create" crates/pgevolve-core/src/parse/mod.rs`). Add an arm for `NodeEnum::CreateExtensionStmt(s)` that calls `builder::create_extension_stmt::build_extension(s, &location)?` and pushes the result onto `catalog.extensions`.

Additionally, find where `AlterExtensionStmt` and `DropStmt` are handled. For `AlterExtensionStmt`, add a rejection arm:

```rust
pg_query::NodeEnum::AlterExtensionStmt(_) => {
    return Err(ParseError::UnsupportedStatement {
        location,
        message: "ALTER EXTENSION is not supported in source files — \
                  declare the desired state via CREATE EXTENSION".into(),
    });
}
```

For `DropStmt`, the existing dispatch may already handle the various DROP variants; check whether `DropStmt::removeType == OBJECT_EXTENSION` is currently a passthrough or error. If passthrough, add an error case mirroring the AlterExtension rejection. If unclear, leave it — the parser already rejects unknown DROP shapes via its whitelist.

- [ ] **Step 4: Run tests + clippy**

```
cargo test -p pgevolve-core --lib parse::builder::create_extension_stmt
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: 6 new builder tests pass. Full suite green.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/parse/builder/create_extension_stmt.rs crates/pgevolve-core/src/parse/builder/mod.rs crates/pgevolve-core/src/parse/mod.rs
git commit -m "$(cat <<'EOF'
feat(parse): CREATE EXTENSION source builder

Parses CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s]
[VERSION 'v'] into the Extension IR. Rejects CASCADE, FROM
old_version, and unknown options with UnsupportedClause errors.
ALTER EXTENSION in source files is rejected.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Catalog query for `pg_extension`

**Files:**
- Create: `crates/pgevolve-core/src/catalog/queries/extensions.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/mod.rs`
- Modify: `crates/pgevolve-core/src/catalog/mod.rs`
- Modify: `crates/pgevolve-core/src/catalog/assemble.rs`

- [ ] **Step 1: Write the SQL query**

Create `crates/pgevolve-core/src/catalog/queries/extensions.rs`:

```rust
//! Catalog query for `pg_extension` — one row per installed extension.

/// Reads name + schema + version + optional comment for every installed
/// extension. Not filtered by managed schemas — extensions are
/// cluster-global in pgevolve's worldview (we list them all and let the
/// differ decide which to keep).
pub const SELECT_EXTENSIONS: &str = r"
SELECT
    e.extname::text         AS name,
    n.nspname::text         AS schema,
    e.extversion::text      AS version,
    d.description           AS comment
FROM pg_catalog.pg_extension e
JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
LEFT JOIN pg_catalog.pg_description d
    ON d.objoid = e.oid
   AND d.classoid = 'pg_catalog.pg_extension'::regclass
ORDER BY e.extname
";
```

The query takes no parameters (extensions aren't scoped to managed schemas — pgevolve sees the whole list and diffs against the source-declared set).

- [ ] **Step 2: Register the query variant**

Edit `crates/pgevolve-core/src/catalog/mod.rs`. Add `Extensions` to the `CatalogQuery` enum (after `Functions`):

```rust
    /// `pg_extension` rows for installed extensions.
    Extensions,
```

Edit `crates/pgevolve-core/src/catalog/queries/mod.rs`. Register the new mapping in `query_for`:

```rust
        (_, CatalogQuery::Extensions) => extensions::SELECT_EXTENSIONS,
```

And add `pub mod extensions;` at the top of `queries/mod.rs` in alphabetical order.

- [ ] **Step 3: Wire `read_catalog` to fetch extensions**

Edit `crates/pgevolve-core/src/catalog/mod.rs`. In `read_catalog` find the block that fetches each query (looks like a series of `querier.fetch(CatalogQuery::*, ...)` calls). Add:

```rust
    let extensions_rows = querier.fetch(CatalogQuery::Extensions, &managed)?;
```

Add `extensions: extensions_rows` to the `RawRows` struct construction.

- [ ] **Step 4: Add `extensions` field to `RawRows` and `build_extensions`**

Edit `crates/pgevolve-core/src/catalog/assemble.rs`. Find the `pub struct RawRows` definition. Add:

```rust
    pub extensions: Vec<Row>,
```

In `assemble()`, after the existing object builds (functions/types/etc.), add:

```rust
    catalog.extensions = build_extensions(&extensions)?;
```

Then write the helper at the bottom of the file (modeled on `build_sequence` / `build_user_types`):

```rust
fn build_extensions(rows: &[Row]) -> Result<Vec<Extension>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Extensions;
        let name = Identifier::from_unquoted(&r.get_text(q, "name")?)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        let schema_str = r.get_text(q, "schema")?;
        let schema = Identifier::from_unquoted(&schema_str)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        let version = r.get_text(q, "version")?;
        let comment = r.get_opt_text(q, "comment")?;
        out.push(Extension {
            name,
            schema: Some(schema),
            version: Some(version),
            comment,
        });
    }
    Ok(out)
}
```

Add `use crate::ir::extension::Extension;` near the top of `assemble.rs`.

- [ ] **Step 5: Run tests + clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green. No new tests yet — catalog round-trip tested via conformance fixtures in T13.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/catalog/queries/extensions.rs crates/pgevolve-core/src/catalog/queries/mod.rs crates/pgevolve-core/src/catalog/mod.rs crates/pgevolve-core/src/catalog/assemble.rs
git commit -m "$(cat <<'EOF'
feat(catalog): read pg_extension into Extension IR

New CatalogQuery::Extensions variant + SELECT_EXTENSIONS SQL +
build_extensions assembler. Catalog-side extensions always carry
concrete schema and version; source can leave them None.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Filter extension-owned objects from other catalog queries

This is the load-bearing change for Decision 20: objects installed by extensions (operators, functions, types, etc.) appear in the catalog with `pg_depend.deptype = 'e'`. Without filtering, they'd diff as drift on every plan.

**Files:**
- Modify: `crates/pgevolve-core/src/catalog/queries/shared.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/pg14.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/functions.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/types.rs`
- Modify: `crates/pgevolve-core/src/catalog/queries/views.rs`

- [ ] **Step 1: Identify queries that need the filter**

The filter excludes pg_class / pg_proc / pg_type rows owned by extensions. The clause:

```sql
AND NOT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_depend dep
    WHERE dep.classid = '<pg_catalog_relation>'::regclass
      AND dep.objid = <oid_column>
      AND dep.deptype = 'e'
)
```

Apply to every query that reads catalog rows that could be extension-owned:

| Query (constant name)              | Catalog              | classid                              | oid column |
|------------------------------------|----------------------|--------------------------------------|------------|
| `TABLES_QUERY` (shared.rs)         | pg_class             | `'pg_catalog.pg_class'::regclass`    | `c.oid`    |
| `COLUMNS_QUERY` (shared.rs)        | (joined via pg_class) | `'pg_catalog.pg_class'::regclass`   | `a.attrelid` |
| `CONSTRAINTS_QUERY` (shared.rs)    | (via pg_class table) | `'pg_catalog.pg_class'::regclass`    | the table's oid |
| `INDEXES_QUERY` (shared.rs + pg14.rs) | pg_class           | `'pg_catalog.pg_class'::regclass`    | `c.oid` (index) |
| `SEQUENCES_QUERY` (shared.rs)      | pg_class             | `'pg_catalog.pg_class'::regclass`    | `c.oid`    |
| `SELECT_VIEWS_AND_MVS` (views.rs)  | pg_class             | `'pg_catalog.pg_class'::regclass`    | `c.oid`    |
| `SELECT_USER_TYPES` (types.rs)     | pg_type              | `'pg_catalog.pg_type'::regclass`     | `t.oid`    |
| `SELECT_FUNCTIONS` (functions.rs)  | pg_proc              | `'pg_catalog.pg_proc'::regclass`     | `p.oid`    |

`SCHEMAS_QUERY` is NOT filtered — schemas can be created by both users and extensions, and pgevolve manages whichever schemas the user lists in `[managed].schemas`.

For columns / constraints / indexes / dependencies that join via `pg_class`, the filter on the parent `pg_class` row is sufficient: if the parent table is extension-owned, its columns/constraints/indexes are excluded by virtue of the parent JOIN not finding the row.

- [ ] **Step 2: Add the filter to each query**

For each of TABLES_QUERY, INDEXES_QUERY (in both shared.rs and pg14.rs), SEQUENCES_QUERY, SELECT_VIEWS_AND_MVS, SELECT_USER_TYPES, SELECT_FUNCTIONS:

Locate the existing `WHERE` clause and append (or insert before `ORDER BY` if no other WHERE conditions exist):

```sql
  AND NOT EXISTS (
      SELECT 1
      FROM pg_catalog.pg_depend dep
      WHERE dep.classid = '<CATALOG>'::regclass
        AND dep.objid = <OID_COL>
        AND dep.deptype = 'e'
  )
```

Substitute `<CATALOG>` and `<OID_COL>` per the table above.

Example for `TABLES_QUERY` (`shared.rs`): add the clause to the existing WHERE.

For COLUMNS_QUERY, CONSTRAINTS_QUERY, and the dependency query: the filter is NOT needed on the columns/constraints/dependencies themselves — those are only emitted for tables that already passed the TABLES_QUERY filter. The columns/constraints assembler only consumes rows for tables it built. Double-check this by reading `assemble.rs`'s `build_tables` / `apply_constraints` flow — if they iterate over all rows regardless of table presence, then COLUMNS_QUERY and CONSTRAINTS_QUERY also need the parent-table filter.

- [ ] **Step 3: Run conformance suite to verify no regression**

```
cargo test -p pgevolve-conformance --test run
```
Expected: PASS. Existing fixtures don't install extensions, so the new filter is a no-op for them. The new filter being a no-op is the regression-safety check.

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/catalog/queries/
git commit -m "$(cat <<'EOF'
feat(catalog): filter extension-owned objects from catalog queries

Every object query (tables, indexes, sequences, views/MVs, types,
functions) gains a NOT EXISTS (pg_depend deptype='e') clause that
excludes rows owned by an extension. Per Decision 20, pgevolve does
not manage objects installed by extensions; this stops them from
appearing as drift on every plan.

Existing fixtures unaffected (none install extensions). The new
filter is exercised by the scenarios/extension-owned-objects-ignored
fixture added in T13.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Differ for extensions

**Files:**
- Create: `crates/pgevolve-core/src/diff/extensions.rs`
- Modify: `crates/pgevolve-core/src/diff/change.rs`
- Modify: `crates/pgevolve-core/src/diff/mod.rs`

- [ ] **Step 1: Add the `ExtensionChange` enum to `change.rs`**

Edit `crates/pgevolve-core/src/diff/change.rs`. Near the other family change enums (`FunctionChange`, `ProcedureChange`, `UserTypeChange`, etc., typically at the bottom of the file), add:

```rust
/// Change to one extension. Pair-by-name semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ExtensionChange {
    /// Install a new extension.
    Create(Extension),
    /// Drop an extension by name. Destructive — emits `DROP EXTENSION ... CASCADE`.
    Drop(Identifier),
    /// Bump extension version: `ALTER EXTENSION ... UPDATE TO 'v'`.
    AlterUpdate {
        name: Identifier,
        to_version: String,
    },
    /// Schema-changing replace (destructive, DROP + CREATE).
    ReplaceWithCascade(Extension),
    /// Change the `COMMENT ON EXTENSION` text.
    CommentOn {
        name: Identifier,
        comment: Option<String>,
    },
}
```

Add `use crate::ir::extension::Extension;` to the imports if not already present.

Then add the variant to the `Change` enum (right after `Procedure(ProcedureChange)`):

```rust
    /// An extension change.
    Extension(ExtensionChange),
```

- [ ] **Step 2: Write the differ**

Create `crates/pgevolve-core/src/diff/extensions.rs`:

```rust
//! Differ for `Catalog::extensions`.
//!
//! Pair-by-name. Source can leave `schema` or `version` as `None` to mean
//! "don't care"; the differ never emits a change when the source side is
//! `None` for the relevant field.

use crate::diff::change::{Change, ExtensionChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::extension::Extension;

pub fn diff_extensions(
    target: &[Extension],
    source: &[Extension],
    out: &mut ChangeSet,
) {
    use std::collections::BTreeMap;
    let target_by_name: BTreeMap<_, _> = target.iter().map(|e| (e.name.clone(), e)).collect();
    let source_by_name: BTreeMap<_, _> = source.iter().map(|e| (e.name.clone(), e)).collect();

    // Drops: in target but not source.
    for (name, _t) in &target_by_name {
        if !source_by_name.contains_key(name) {
            out.push(
                Change::Extension(ExtensionChange::Drop(name.clone())),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!(
                        "DROP EXTENSION {name} CASCADE removes every object owned by the extension."
                    ),
                },
            );
        }
    }

    // Creates and alters.
    for (name, s) in &source_by_name {
        match target_by_name.get(name) {
            None => out.push(
                Change::Extension(ExtensionChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            Some(t) => {
                // Schema mismatch (source-None matches anything).
                if let Some(source_schema) = &s.schema
                    && t.schema.as_ref() != Some(source_schema)
                {
                    out.push(
                        Change::Extension(ExtensionChange::ReplaceWithCascade((*s).clone())),
                        Destructiveness::RequiresApprovalAndDataLossWarning {
                            reason: format!(
                                "Changing the schema of extension {name} requires DROP CASCADE; \
                                 every object owned by the extension is removed and re-created."
                            ),
                        },
                    );
                    continue;
                }
                // Version mismatch (source-None matches anything).
                if let Some(source_version) = &s.version
                    && t.version.as_ref() != Some(source_version)
                {
                    out.push(
                        Change::Extension(ExtensionChange::AlterUpdate {
                            name: name.clone(),
                            to_version: source_version.clone(),
                        }),
                        Destructiveness::Safe,
                    );
                }
                // Comment mismatch.
                if t.comment != s.comment {
                    out.push(
                        Change::Extension(ExtensionChange::CommentOn {
                            name: name.clone(),
                            comment: s.comment.clone(),
                        }),
                        Destructiveness::Safe,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn ext(name: &str) -> Extension {
        Extension {
            name: id(name),
            schema: None,
            version: None,
            comment: None,
        }
    }

    #[test]
    fn create_when_only_in_source() {
        let mut cs = ChangeSet::new();
        diff_extensions(&[], &[ext("pgcrypto")], &mut cs);
        assert!(matches!(
            cs.iter().next().map(|e| &e.change),
            Some(Change::Extension(ExtensionChange::Create(_)))
        ));
    }

    #[test]
    fn drop_when_only_in_target() {
        let mut cs = ChangeSet::new();
        diff_extensions(&[ext("pgcrypto")], &[], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Extension(ExtensionChange::Drop(_))
        ));
        assert!(matches!(
            &first.destructiveness,
            Destructiveness::RequiresApprovalAndDataLossWarning { .. }
        ));
    }

    #[test]
    fn version_unpinned_in_source_matches_any_catalog_version() {
        let mut t = ext("pgcrypto");
        t.version = Some("1.3".into());
        let s = ext("pgcrypto"); // source unpinned
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        assert!(cs.iter().next().is_none(), "unpinned source must not diff");
    }

    #[test]
    fn version_pinned_in_source_triggers_alter_update() {
        let mut t = ext("pgcrypto");
        t.version = Some("1.3".into());
        let mut s = ext("pgcrypto");
        s.version = Some("1.4".into());
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Extension(ExtensionChange::AlterUpdate { to_version, .. })
                if to_version == "1.4"
        ));
    }

    #[test]
    fn schema_change_triggers_replace_with_cascade() {
        let mut t = ext("pgcrypto");
        t.schema = Some(id("public"));
        let mut s = ext("pgcrypto");
        s.schema = Some(id("app"));
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Extension(ExtensionChange::ReplaceWithCascade(_))
        ));
        assert!(matches!(
            &first.destructiveness,
            Destructiveness::RequiresApprovalAndDataLossWarning { .. }
        ));
    }

    #[test]
    fn schema_unpinned_in_source_skips_schema_diff() {
        let mut t = ext("pgcrypto");
        t.schema = Some(id("public"));
        let s = ext("pgcrypto"); // unpinned schema
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        assert!(cs.iter().next().is_none());
    }
}
```

If `ChangeSet::iter()` or `ChangeSet::push(change, destructiveness)` are spelled differently in the existing codebase, adjust. Refer to `crates/pgevolve-core/src/diff/changeset.rs`.

- [ ] **Step 3: Wire into `diff()`**

Edit `crates/pgevolve-core/src/diff/mod.rs`. Add the new module declaration near other family modules:

```rust
pub mod extensions;
```

In the `diff()` function, after `schemas::diff_schemas(target, source, &mut out);` add:

```rust
    extensions::diff_extensions(&target.extensions, &source.extensions, &mut out);
```

Extensions diff right after schemas because they're created right after schemas in the planner's order.

Also re-export the new type from the `pub use change` block:

```rust
pub use change::{
    Change, ChangeEntry, ExtensionChange, FunctionChange, MvChange, ProcedureChange,
    UserTypeChange, ViewChange,
};
```

- [ ] **Step 4: Run tests + clippy**

```
cargo test -p pgevolve-core --lib diff::extensions
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: 6 new differ tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/diff/extensions.rs crates/pgevolve-core/src/diff/change.rs crates/pgevolve-core/src/diff/mod.rs
git commit -m "$(cat <<'EOF'
feat(diff): ExtensionChange differ with source-None symmetry

ExtensionChange variants: Create, Drop, AlterUpdate, ReplaceWithCascade,
CommentOn. Source-None for schema/version means \"any catalog value\"
so unpinned source declarations don't diff against any installed
version.

Drop and ReplaceWithCascade are Destructive::Loss with the cascade
context in destructive_reason.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: NodeId variant and dep graph edges

**Files:**
- Modify: `crates/pgevolve-core/src/plan/edges.rs`

- [ ] **Step 1: Add `NodeId::Extension`**

Edit `crates/pgevolve-core/src/plan/edges.rs`. In the `NodeId` enum (around line 34), add a new variant after `Schema`:

```rust
    /// An installed extension.
    Extension(Identifier),
```

- [ ] **Step 2: Register and edge-wire extension nodes in `build_create_graph`**

In the same file, find `pub fn build_create_graph`. After the existing schema-node registration block, add:

```rust
    for e in &catalog.extensions {
        g.add_node(NodeId::Extension(e.name.clone()));
        if let Some(schema) = &e.schema {
            g.add_edge(NodeId::Extension(e.name.clone()), NodeId::Schema(schema.clone()));
        }
    }
```

The edge `Extension → Schema` means "extension depends on schema" — `topological_sort` produces dependencies first, so schemas come before extensions in the create order, and extensions come before schemas in the drop order (reverse topo).

No reverse edges in v0.2: managed objects do not declare a dep on any extension. The implicit guarantee comes from the bucket ordering — extensions are in the same "creates" bucket as schemas, dispatched together.

- [ ] **Step 3: Run tests + clippy**

```
cargo test -p pgevolve-core --lib plan::edges
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/edges.rs
git commit -m "$(cat <<'EOF'
feat(plan): NodeId::Extension + Extension → Schema dep edges

Extensions register as graph nodes; when an extension declares
WITH SCHEMA s, an edge from Extension(name) → Schema(s) ensures
the schema is created first (and dropped last). No reverse edges
from managed objects to extensions in v0.2.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: StepKind variants + ordering bucket

**Files:**
- Modify: `crates/pgevolve-core/src/plan/raw_step.rs`
- Modify: `crates/pgevolve-core/src/plan/ordering.rs`

- [ ] **Step 1: Add four `StepKind` variants**

Edit `crates/pgevolve-core/src/plan/raw_step.rs`. After the existing v0.2 step kinds (after `CommentOnProcedure`), add:

```rust
    // --- v0.2 extension step kinds ---
    /// `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']`.
    CreateExtension,
    /// `DROP EXTENSION name CASCADE`. Destructive.
    DropExtension,
    /// `ALTER EXTENSION name UPDATE TO 'v'`.
    AlterExtensionUpdate,
    /// `COMMENT ON EXTENSION name IS '...'`.
    CommentOnExtension,
```

- [ ] **Step 2: Place extensions in the correct ordering bucket**

Edit `crates/pgevolve-core/src/plan/ordering.rs`. Find the `order()` function (or wherever `Change` variants are routed to creates / modifies / drops buckets). Add cases for `Change::Extension`:

- `ExtensionChange::Create` → creates bucket
- `ExtensionChange::Drop` → drops bucket
- `ExtensionChange::AlterUpdate`, `CommentOn` → modifies bucket
- `ExtensionChange::ReplaceWithCascade` → emits a drop + create pair into the corresponding buckets; the planner already handles `ReplaceWithCascade` for other families — model on the existing `UserTypeChange::ReplaceWithCascade` or `FunctionChange::ReplaceWithCascade` handling.

The exact case body depends on how the existing ordering routes Change variants. Read the surrounding code carefully and follow the pattern used for `Change::UserType` and `Change::Function`.

- [ ] **Step 3: Run tests + clippy**

```
cargo test -p pgevolve-core --lib plan::ordering
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/raw_step.rs crates/pgevolve-core/src/plan/ordering.rs
git commit -m "$(cat <<'EOF'
feat(plan): 4 new StepKind variants + extension bucket ordering

CreateExtension, DropExtension, AlterExtensionUpdate, CommentOnExtension.
Extensions placed in creates/modifies/drops buckets per change variant;
ReplaceWithCascade emits drop + create like the existing user-type
and function paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: SQL string emission helpers

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/extensions.rs`

- [ ] **Step 1: Write the SQL helpers**

Create `crates/pgevolve-core/src/plan/rewrite/extensions.rs`:

```rust
//! SQL emission for extension planner steps.
//!
//! Each helper produces a single canonical SQL statement string ending
//! with `;`, deterministic for byte-stable plan output.

use crate::identifier::Identifier;
use crate::ir::extension::Extension;

/// `CREATE EXTENSION IF NOT EXISTS "name" [WITH SCHEMA "schema"] [VERSION 'v'];`
pub(crate) fn create_extension(e: &Extension) -> String {
    let mut sql = format!("CREATE EXTENSION IF NOT EXISTS {}", e.name.render_sql());
    if let Some(schema) = &e.schema {
        sql.push_str(&format!(" WITH SCHEMA {}", schema.render_sql()));
    }
    if let Some(version) = &e.version {
        sql.push_str(&format!(" VERSION '{}'", escape_sql_string(version)));
    }
    sql.push(';');
    sql
}

/// `DROP EXTENSION "name" CASCADE;`
pub(crate) fn drop_extension(name: &Identifier) -> String {
    format!("DROP EXTENSION {} CASCADE;", name.render_sql())
}

/// `ALTER EXTENSION "name" UPDATE TO 'v';`
pub(crate) fn alter_extension_update(name: &Identifier, to_version: &str) -> String {
    format!(
        "ALTER EXTENSION {} UPDATE TO '{}';",
        name.render_sql(),
        escape_sql_string(to_version),
    )
}

/// `COMMENT ON EXTENSION "name" IS '...';` or `IS NULL;`
pub(crate) fn comment_on_extension(name: &Identifier, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON EXTENSION {} IS '{}';",
            name.render_sql(),
            escape_sql_string(c),
        ),
        None => format!("COMMENT ON EXTENSION {} IS NULL;", name.render_sql()),
    }
}

fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn ext_with(name: &str, schema: Option<&str>, version: Option<&str>) -> Extension {
        Extension {
            name: id(name),
            schema: schema.map(id),
            version: version.map(str::to_string),
            comment: None,
        }
    }

    #[test]
    fn create_bare() {
        assert_eq!(
            create_extension(&ext_with("pgcrypto", None, None)),
            "CREATE EXTENSION IF NOT EXISTS pgcrypto;"
        );
    }

    #[test]
    fn create_with_schema_and_version() {
        assert_eq!(
            create_extension(&ext_with("pg_trgm", Some("app"), Some("1.6"))),
            "CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA app VERSION '1.6';"
        );
    }

    #[test]
    fn drop_renders_cascade() {
        assert_eq!(
            drop_extension(&id("pgcrypto")),
            "DROP EXTENSION pgcrypto CASCADE;"
        );
    }

    #[test]
    fn alter_update_to_version() {
        assert_eq!(
            alter_extension_update(&id("pgcrypto"), "1.4"),
            "ALTER EXTENSION pgcrypto UPDATE TO '1.4';"
        );
    }

    #[test]
    fn comment_set_and_clear() {
        assert_eq!(
            comment_on_extension(&id("pgcrypto"), Some("crypto helpers")),
            "COMMENT ON EXTENSION pgcrypto IS 'crypto helpers';"
        );
        assert_eq!(
            comment_on_extension(&id("pgcrypto"), None),
            "COMMENT ON EXTENSION pgcrypto IS NULL;"
        );
    }

    #[test]
    fn escape_single_quote() {
        assert_eq!(
            comment_on_extension(&id("pgcrypto"), Some("it's fine")),
            "COMMENT ON EXTENSION pgcrypto IS 'it''s fine';"
        );
    }
}
```

If `Identifier::render_sql()` doesn't exist, use whichever method other sibling SQL helpers use (check `sql.rs::create_schema` for the established pattern).

- [ ] **Step 2: Register the module**

Edit `crates/pgevolve-core/src/plan/rewrite/mod.rs`. Add `pub mod extensions;` in alphabetical order with the existing SQL helper modules (sibling to `sql`, `functions`, `views`, `types`).

- [ ] **Step 3: Run tests + clippy**

```
cargo test -p pgevolve-core --lib plan::rewrite::extensions
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: 6 new helper tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/extensions.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
feat(rewrite): SQL emission helpers for extension steps

Four helpers: create_extension, drop_extension (always CASCADE),
alter_extension_update, comment_on_extension. Sibling to the
existing sql.rs / functions.rs / views.rs / types.rs helpers.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Per-family dispatcher in `emit/extension.rs`

**Files:**
- Create: `crates/pgevolve-core/src/plan/rewrite/emit/extension.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs`
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs`

- [ ] **Step 1: Write the dispatcher**

Create `crates/pgevolve-core/src/plan/rewrite/emit/extension.rs`:

```rust
//! Dispatcher for `Change::Extension(ExtensionChange)`.

use crate::diff::change::ExtensionChange;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::extensions as sql;

pub fn emit(
    ec: ExtensionChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match ec {
        ExtensionChange::Create(e) => {
            let name = e.name.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateExtension,
                destructive,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::create_extension(&e),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(comment) = &e.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnExtension,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![extension_target(&name)],
                    sql: sql::comment_on_extension(&name, Some(comment)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        ExtensionChange::Drop(name) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropExtension,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::drop_extension(&name),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        ExtensionChange::AlterUpdate { name, to_version } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterExtensionUpdate,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::alter_extension_update(&name, &to_version),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        ExtensionChange::ReplaceWithCascade(e) => {
            let name = e.name.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropExtension,
                destructive,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::drop_extension(&name),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateExtension,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::create_extension(&e),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        ExtensionChange::CommentOn { name, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnExtension,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::comment_on_extension(&name, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

/// Extensions live outside schema scope; surface a synthetic
/// `<extension>.<name>` target so the plan format's `targets` field has
/// a value. Matches what `schema_target` does for schemas.
fn extension_target(name: &crate::identifier::Identifier) -> crate::identifier::QualifiedName {
    crate::identifier::QualifiedName::new(
        crate::identifier::Identifier::from_unquoted("<extension>").unwrap(),
        name.clone(),
    )
}
```

If `Identifier::from_unquoted("<extension>")` rejects the angle-brackets (because `<` is invalid), use a different convention — `pg_extension` (PG-style namespace) is acceptable. Confirm by reading `schema_target` in `mod.rs` and matching that pattern.

- [ ] **Step 2: Register the emit module**

Edit `crates/pgevolve-core/src/plan/rewrite/emit/mod.rs`. Add `pub mod extension;` in alphabetical order.

- [ ] **Step 3: Add the dispatcher arm in `emit_change`**

Edit `crates/pgevolve-core/src/plan/rewrite/mod.rs`. In the `emit_change` match block, add a new arm after the existing `Change::Procedure(pc) => ...` arm:

```rust
        Change::Extension(ec) => emit::extension::emit(ec, destructive, destructive_reason, out),
```

- [ ] **Step 4: Run tests + clippy**

```
cargo test -p pgevolve-core --lib
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/plan/rewrite/emit/extension.rs crates/pgevolve-core/src/plan/rewrite/emit/mod.rs crates/pgevolve-core/src/plan/rewrite/mod.rs
git commit -m "$(cat <<'EOF'
feat(rewrite): emit/extension.rs dispatcher

12th per-family dispatcher in the emit/ submodule. Routes
ExtensionChange variants to the SQL helper module. Create emits a
CreateExtension step plus an optional CommentOnExtension follow-up
when source declares a comment.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Lint rule — `extension-version-unpinned`

**Files:**
- Modify: `crates/pgevolve-core/src/lint/universal.rs`

- [ ] **Step 1: Add the rule**

Edit `crates/pgevolve-core/src/lint/universal.rs`. Add a new rule function in the same shape as the existing `view-shadows-table` / `type-shadows-table` rules. The rule reads `catalog.extensions` and emits a Warning for every extension where `version.is_none()`.

```rust
/// `extension-version-unpinned` — fires when a source-declared extension
/// has no `VERSION` clause. Unpinned extensions can shift between
/// environments; pinning ensures dev and prod install the same version.
fn extension_version_unpinned(source: &Catalog, findings: &mut Vec<Finding>) {
    for e in &source.extensions {
        if e.version.is_none() {
            findings.push(Finding {
                rule: "extension-version-unpinned".into(),
                severity: Severity::Warning,
                message: format!(
                    "{}: extension is declared without a VERSION clause. Pinning the version \
                     ensures the same version is installed across environments.",
                    e.name,
                ),
                location: None,
            });
        }
    }
}
```

Find the existing `run_source_lints` (or equivalent — `grep -n "pub fn run_source_lints\|run_universal\|run_drift_lints" crates/pgevolve-core/src/lint/universal.rs`) and add a call to `extension_version_unpinned(source, &mut findings)`.

- [ ] **Step 2: Update the rule list docstring at the top**

Edit `crates/pgevolve-core/src/lint/universal.rs`'s top-level module docstring (the `//!` block). Add the rule name to the list of universal rules:

```rust
//! - **`extension-version-unpinned`** — fires when a source-declared
//!   extension lacks a `VERSION` clause.
```

- [ ] **Step 3: Add tests**

In the existing tests module within `universal.rs`, add:

```rust
    #[test]
    fn extension_version_unpinned_fires_on_unpinned() {
        let mut source = Catalog::empty();
        source.extensions.push(Extension {
            name: Identifier::from_unquoted("pgcrypto").unwrap(),
            schema: None,
            version: None,
            comment: None,
        });
        let findings = run_source_lints(&source);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-version-unpinned")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn extension_version_unpinned_silent_when_pinned() {
        let mut source = Catalog::empty();
        source.extensions.push(Extension {
            name: Identifier::from_unquoted("pgcrypto").unwrap(),
            schema: None,
            version: Some("1.3".into()),
            comment: None,
        });
        let findings = run_source_lints(&source);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-version-unpinned")
            .count();
        assert_eq!(count, 0);
    }
```

If `run_source_lints` is named differently (e.g., `run_lints`, `lint_source`), use the actual name from the file. Add a `use crate::ir::extension::Extension;` import to the tests module if not already present.

- [ ] **Step 4: Run tests + clippy**

```
cargo test -p pgevolve-core --lib lint::universal
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: 2 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/lint/universal.rs
git commit -m "$(cat <<'EOF'
feat(lint): extension-version-unpinned warning rule

Fires when a source-declared CREATE EXTENSION lacks a VERSION clause.
Pinning protects against extensions shifting between environments.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Lint rule — `extension-references-unmanaged-schema`

**Files:**
- Modify: `crates/pgevolve-core/src/lint/universal.rs`

- [ ] **Step 1: Add the rule**

Edit `crates/pgevolve-core/src/lint/universal.rs`. Add another rule function:

```rust
/// `extension-references-unmanaged-schema` — fires when a CREATE EXTENSION
/// references a schema not in the source catalog. Without the schema
/// being managed, the planner can't guarantee ordering.
fn extension_references_unmanaged_schema(source: &Catalog, findings: &mut Vec<Finding>) {
    use std::collections::BTreeSet;
    let managed_schemas: BTreeSet<&str> = source.schemas.iter().map(|s| s.name.as_str()).collect();
    for e in &source.extensions {
        if let Some(schema) = &e.schema
            && !managed_schemas.contains(schema.as_str())
        {
            findings.push(Finding {
                rule: "extension-references-unmanaged-schema".into(),
                severity: Severity::Error,
                message: format!(
                    "{}: WITH SCHEMA {} references a schema not declared in source. \
                     Add a CREATE SCHEMA {} to the source or remove the WITH SCHEMA clause.",
                    e.name, schema, schema,
                ),
                location: None,
            });
        }
    }
}
```

Wire it into `run_source_lints` next to `extension_version_unpinned`.

Update the module-level docstring to include the new rule.

- [ ] **Step 2: Add tests**

```rust
    #[test]
    fn extension_references_unmanaged_schema_fires() {
        let mut source = Catalog::empty();
        source.extensions.push(Extension {
            name: Identifier::from_unquoted("pg_trgm").unwrap(),
            schema: Some(Identifier::from_unquoted("missing").unwrap()),
            version: None,
            comment: None,
        });
        let findings = run_source_lints(&source);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-references-unmanaged-schema")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn extension_references_managed_schema_silent() {
        let mut source = Catalog::empty();
        source.schemas.push(Schema::new(Identifier::from_unquoted("app").unwrap()));
        source.extensions.push(Extension {
            name: Identifier::from_unquoted("pg_trgm").unwrap(),
            schema: Some(Identifier::from_unquoted("app").unwrap()),
            version: None,
            comment: None,
        });
        let findings = run_source_lints(&source);
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-references-unmanaged-schema")
            .count();
        assert_eq!(count, 0);
    }
```

- [ ] **Step 3: Run tests + clippy**

```
cargo test -p pgevolve-core --lib lint::universal
cargo clippy -p pgevolve-core --all-targets -- -D warnings
```
Expected: 2 new tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/lint/universal.rs
git commit -m "$(cat <<'EOF'
feat(lint): extension-references-unmanaged-schema error rule

Fires when CREATE EXTENSION ... WITH SCHEMA s declares a schema
that isn't in the source catalog. The planner can't guarantee
creation ordering for an unmanaged schema; the user must either
declare it (CREATE SCHEMA s) or drop the WITH SCHEMA clause.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Conformance fixtures

**Files:**
- Create: ~12 fixture directories under `crates/pgevolve-conformance/tests/cases/`

- [ ] **Step 1: Inspect the established fixture layout**

Run: `ls crates/pgevolve-conformance/tests/cases/objects/functions/` and pick one fixture (e.g., `create-simple`). Read its files:

```
crates/pgevolve-conformance/tests/cases/objects/functions/create-simple/
  fixture.toml
  before.sql
  after.sql
  expected/
    plan.sql
    diff.txt
    dep-graph.dot
```

Mirror that layout for every extension fixture.

- [ ] **Step 2: Create the 12 fixtures**

Create each of the following directories under `crates/pgevolve-conformance/tests/cases/objects/extensions/` with a `fixture.toml`, `before.sql`, `after.sql`, and `expected/{plan.sql,diff.txt,dep-graph.dot}`. Generate the expected golden files by running `cargo xtask bless --conformance` after the fixture inputs are in place (or, for each one, generate the goldens by hand-running the pipeline once with a `PGEVOLVE_BLESS=1` env var if that's how xtask bless works — read `xtask/src/main.rs` for the exact protocol).

| Fixture path | before.sql | after.sql | What it exercises |
|---|---|---|---|
| `objects/extensions/create-simple` | `CREATE SCHEMA app;` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto;` | bare CREATE |
| `objects/extensions/create-with-schema` | `CREATE SCHEMA app;` | `CREATE SCHEMA app; CREATE EXTENSION pg_trgm WITH SCHEMA app;` | WITH SCHEMA branch |
| `objects/extensions/create-with-version` | `CREATE SCHEMA app;` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto VERSION '1.3';` | VERSION branch |
| `objects/extensions/drop-simple` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto;` | `CREATE SCHEMA app;` | DROP CASCADE + intent required |
| `objects/extensions/update-version` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto VERSION '1.3';` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto VERSION '1.4';` | AlterUpdate path (note: PG must actually have both versions available, or use whichever versions ship with the PG14-PG17 base image) |
| `objects/extensions/replace-schema` | `CREATE SCHEMA app; CREATE SCHEMA gis; CREATE EXTENSION pg_trgm WITH SCHEMA app;` | `CREATE SCHEMA app; CREATE SCHEMA gis; CREATE EXTENSION pg_trgm WITH SCHEMA gis;` | ReplaceWithCascade + intent |
| `objects/extensions/comment-on` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto;` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto; COMMENT ON EXTENSION pgcrypto IS 'crypto helpers';` | CommentOn |
| `objects/extensions/version-pin-noop` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto;` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto VERSION '<whatever-version-pg-installs>';` | source-pinned matches catalog → empty plan |
| `objects/extensions/version-unpinned-noop` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto;` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto;` | unpinned source matches anything → empty plan |
| `scenarios/extension-owned-objects-ignored` | `CREATE SCHEMA app; CREATE EXTENSION pg_trgm WITH SCHEMA app;` | `CREATE SCHEMA app; CREATE EXTENSION pg_trgm WITH SCHEMA app;` | empty plan; pg_trgm installs operators in `app`; the deptype='e' filter must skip them |
| `scenarios/create-order-schema-first` | (empty) | `CREATE SCHEMA app; CREATE EXTENSION pg_trgm WITH SCHEMA app;` | verify schema step precedes extension step in plan.sql |
| `objects/extensions/lint-unpinned-warning` | `CREATE SCHEMA app;` | `CREATE SCHEMA app; CREATE EXTENSION pgcrypto;` | lint warning fires on unpinned source; fixture.toml sets `[expect.lint]` for the rule |

For the `drop-simple` and `replace-schema` fixtures, add an `[expect.apply]` block in `fixture.toml`:

```toml
[expect.apply]
intent = ["destructive"]   # exact key matching whichever variant the fixture loader expects for intent assertion
```

Refer to `crates/pgevolve-conformance/AUTHORING.md` (if it exists) or one of the existing destructive fixtures (e.g., `objects/functions/drop-simple`) for the exact `fixture.toml` schema. Each fixture's expected goldens (`plan.sql`, `diff.txt`, `dep-graph.dot`) are generated by `cargo xtask bless --conformance` once the fixture inputs compile.

- [ ] **Step 3: Generate goldens**

Run: `cargo xtask bless --conformance`
Expected: golden files appear under each fixture's `expected/` directory. Inspect a few to ensure the generated plan.sql contains the expected extension SQL.

- [ ] **Step 4: Run the conformance suite**

```
cargo test -p pgevolve-conformance --test run
```
Expected: PASS. All new fixtures green plus all pre-existing fixtures.

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-conformance/tests/cases/objects/extensions/ crates/pgevolve-conformance/tests/cases/scenarios/extension-owned-objects-ignored/ crates/pgevolve-conformance/tests/cases/scenarios/create-order-schema-first/
git commit -m "$(cat <<'EOF'
test(conformance): 12 extension fixtures covering create/drop/update/comment

Includes:
- create-simple, create-with-schema, create-with-version
- drop-simple (intent required)
- update-version (AlterUpdate path)
- replace-schema (ReplaceWithCascade + intent)
- comment-on
- version-pin-noop, version-unpinned-noop (asymmetry rules)
- scenarios/extension-owned-objects-ignored (pg_depend deptype='e' filter)
- scenarios/create-order-schema-first (schema → extension dep edge)
- lint-unpinned-warning (lint integration)

Goldens generated via `cargo xtask bless --conformance`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Documentation updates

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/spec/objects.md`
- Modify: `docs/spec/cli.md`

- [ ] **Step 1: Update README sub-spec progress table**

In `README.md`, find the v0.2 sub-spec progress table. Change sub-spec #3 status from `📋 Planned` to `✅ Landed <SHA>` (use the actual merge SHA after this branch is integrated; for the in-flight plan use `IN PROGRESS`).

Add a new section below the existing `### v0.2 functions and procedures` summary, modeled on the existing sub-spec summaries:

```markdown
### v0.2 extensions — what's in `<SHA>`

| Feature | Status |
|---|---|
| `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']` parser | ✅ Implemented |
| Catalog reader for `pg_extension` | ✅ Implemented |
| `pg_depend deptype='e'` filter on every other catalog query | ✅ Implemented |
| `ExtensionChange` variants: Create, Drop, AlterUpdate, ReplaceWithCascade, CommentOn | ✅ Implemented |
| 4 new step kinds: CreateExtension, DropExtension (destructive), AlterExtensionUpdate, CommentOnExtension | ✅ Implemented |
| Source-`None` symmetry for schema and version (unpinned = "any") | ✅ Implemented |
| 2 new lint rules (`extension-version-unpinned`, `extension-references-unmanaged-schema`) | ✅ Implemented |
| 12 conformance fixtures | ✅ Implemented |
```

- [ ] **Step 2: Update CHANGELOG**

In `CHANGELOG.md`, under the `## [0.2.0] — Unreleased` section, add a new subsection for extensions modeled on the existing function/type subsections:

```markdown
### Added — IR (extensions)

- `Extension { name, schema: Option<Identifier>, version: Option<String>, comment: Option<String> }` flat IR type in `pgevolve-core::ir::extension`.
- `Catalog::extensions: Vec<Extension>` flat collection.

### Added — pipeline (extensions)

- **Source parser** — `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']` parses into the `Extension` IR. `CASCADE`, `FROM old_version`, and unknown options rejected with `UnsupportedClause`.
- **Catalog reader** — queries `pg_extension` joined with `pg_namespace` and `pg_description`. The reader for every other object kind (tables, indexes, sequences, functions, types, views, MVs) gains a `NOT EXISTS (pg_depend deptype='e')` filter so extension-owned objects never appear as drift.
- **Differ** — `ExtensionChange` variants: `Create`, `Drop`, `AlterUpdate`, `ReplaceWithCascade`, `CommentOn`. Source-`None` for schema or version means "any catalog value", so unpinned source declarations don't diff against any installed version.
- **Planner** — 4 new step kinds: `CreateExtension`, `DropExtension` (destructive), `AlterExtensionUpdate`, `CommentOnExtension`. Schema changes go through `DropExtension` + `CreateExtension` with linked intent.
- **`NodeId::Extension`** — added to the dep graph; `Extension → Schema` edges force the schema to exist before the extension is created.

### Added — lint rules (extensions)

- `extension-version-unpinned` (Warning) — `CREATE EXTENSION foo;` without a `VERSION` clause.
- `extension-references-unmanaged-schema` (Error) — `WITH SCHEMA gis` but `gis` isn't in the source catalog.

### Added — tests (extensions)

- **12 conformance fixtures** (Tier C): `objects/extensions/` covering create/drop/update/replace/comment paths plus version-pin and version-unpinned no-op cases. `scenarios/extension-owned-objects-ignored` exercises the `pg_depend deptype='e'` filter. `scenarios/create-order-schema-first` verifies the `Extension → Schema` dep ordering.
```

- [ ] **Step 3: Update objects spec**

Edit `docs/spec/objects.md`. Find the `EXTENSION` row and change status from `📋 Planned, v0.2` to `✅ Implemented` with updated notes. Same for `Extension version upgrade (ALTER EXTENSION ... UPDATE)`.

- [ ] **Step 4: Run a smoke check that all docs render**

```
git diff --stat docs/ README.md CHANGELOG.md
```
Visual check: the sub-spec table entry is updated, the CHANGELOG additions match the actual implementation, and `docs/spec/objects.md` reflects the new status.

- [ ] **Step 5: Commit**

```bash
git add README.md CHANGELOG.md docs/spec/objects.md docs/spec/cli.md
git commit -m "$(cat <<'EOF'
docs: v0.2 sub-spec #3 extensions landed

README sub-spec table, CHANGELOG, spec/objects.md updated.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Workspace verification

**Files:** none modified — verification only.

- [ ] **Step 1: Full workspace test suite**

Run: `cargo test --workspace --lib --tests`
Expected: all green.

- [ ] **Step 2: Workspace clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Format check**

Run: `cargo fmt --check`
Expected: no output. If any fmt fixups, apply and commit them as a follow-up `chore(extensions): post-merge fmt fixups`.

- [ ] **Step 4: Conformance across PG 14–17**

Run:
```
for v in 14 15 16 17; do
  echo "=== PG $v ==="
  PGEVOLVE_TEST_PG_VERSION=$v cargo test -p pgevolve-conformance --test run 2>&1 | tail -3
done
```
Expected: all 4 versions PASS.

If a specific version fails on a fixture that relies on a particular extension version being available (e.g., `update-version` uses pgcrypto 1.3 → 1.4 but a PG version only ships 1.3), the fix is either: (a) make the fixture's source SQL conditionally pin to whatever versions PG ships, or (b) skip the fixture for the affected PG version via `[expect.pg]` selectors in `fixture.toml`. Inspect the failure and adjust.

- [ ] **Step 5: Property tests**

Run:
```
cargo test -p pgevolve-core --test property_tests -- --include-ignored
cargo test -p pgevolve --test pg_property_tests -- --include-ignored
cargo test -p pgevolve --test chaos_apply -- --include-ignored
```
Expected: all PASS.

- [ ] **Step 6: Push to origin**

```bash
git push origin main
```

---

## Self-review pre-flight checklist for the implementing agent

- [ ] `Extension` IR struct exists; `Catalog::extensions` field populated by both parser and catalog reader.
- [ ] `pg_depend deptype='e'` filter applied to every applicable catalog query (verify via `grep -l deptype crates/pgevolve-core/src/catalog/queries/`).
- [ ] `ExtensionChange` enum + `Change::Extension(_)` variant.
- [ ] 4 new `StepKind` variants.
- [ ] `NodeId::Extension` + `Extension → Schema` edges in dep graph.
- [ ] `emit/extension.rs` exists as the 12th per-family dispatcher.
- [ ] `lint/universal.rs` carries `extension-version-unpinned` and `extension-references-unmanaged-schema` rules.
- [ ] ~12 conformance fixtures present and passing on PG 14–17.
- [ ] README + CHANGELOG + spec/objects.md updated.
- [ ] Workspace clippy clean, fmt clean.

---

## Out of scope (do NOT touch)

- `[extensions]` block in `pgevolve.toml` — Decision Q1 was SQL-only.
- `ALTER EXTENSION ADD/DROP MEMBER`.
- Per-version update scripts (PG handles them).
- `ALTER EXTENSION SET SCHEMA` (relocatable extensions) — always DROP+CREATE.
- Reverse dep edges from managed objects to extensions.
- `lint/rules/` directory — does not exist in this codebase; both lints live in `lint/universal.rs`.
