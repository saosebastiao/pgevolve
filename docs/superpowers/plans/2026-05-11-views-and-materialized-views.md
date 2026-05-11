# Views and Materialized Views Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `VIEW` and `MATERIALIZED VIEW` management to pgevolve, including `CREATE OR REPLACE` semantics, dependent-view recreation, and `REFRESH MATERIALIZED VIEW [CONCURRENTLY]` as a planner step.

**Architecture:** Both source and catalog sides produce identical `View`/`MaterializedView` IR records. The source side runs through a shadow ephemeral Postgres so view bodies and dependency edges are read back from `pg_get_viewdef` and `pg_depend` — the same functions the catalog reader uses. The differ recognizes view-specific change kinds and the planner emits new step kinds. Plan output is byte-identical for byte-identical input (the existing determinism harness extends to the new step kinds).

**Tech Stack:** Rust, `pg_query` for source parsing, `tokio-postgres` for live + shadow PG, `testcontainers` for ephemeral PG, `serde` + `toml` for plan/intent serialization, `proptest` for property tests, `blake3` for IR hashing.

**Spec:** [`docs/superpowers/specs/2026-05-11-views-and-materialized-views-design.md`](../specs/2026-05-11-views-and-materialized-views-design.md). Read it before starting.

**Existing patterns to study before starting:**
- `crates/pgevolve-core/src/ir/table.rs` — IR record shape with derive-PartialEq and serde
- `crates/pgevolve-core/src/ir/index.rs` — IR record with `ObjectRef`-style fields
- `crates/pgevolve-core/src/parse/builder/create_stmt.rs` — `CreateStmt` → IR builder; the pattern to follow for `CreateViewStmt`
- `crates/pgevolve-core/src/catalog/queries/pg17.rs` — version-specific catalog SQL; copy-and-modify for each PG major
- `crates/pgevolve-core/src/diff/tables.rs` — diff pattern: produce a sequence of typed `Change` variants
- `crates/pgevolve-core/src/plan/raw_step.rs` — `StepKind` enum and how new variants get added
- `crates/pgevolve-core/src/plan/serialize.rs` — how steps become `plan.sql` lines + `intent.toml` rows
- `crates/pgevolve/src/shadow_pg.rs` — existing shadow-PG bootstrap used by `validate --shadow`
- `crates/pgevolve-conformance/tests/cases/tables/add-column-nullable/` — proof-of-life fixture; copy for new view fixtures

---

## Task 1: IR types — `View`, `MaterializedView`, `ViewColumn`, `DepEdge`, `CanonicalViewBody`

**Files:**
- Create: `crates/pgevolve-core/src/ir/view.rs`
- Create: `crates/pgevolve-core/src/ir/dep_edge.rs`
- Modify: `crates/pgevolve-core/src/ir/mod.rs` (add `mod view; mod dep_edge; pub use view::*; pub use dep_edge::*;`)
- Test: `crates/pgevolve-core/src/ir/view.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1.1: Write the failing test in `crates/pgevolve-core/src/ir/view.rs`**

```rust
//! View and materialized view IR records.

use crate::identifier::{ColumnName, ObjectName, SchemaName};
use crate::ir::dep_edge::DepEdge;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CanonicalViewBody(String);

impl CanonicalViewBody {
    /// Wrap a string that came from `pg_get_viewdef`. The constructor is intentionally
    /// `pub(crate)` — production code MUST NOT construct one from arbitrary user text.
    pub(crate) fn from_pg_get_viewdef(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ViewColumn {
    pub name: ColumnName,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct View {
    pub schema: SchemaName,
    pub name: ObjectName,
    pub columns: Vec<ViewColumn>,
    pub body_canonical: CanonicalViewBody,
    pub body_dependencies: Vec<DepEdge>,
    pub security_barrier: Option<bool>,
    pub security_invoker: Option<bool>,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct MaterializedView {
    pub schema: SchemaName,
    pub name: ObjectName,
    pub columns: Vec<ViewColumn>,
    pub body_canonical: CanonicalViewBody,
    pub body_dependencies: Vec<DepEdge>,
    pub comment: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_view() -> View {
        View {
            schema: SchemaName::new("app").unwrap(),
            name: ObjectName::new("users_summary").unwrap(),
            columns: vec![ViewColumn { name: ColumnName::new("id").unwrap(), comment: None }],
            body_canonical: CanonicalViewBody::from_pg_get_viewdef(" SELECT users.id FROM users;"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
        }
    }

    #[test]
    fn views_with_equal_fields_compare_equal() {
        assert_eq!(sample_view(), sample_view());
    }

    #[test]
    fn views_diff_when_body_differs() {
        let mut other = sample_view();
        other.body_canonical = CanonicalViewBody::from_pg_get_viewdef(" SELECT id FROM users;");
        assert_ne!(sample_view(), other);
    }

    #[test]
    fn materialized_view_round_trips_through_serde() {
        let mv = MaterializedView {
            schema: SchemaName::new("app").unwrap(),
            name: ObjectName::new("daily_revenue").unwrap(),
            columns: vec![],
            body_canonical: CanonicalViewBody::from_pg_get_viewdef(" SELECT 1;"),
            body_dependencies: vec![],
            comment: None,
        };
        let json = serde_json::to_string(&mv).unwrap();
        let back: MaterializedView = serde_json::from_str(&json).unwrap();
        assert_eq!(mv, back);
    }
}
```

- [ ] **Step 1.2: Write `crates/pgevolve-core/src/ir/dep_edge.rs`**

```rust
//! Dependency edges produced by `pg_depend` reads.

use crate::identifier::{ColumnName, ObjectName, SchemaName};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum DepKind {
    Column,
    Function,
    Type,
    Sequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DepTarget {
    pub schema: SchemaName,
    pub name: ObjectName,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DepEdge {
    pub kind: DepKind,
    pub target: DepTarget,
    /// Populated when `kind = Column`. Maps to `pg_depend.refobjsubid` after column
    /// name resolution against `pg_attribute`.
    pub subobject: Option<ColumnName>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_edge_round_trips_through_serde() {
        let edge = DepEdge {
            kind: DepKind::Column,
            target: DepTarget {
                schema: SchemaName::new("app").unwrap(),
                name: ObjectName::new("users").unwrap(),
            },
            subobject: Some(ColumnName::new("email").unwrap()),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let back: DepEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(edge, back);
    }

    #[test]
    fn dep_edges_hash_as_keys() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        s.insert(DepEdge {
            kind: DepKind::Function,
            target: DepTarget {
                schema: SchemaName::new("app").unwrap(),
                name: ObjectName::new("now").unwrap(),
            },
            subobject: None,
        });
        assert_eq!(s.len(), 1);
    }
}
```

- [ ] **Step 1.3: Wire up the modules**

Edit `crates/pgevolve-core/src/ir/mod.rs` and add the two new modules in alphabetical order with the rest, then re-export their public types alongside the existing re-exports. Use `git diff` after to verify only those two lines moved.

- [ ] **Step 1.4: Run the tests**

```bash
cargo test -p pgevolve-core --lib ir::view ir::dep_edge -- --nocapture
```

Expected: 5 tests pass (3 in view, 2 in dep_edge).

- [ ] **Step 1.5: Commit**

```bash
git add crates/pgevolve-core/src/ir/view.rs crates/pgevolve-core/src/ir/dep_edge.rs crates/pgevolve-core/src/ir/mod.rs
git commit -m "feat(ir): View, MaterializedView, DepEdge types

Adds the IR records the v0.2 views sub-spec relies on. No parser,
diff, or planner integration yet — those land in later tasks."
```

---

## Task 2: IR — extend `Index::on` with an `ObjectRef::Mv` variant

**Files:**
- Modify: `crates/pgevolve-core/src/ir/index.rs`
- Test: `crates/pgevolve-core/src/ir/index.rs` (extend the existing `#[cfg(test)]` module)

- [ ] **Step 2.1: Read the current `Index` struct**

Open `crates/pgevolve-core/src/ir/index.rs` and locate the field that records what relation the index is on. The existing v0.1 code likely uses a plain `(schema, table)` pair. Audit how it's used across the diff and planner before changing it — a `git grep` for `Index::` and for the field name will surface every call site.

- [ ] **Step 2.2: Write the failing test**

```rust
#[test]
fn index_can_target_a_materialized_view() {
    let ix = Index {
        // … existing fields …
        on: IndexParent::Mv {
            schema: SchemaName::new("app").unwrap(),
            name: ObjectName::new("daily_revenue").unwrap(),
        },
        // … rest …
    };
    assert!(matches!(ix.on, IndexParent::Mv { .. }));
}
```

(Field/variant names: see Step 2.3 below for the exact shapes.)

- [ ] **Step 2.3: Introduce `IndexParent`**

Define:

```rust
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IndexParent {
    Table { schema: SchemaName, name: ObjectName },
    Mv { schema: SchemaName, name: ObjectName },
}
```

Migrate the existing field that holds the index's parent table to be an `IndexParent::Table { .. }`. Update every call site in `diff/indexes.rs`, `plan/`, `catalog/`, and any test fixture builders to construct the `Table` variant explicitly. `cargo check -p pgevolve-core` must pass before tests run.

- [ ] **Step 2.4: Run the tests**

```bash
cargo test -p pgevolve-core --lib
```

Expected: all existing tests still pass, plus the new one for the `Mv` variant.

- [ ] **Step 2.5: Commit**

```bash
git add crates/pgevolve-core/src/ir/index.rs crates/pgevolve-core/src/diff/ crates/pgevolve-core/src/plan/ crates/pgevolve-core/src/catalog/
git commit -m "feat(ir): Index::on becomes IndexParent { Table | Mv }

Prepares Index for materialized-view parents. v0.2 views sub-spec
needs this; v0.1 callers all migrate to the Table variant with no
behavior change."
```

---

## Task 3: Source parser — `CREATE VIEW` and `CREATE MATERIALIZED VIEW` → provisional IR

**Files:**
- Create: `crates/pgevolve-core/src/parse/builder/create_view_stmt.rs`
- Modify: `crates/pgevolve-core/src/parse/builder/mod.rs` (`mod create_view_stmt;`)
- Modify: `crates/pgevolve-core/src/parse/statement.rs` (route `CreateViewStmt` and `CreateTableAsStmt` to the new builder)
- Test: `crates/pgevolve-core/tests/parser_corpus.rs` (add view/MV fixtures)
- Test: new corpus files under `crates/pgevolve-core/tests/corpus/views/`

- [ ] **Step 3.1: Add the failing parser corpus fixtures**

Create these files (one fixture each — directory layout follows the existing pattern under `tests/corpus/`):

`crates/pgevolve-core/tests/corpus/views/simple/source.sql`:
```sql
CREATE VIEW app.users_summary AS
  SELECT u.id, u.email FROM app.users u;
```

`crates/pgevolve-core/tests/corpus/views/simple/expected.json`:
```json
{
  "views": [{
    "schema": "app",
    "name": "users_summary",
    "columns": [
      {"name": "id", "comment": null},
      {"name": "email", "comment": null}
    ],
    "body_canonical": "",
    "body_dependencies": [],
    "security_barrier": null,
    "security_invoker": null,
    "comment": null
  }]
}
```

`crates/pgevolve-core/tests/corpus/views/aliased-columns/source.sql`:
```sql
CREATE VIEW app.users_summary (uid, addr) AS
  SELECT id, email FROM app.users;
```

`crates/pgevolve-core/tests/corpus/views/aliased-columns/expected.json`:
```json
{
  "views": [{
    "schema": "app",
    "name": "users_summary",
    "columns": [
      {"name": "uid", "comment": null},
      {"name": "addr", "comment": null}
    ],
    "body_canonical": "",
    "body_dependencies": [],
    "security_barrier": null,
    "security_invoker": null,
    "comment": null
  }]
}
```

`crates/pgevolve-core/tests/corpus/views/security-barrier/source.sql`:
```sql
CREATE VIEW app.protected_users
  WITH (security_barrier = true) AS
  SELECT id FROM app.users;
```

`crates/pgevolve-core/tests/corpus/views/security-barrier/expected.json`:
```json
{
  "views": [{
    "schema": "app",
    "name": "protected_users",
    "columns": [{"name": "id", "comment": null}],
    "body_canonical": "",
    "body_dependencies": [],
    "security_barrier": true,
    "security_invoker": null,
    "comment": null
  }]
}
```

`crates/pgevolve-core/tests/corpus/matviews/simple/source.sql`:
```sql
CREATE MATERIALIZED VIEW app.daily_revenue AS
  SELECT date_trunc('day', created_at) AS day, sum(amount) AS total
  FROM app.orders
  GROUP BY 1
  WITH NO DATA;
```

`crates/pgevolve-core/tests/corpus/matviews/simple/expected.json`:
```json
{
  "materialized_views": [{
    "schema": "app",
    "name": "daily_revenue",
    "columns": [
      {"name": "day", "comment": null},
      {"name": "total", "comment": null}
    ],
    "body_canonical": "",
    "body_dependencies": [],
    "comment": null
  }]
}
```

The provisional record's `body_canonical` is an **empty string** at the parse stage — Task 4's shadow-load pass fills it. Likewise `body_dependencies` is empty.

- [ ] **Step 3.2: Run the corpus tests to confirm they fail**

```bash
cargo test -p pgevolve-core --test parser_corpus -- views matviews
```

Expected: tests fail because the parser doesn't yet handle these statement kinds (either panics on unknown stmt or returns an empty IR depending on existing behavior).

- [ ] **Step 3.3: Write the builder in `crates/pgevolve-core/src/parse/builder/create_view_stmt.rs`**

```rust
//! Source-side parsing of `CREATE VIEW` and `CREATE MATERIALIZED VIEW`.
//!
//! The builder produces a *provisional* `View` / `MaterializedView` IR record
//! with `body_canonical = ""` and `body_dependencies = vec![]`. The shadow-load
//! pass (see `src/parse/shadow.rs`) fills those fields after the source IR is
//! complete enough to load into an ephemeral PG.

use crate::ir::{CanonicalViewBody, MaterializedView, View, ViewColumn};
use crate::identifier::{ColumnName, ObjectName, SchemaName};
use crate::parse::error::ParseError;
use pg_query::protobuf::{CreateTableAsStmt, ViewStmt};

pub(crate) fn build_view(stmt: &ViewStmt) -> Result<View, ParseError> {
    let (schema, name) = qualified_name_from_rangevar(stmt.view.as_ref())?;
    let columns = view_columns_from_stmt(stmt)?;
    let (security_barrier, security_invoker) = view_reloptions(stmt)?;
    Ok(View {
        schema,
        name,
        columns,
        body_canonical: CanonicalViewBody::from_pg_get_viewdef(""),
        body_dependencies: Vec::new(),
        security_barrier,
        security_invoker,
        comment: None,
    })
}

pub(crate) fn build_materialized_view(
    stmt: &CreateTableAsStmt,
) -> Result<MaterializedView, ParseError> {
    let into = stmt.into.as_ref().ok_or_else(|| ParseError::generic("CREATE MATERIALIZED VIEW missing INTO clause"))?;
    let rel = into.rel.as_ref().ok_or_else(|| ParseError::generic("CREATE MATERIALIZED VIEW missing target relation"))?;
    let (schema, name) = qualified_name_from_rangevar(rel)?;
    let columns = mv_columns_from_stmt(stmt)?;
    Ok(MaterializedView {
        schema,
        name,
        columns,
        body_canonical: CanonicalViewBody::from_pg_get_viewdef(""),
        body_dependencies: Vec::new(),
        comment: None,
    })
}

fn qualified_name_from_rangevar(
    rv: &pg_query::protobuf::RangeVar,
) -> Result<(SchemaName, ObjectName), ParseError> {
    // Follow the existing pattern from `parse/builder/shared.rs`. If `rv.schemaname`
    // is empty, fall back to the default schema policy already used by other builders.
    crate::parse::builder::shared::qualified_name_from_rangevar(rv)
}

fn view_columns_from_stmt(stmt: &ViewStmt) -> Result<Vec<ViewColumn>, ParseError> {
    // ViewStmt::aliases holds the optional aliased column list (CREATE VIEW v(a, b) ...).
    // When aliases is non-empty, those are the column names directly.
    // When aliases is empty, derive column names from the SELECT target list.
    if !stmt.aliases.is_empty() {
        stmt.aliases
            .iter()
            .map(|node| {
                let alias = crate::parse::builder::shared::str_from_string_node(node)?;
                Ok(ViewColumn {
                    name: ColumnName::new(&alias)?,
                    comment: None,
                })
            })
            .collect()
    } else {
        view_columns_from_select_target_list(stmt.query.as_ref())
    }
}

fn mv_columns_from_stmt(
    stmt: &CreateTableAsStmt,
) -> Result<Vec<ViewColumn>, ParseError> {
    // CreateTableAsStmt does not have `aliases` directly; if the target relation
    // carries an alias list, use it; otherwise derive from the SELECT target list.
    let into = stmt.into.as_ref().unwrap();
    if !into.col_names.is_empty() {
        into.col_names
            .iter()
            .map(|node| {
                let alias = crate::parse::builder::shared::str_from_string_node(node)?;
                Ok(ViewColumn {
                    name: ColumnName::new(&alias)?,
                    comment: None,
                })
            })
            .collect()
    } else {
        view_columns_from_select_target_list(stmt.query.as_ref())
    }
}

fn view_columns_from_select_target_list(
    query: Option<&pg_query::protobuf::Node>,
) -> Result<Vec<ViewColumn>, ParseError> {
    // Walk the SELECT's target list. Each `ResTarget` either has an explicit alias
    // (e.g., SELECT id AS uid) or derives the column name from the rightmost name
    // in the expression (e.g., SELECT users.email → "email"). For expressions that
    // can't be named (SELECT 1, SELECT 1 + 1), PG generates "?column?"; mirror that.
    //
    // Detailed walk: parse/builder/shared.rs has helpers `target_alias` and
    // `derived_column_name` — extend them if missing, then call them here.
    crate::parse::builder::shared::derive_view_columns_from_query(query)
}

fn view_reloptions(stmt: &ViewStmt) -> Result<(Option<bool>, Option<bool>), ParseError> {
    let mut barrier = None;
    let mut invoker = None;
    for opt in &stmt.options {
        let (name, value) = crate::parse::builder::shared::reloption_kv(opt)?;
        match name.as_str() {
            "security_barrier" => barrier = Some(parse_bool_reloption(&value)?),
            "security_invoker" => invoker = Some(parse_bool_reloption(&value)?),
            other => return Err(ParseError::generic(format!("unsupported view reloption: {other}"))),
        }
    }
    Ok((barrier, invoker))
}

fn parse_bool_reloption(value: &str) -> Result<bool, ParseError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" => Ok(true),
        "false" | "off" | "0" => Ok(false),
        other => Err(ParseError::generic(format!("expected boolean reloption value, got {other}"))),
    }
}
```

Note: the calls into `parse::builder::shared::*` will need new helper functions if they don't already exist (`str_from_string_node`, `qualified_name_from_rangevar`, `reloption_kv`, `derive_view_columns_from_query`). Add them following the patterns already used for table parsing. Each helper gets its own short unit test in `shared.rs`.

- [ ] **Step 3.4: Route view statements through the new builder**

In `crates/pgevolve-core/src/parse/statement.rs`, locate the match arms for `pg_query` statement variants. Add arms for `ViewStmt` and `CreateTableAsStmt` (the latter conditionally — `CreateTableAsStmt` can also produce regular tables via `CREATE TABLE AS`; gate on `into.relkind == 'm'` or the equivalent pg_query enum). Route each to the corresponding builder and append the result to the appropriate IR collection on the source-IR struct.

The source-IR struct (typically `SourceIr` or similar — confirm by reading `parse/mod.rs`) gains:

```rust
pub views: Vec<View>,
pub materialized_views: Vec<MaterializedView>,
```

- [ ] **Step 3.5: Run the corpus tests**

```bash
cargo test -p pgevolve-core --test parser_corpus -- views matviews
```

Expected: 4 view fixtures + 1 MV fixture pass.

- [ ] **Step 3.6: Commit**

```bash
git add crates/pgevolve-core/src/parse/ crates/pgevolve-core/tests/corpus/views/ crates/pgevolve-core/tests/corpus/matviews/
git commit -m "feat(parse): CREATE VIEW and CREATE MATERIALIZED VIEW source parsing

Produces provisional IR records — body_canonical and body_dependencies
remain empty until the shadow-load pass fills them in Task 4."
```

---

## Task 4: Shadow-load pass — fill `body_canonical` and `body_dependencies`

**Files:**
- Create: `crates/pgevolve-core/src/parse/shadow.rs`
- Modify: `crates/pgevolve-core/src/parse/mod.rs` (`pub mod shadow;` + a `ShadowLoader` trait re-export)
- Modify: `crates/pgevolve/src/shadow_pg.rs` (implement the `ShadowLoader` trait against the live shadow PG)
- Test: `crates/pgevolve/tests/shadow_validate.rs` (extend the existing shadow harness with view fixtures)

The shadow-load pass is the chokepoint of correctness. Tests for it must run against a real PG.

- [ ] **Step 4.1: Define the `ShadowLoader` trait in `crates/pgevolve-core/src/parse/shadow.rs`**

```rust
//! Shadow-load pass: fills `body_canonical` and `body_dependencies` on
//! provisional View/MaterializedView records by routing the source IR
//! through an ephemeral PG and reading back pg_get_viewdef + pg_depend.

use crate::ir::{DepEdge, MaterializedView, View};
use crate::parse::SourceIr;
use std::error::Error;

/// Implemented by the `pgevolve` binary — `pgevolve-core` doesn't depend on
/// tokio-postgres directly. The trait lets the parser request a shadow load
/// without owning the connection pool.
pub trait ShadowLoader {
    type Error: Error + Send + Sync + 'static;

    /// Apply `ir`'s non-view objects to the shadow PG in dependency order, then
    /// the views and MVs. For each view/MV, return the canonical body string
    /// and the dependency edges. The returned vectors are 1:1 with
    /// `ir.views` and `ir.materialized_views` in index order.
    fn load_and_introspect(
        &mut self,
        ir: &SourceIr,
    ) -> Result<ShadowReport, Self::Error>;
}

#[derive(Clone, Debug)]
pub struct ShadowReport {
    pub views: Vec<ShadowViewInfo>,
    pub materialized_views: Vec<ShadowViewInfo>,
}

#[derive(Clone, Debug)]
pub struct ShadowViewInfo {
    pub body_canonical: String,
    pub body_dependencies: Vec<DepEdge>,
}

/// Returns `true` if `ir` contains at least one view or materialized view.
pub fn requires_shadow_pass(ir: &SourceIr) -> bool {
    !ir.views.is_empty() || !ir.materialized_views.is_empty()
}

/// Mutates `ir` in place, filling each View/MV's `body_canonical` and
/// `body_dependencies` from `report`. Indexes must match in order.
pub fn apply_shadow_report(ir: &mut SourceIr, report: ShadowReport) {
    for (v, info) in ir.views.iter_mut().zip(report.views) {
        v.body_canonical = crate::ir::CanonicalViewBody::from_pg_get_viewdef(info.body_canonical);
        v.body_dependencies = info.body_dependencies;
    }
    for (mv, info) in ir.materialized_views.iter_mut().zip(report.materialized_views) {
        mv.body_canonical = crate::ir::CanonicalViewBody::from_pg_get_viewdef(info.body_canonical);
        mv.body_dependencies = info.body_dependencies;
    }
}
```

- [ ] **Step 4.2: Implement `ShadowLoader` for the live shadow PG**

In `crates/pgevolve/src/shadow_pg.rs`, add an impl of `ShadowLoader` that:

1. Boots the existing `ShadowPg` (already used by `validate --shadow`).
2. Applies the source IR via the existing IR-to-SQL emit path (`Plan::to_sql` or the equivalent — locate it from `commands/validate.rs`). Use the same dependency-correct ordering already used by `validate --shadow`.
3. After every CREATE has succeeded, runs two queries per managed schema:
   - `SELECT n.nspname, c.relname, pg_get_viewdef(c.oid, true) FROM pg_class c JOIN pg_namespace n ON c.relnamespace = n.oid WHERE c.relkind = ANY(ARRAY['v','m']) AND n.nspname = $1` — gives the canonical body for each view/MV.
   - A `pg_depend` join (see the catalog reader pattern in Task 6) — gives the dep edges.
4. Indexes results by `(schema, name)` and aligns them with the input IR's `views` / `materialized_views` vectors (which are already deterministically sorted by the parser).
5. Returns `ShadowReport`.

If shadow boot fails (Docker not available), return a typed error that the caller surfaces as "shadow PG required for projects with views; set `PGEVOLVE_DISABLE_DOCKER_TESTS=1` to skip, or install Docker."

- [ ] **Step 4.3: Wire the shadow pass into the parse pipeline**

In `crates/pgevolve-core/src/parse/mod.rs`, add a public function:

```rust
pub fn load_source_with_shadow<L: shadow::ShadowLoader>(
    root: &std::path::Path,
    loader: &mut L,
) -> Result<SourceIr, ParseError> {
    let mut ir = load_source(root)?;
    if shadow::requires_shadow_pass(&ir) {
        let report = loader.load_and_introspect(&ir).map_err(ParseError::shadow)?;
        shadow::apply_shadow_report(&mut ir, report);
    }
    Ok(ir)
}
```

Update every call site in `pgevolve` (commands/diff, commands/plan, commands/validate) to pass a concrete `ShadowLoader` when reading source. For now use the same one the existing shadow path uses; caching arrives in Task 5.

- [ ] **Step 4.4: Write an integration test in `crates/pgevolve/tests/shadow_validate.rs`**

```rust
#[test]
#[cfg_attr(not(feature = "docker-tests"), ignore)]
fn shadow_pass_fills_view_body_canonical() {
    // 1. Build a tiny source tree with one table and one view.
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(
        tree.path().join("schemas.sql"),
        "CREATE SCHEMA app;",
    ).unwrap();
    std::fs::write(
        tree.path().join("users.sql"),
        "CREATE TABLE app.users (id int PRIMARY KEY, email text);",
    ).unwrap();
    std::fs::write(
        tree.path().join("users_summary.sql"),
        "CREATE VIEW app.users_summary AS SELECT id, email FROM app.users;",
    ).unwrap();

    // 2. Boot shadow, run parse with shadow pass.
    let pg = ShadowPg::boot(PG_MAJOR_DEFAULT).unwrap();
    let mut loader = pg.shadow_loader();
    let ir = pgevolve_core::parse::load_source_with_shadow(tree.path(), &mut loader).unwrap();

    // 3. Assert body_canonical is non-empty and references the underlying table.
    assert_eq!(ir.views.len(), 1);
    let body = ir.views[0].body_canonical.as_str();
    assert!(body.contains("FROM"), "expected SQL body, got {body:?}");
    assert!(body.contains("users"));

    // 4. Assert at least one column-level dep edge to app.users.
    let deps = &ir.views[0].body_dependencies;
    assert!(
        deps.iter().any(|d|
            d.target.schema.as_str() == "app"
            && d.target.name.as_str() == "users"
            && d.subobject.is_some()),
        "expected column-level dep edge to app.users, got {deps:?}",
    );
}
```

- [ ] **Step 4.5: Run the test**

```bash
cargo test -p pgevolve --test shadow_validate --features docker-tests -- shadow_pass_fills_view_body_canonical
```

Expected: PASS. (If your dev box doesn't have Docker, you can `PGEVOLVE_DISABLE_DOCKER_TESTS=1` to skip but you must run this test in CI before commit.)

- [ ] **Step 4.6: Commit**

```bash
git add crates/pgevolve-core/src/parse/shadow.rs crates/pgevolve-core/src/parse/mod.rs crates/pgevolve/src/shadow_pg.rs crates/pgevolve/src/commands/ crates/pgevolve/tests/shadow_validate.rs
git commit -m "feat(parse): shadow-load pass fills view body_canonical + deps

Routes source IR through an ephemeral PG when views or MVs are present,
then reads pg_get_viewdef + pg_depend back into the IR records. Required
for diff/plan whenever the source tree contains views."
```

---

## Task 5: Shadow-load cache

**Files:**
- Create: `crates/pgevolve/src/shadow_cache.rs`
- Modify: `crates/pgevolve/src/shadow_pg.rs` (wrap `ShadowLoader` impl in a caching layer)
- Modify: `crates/pgevolve/src/commands/init.rs` (add `.pgevolve/cache/` to the default generated `.gitignore`)
- Test: `crates/pgevolve/tests/shadow_validate.rs` (assert second run skips the boot)

- [ ] **Step 5.1: Define cache key + writer**

```rust
// crates/pgevolve/src/shadow_cache.rs
use blake3::Hasher;
use pgevolve_core::parse::SourceIr;
use pgevolve_core::parse::shadow::ShadowReport;
use std::path::{Path, PathBuf};

pub struct ShadowCache {
    dir: PathBuf,
}

impl ShadowCache {
    pub fn for_project(project_root: &Path) -> Self {
        Self { dir: project_root.join(".pgevolve").join("cache").join("shadow") }
    }

    pub fn key(ir: &SourceIr, pg_major: u16) -> String {
        // Follow the existing pattern from `pgevolve-core::plan::plan::PlanId::compute`:
        // bincode-encode for determinism, then blake3-hash. SourceIr must derive
        // serde::Serialize for this to compile — verify it does (it should, since
        // tier-2 fixtures already serialize it to JSON for goldens).
        let mut h = Hasher::new();
        h.update(b"pgevolve-shadow-cache-v1\n");
        h.update(&pg_major.to_le_bytes());
        h.update(&[0]);
        let cfg = bincode::config::standard();
        let bytes = bincode::serde::encode_to_vec(ir, cfg)
            .expect("SourceIr is bincode-serializable");
        h.update(&bytes);
        h.finalize().to_hex().to_string()
    }

    pub fn get(&self, key: &str) -> Option<ShadowReport> {
        let path = self.dir.join(format!("{key}.json"));
        let bytes = std::fs::read(&path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn put(&self, key: &str, report: &ShadowReport) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{key}.json"));
        let bytes = serde_json::to_vec(report).unwrap();
        std::fs::write(path, bytes)
    }
}
```

Add `#[derive(Serialize, Deserialize)]` to `ShadowReport` and `ShadowViewInfo` if they don't have it from Task 4.

- [ ] **Step 5.2: Wrap the shadow loader**

In `crates/pgevolve/src/shadow_pg.rs`:

```rust
pub struct CachedShadowLoader<'a> {
    inner: &'a mut dyn ShadowLoader<Error = ShadowError>,
    cache: ShadowCache,
    pg_major: u16,
}

impl<'a> ShadowLoader for CachedShadowLoader<'a> {
    type Error = ShadowError;
    fn load_and_introspect(&mut self, ir: &SourceIr) -> Result<ShadowReport, ShadowError> {
        let key = ShadowCache::key(ir, self.pg_major);
        if let Some(hit) = self.cache.get(&key) {
            return Ok(hit);
        }
        let report = self.inner.load_and_introspect(ir)?;
        let _ = self.cache.put(&key, &report); // cache write failure is non-fatal
        Ok(report)
    }
}
```

- [ ] **Step 5.3: Add `.pgevolve/cache/` to `init`'s default `.gitignore`**

`crates/pgevolve/src/commands/init.rs`: locate the gitignore-emit code (or `.gitignore` template); append `/.pgevolve/cache/`. Update the matching test fixture for `init` if there is one.

- [ ] **Step 5.4: Test that second run hits the cache**

```rust
#[test]
#[cfg_attr(not(feature = "docker-tests"), ignore)]
fn shadow_load_cache_skips_second_boot() {
    let tree = build_tiny_view_project(); // factor out the boilerplate from Task 4's test
    let project = tempfile::tempdir().unwrap();

    // First run boots PG.
    let pg = ShadowPg::boot(PG_MAJOR_DEFAULT).unwrap();
    let cache = ShadowCache::for_project(project.path());
    let mut inner = pg.shadow_loader();
    let mut cached = CachedShadowLoader { inner: &mut inner, cache, pg_major: PG_MAJOR_DEFAULT };

    let _ir1 = pgevolve_core::parse::load_source_with_shadow(tree.path(), &mut cached).unwrap();

    // Second run with a fresh CachedShadowLoader pointing at a never-booted inner.
    let mut sentinel = NeverBootLoader::default();
    let cache2 = ShadowCache::for_project(project.path());
    let mut cached2 = CachedShadowLoader { inner: &mut sentinel, cache: cache2, pg_major: PG_MAJOR_DEFAULT };
    let _ir2 = pgevolve_core::parse::load_source_with_shadow(tree.path(), &mut cached2).unwrap();

    assert_eq!(sentinel.calls, 0, "second run should have hit cache, not booted PG");
}

#[derive(Default)]
struct NeverBootLoader { calls: usize }
impl ShadowLoader for NeverBootLoader {
    type Error = ShadowError;
    fn load_and_introspect(&mut self, _ir: &SourceIr) -> Result<ShadowReport, ShadowError> {
        self.calls += 1;
        panic!("inner loader should not be called when cache is warm");
    }
}
```

- [ ] **Step 5.5: Run the test**

```bash
cargo test -p pgevolve --test shadow_validate --features docker-tests -- shadow_load_cache_skips_second_boot
```

Expected: PASS.

- [ ] **Step 5.6: Commit**

```bash
git add crates/pgevolve/src/shadow_cache.rs crates/pgevolve/src/shadow_pg.rs crates/pgevolve/src/commands/init.rs crates/pgevolve/tests/shadow_validate.rs
git commit -m "feat(shadow): cache shadow-load results by (source-IR hash, pg_major)

Source trees with views amortize the shadow-PG boot across diff/plan
runs on unchanged source. Cache lives in .pgevolve/cache/shadow/ and
is added to init's default .gitignore."
```

---

## Task 6: Catalog reader — `read_views`, `read_materialized_views`, index parent extension

**Files:**
- Create: `crates/pgevolve-core/src/catalog/queries/views.rs` (shared SQL fragments)
- Modify: `crates/pgevolve-core/src/catalog/queries/{pg14,pg15,pg16,pg17}.rs` (add view + MV queries; `security_invoker` is PG15+)
- Modify: `crates/pgevolve-core/src/catalog/queries/mod.rs` (expose new queries via the per-version trait)
- Modify: `crates/pgevolve-core/src/catalog/rows.rs` (add `ViewRow`, `MvRow`, `DepEdgeRow`)
- Modify: `crates/pgevolve-core/src/catalog/assemble.rs` (fold rows into `Vec<View>` / `Vec<MaterializedView>`)
- Modify: `crates/pgevolve-core/src/catalog/mod.rs` (`CatalogIr` gains `views` and `materialized_views` fields)
- Modify: existing index reader to accept `relkind IN ('r','m')` for the parent
- Test: `crates/pgevolve-core/tests/catalog_round_trip.rs`

- [ ] **Step 6.1: Add a failing round-trip test**

```rust
// crates/pgevolve-core/tests/catalog_round_trip.rs
#[test]
#[cfg_attr(not(feature = "docker-tests"), ignore)]
fn catalog_reads_view_with_security_barrier() {
    let pg = EphemeralPostgres::boot(17).unwrap();
    pg.exec("CREATE SCHEMA app;").unwrap();
    pg.exec("CREATE TABLE app.users (id int);").unwrap();
    pg.exec(r#"
        CREATE VIEW app.protected
          WITH (security_barrier = true)
          AS SELECT id FROM app.users;
    "#).unwrap();

    let ir = pgevolve_core::catalog::read(&pg.client(), &["app".into()]).unwrap();
    assert_eq!(ir.views.len(), 1);
    let v = &ir.views[0];
    assert_eq!(v.schema.as_str(), "app");
    assert_eq!(v.name.as_str(), "protected");
    assert_eq!(v.security_barrier, Some(true));
    assert!(v.body_canonical.as_str().contains("FROM"));
}

#[test]
#[cfg_attr(not(feature = "docker-tests"), ignore)]
fn catalog_reads_mv_and_its_index() {
    let pg = EphemeralPostgres::boot(17).unwrap();
    pg.exec("CREATE SCHEMA app;").unwrap();
    pg.exec("CREATE TABLE app.orders (id int, amount numeric);").unwrap();
    pg.exec("CREATE MATERIALIZED VIEW app.totals AS SELECT 1 AS k, sum(amount) AS s FROM app.orders GROUP BY 1 WITH NO DATA;").unwrap();
    pg.exec("CREATE UNIQUE INDEX totals_uk ON app.totals (k);").unwrap();

    let ir = pgevolve_core::catalog::read(&pg.client(), &["app".into()]).unwrap();
    assert_eq!(ir.materialized_views.len(), 1);
    assert_eq!(ir.materialized_views[0].name.as_str(), "totals");
    assert_eq!(ir.indexes.len(), 1, "unique index on MV must surface");
    assert!(matches!(ir.indexes[0].on, IndexParent::Mv { .. }));
}
```

Run:
```bash
cargo test -p pgevolve-core --test catalog_round_trip --features docker-tests -- catalog_reads_view catalog_reads_mv
```
Expected: FAIL (no view reader yet).

- [ ] **Step 6.2: Add shared SQL fragments**

`crates/pgevolve-core/src/catalog/queries/views.rs`:

```rust
//! View / materialized-view catalog queries. Each per-version `pgNN.rs`
//! exposes these strings (or overrides for security_invoker etc.).

/// Returns one row per managed view or MV: schema, name, relkind ('v' or 'm'),
/// body, reloptions (jsonb).
pub const SELECT_VIEWS_AND_MVS: &str = r#"
SELECT
  n.nspname                            AS schema_name,
  c.relname                            AS name,
  c.relkind                            AS relkind,
  pg_get_viewdef(c.oid, true)          AS body,
  to_jsonb(coalesce(c.reloptions, '{}'::text[])) AS reloptions
FROM pg_class c
JOIN pg_namespace n ON c.relnamespace = n.oid
WHERE c.relkind IN ('v','m')
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, c.relname
"#;

/// Returns one row per column of every view/MV in the managed schemas.
pub const SELECT_VIEW_COLUMNS: &str = r#"
SELECT
  n.nspname     AS schema_name,
  c.relname     AS view_name,
  a.attnum      AS attnum,
  a.attname     AS column_name,
  d.description AS column_comment
FROM pg_class c
JOIN pg_namespace n  ON c.relnamespace = n.oid
JOIN pg_attribute a  ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
LEFT JOIN pg_description d ON d.objoid = c.oid AND d.objsubid = a.attnum
WHERE c.relkind IN ('v','m')
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, c.relname, a.attnum
"#;

/// Returns column-level dep edges from each view/MV's rewrite rule to objects
/// in pg_class, pg_proc, pg_type. `dep_subobject_column_name` is non-null when
/// the dep targets a specific column.
pub const SELECT_VIEW_DEPENDENCIES: &str = r#"
SELECT
  vn.nspname  AS view_schema,
  vc.relname  AS view_name,
  CASE refclassid
    WHEN 'pg_class'::regclass THEN
      CASE refkind WHEN 'r' THEN 'column' WHEN 'S' THEN 'sequence' ELSE 'column' END
    WHEN 'pg_proc'::regclass  THEN 'function'
    WHEN 'pg_type'::regclass  THEN 'type'
  END                       AS dep_kind,
  refsch.nspname            AS dep_schema,
  refname                   AS dep_name,
  CASE WHEN refclassid = 'pg_class'::regclass AND d.refobjsubid > 0
       THEN refatt.attname
       ELSE NULL END        AS dep_subobject_column_name
FROM pg_depend d
JOIN pg_rewrite r ON d.objid = r.oid AND d.classid = 'pg_rewrite'::regclass
JOIN pg_class vc ON r.ev_class = vc.oid
JOIN pg_namespace vn ON vc.relnamespace = vn.oid
LEFT JOIN LATERAL (
  SELECT
    CASE d.refclassid
      WHEN 'pg_class'::regclass THEN (SELECT relname FROM pg_class WHERE oid = d.refobjid)
      WHEN 'pg_proc'::regclass  THEN (SELECT proname FROM pg_proc  WHERE oid = d.refobjid)
      WHEN 'pg_type'::regclass  THEN (SELECT typname FROM pg_type  WHERE oid = d.refobjid)
    END AS refname,
    CASE d.refclassid
      WHEN 'pg_class'::regclass THEN (SELECT n.nspname FROM pg_class c JOIN pg_namespace n ON c.relnamespace = n.oid WHERE c.oid = d.refobjid)
      WHEN 'pg_proc'::regclass  THEN (SELECT n.nspname FROM pg_proc  p JOIN pg_namespace n ON p.pronamespace = n.oid WHERE p.oid = d.refobjid)
      WHEN 'pg_type'::regclass  THEN (SELECT n.nspname FROM pg_type  t JOIN pg_namespace n ON t.typnamespace = n.oid WHERE t.oid = d.refobjid)
    END AS refnsp,
    CASE d.refclassid
      WHEN 'pg_class'::regclass THEN (SELECT relkind FROM pg_class WHERE oid = d.refobjid)
    END AS refkind
) ref_lookup ON true
LEFT JOIN pg_namespace refsch ON refsch.nspname = ref_lookup.refnsp
LEFT JOIN pg_attribute refatt
  ON refatt.attrelid = d.refobjid AND refatt.attnum = d.refobjsubid
WHERE vn.nspname = ANY($1::text[])
  AND vc.relkind IN ('v','m')
  AND d.deptype = 'n' -- normal dependencies only
ORDER BY view_schema, view_name, dep_kind, dep_schema, dep_name, dep_subobject_column_name NULLS FIRST
"#;
```

The query above is deliberately verbose for clarity; tighten it after the round-trip tests pass if it's the hot path. Run the queries by hand against PG17 in a scratch container while writing the test to confirm shape.

- [ ] **Step 6.3: Per-version wiring**

In each `pg14.rs`/`pg15.rs`/`pg16.rs`/`pg17.rs`, expose `SELECT_VIEWS_AND_MVS`, `SELECT_VIEW_COLUMNS`, `SELECT_VIEW_DEPENDENCIES` via the per-version trait already used by other readers. `security_invoker` parsing applies only to PG 15+ — for PG14, parse the reloption array but ignore the `security_invoker` key (defensive; it won't appear there in practice).

- [ ] **Step 6.4: Wire rows → IR in `assemble.rs`**

Define `ViewRow`, `MvRow`, `ViewColumnRow`, `DepEdgeRow` types alongside the existing rows. Assembly steps:
1. Collect column rows into per-(schema, name) maps.
2. Collect dep edge rows into per-(schema, name) maps.
3. For each view-or-MV row, parse reloptions (the `to_jsonb(reloptions)` cast gives a `Vec<String>` of `"k=v"`), build the IR record with columns and dep edges joined in.
4. `relkind = 'v'` → push into `views`; `relkind = 'm'` → push into `materialized_views`.

- [ ] **Step 6.5: Extend the index reader for MV parents**

Locate the existing index reader (likely `catalog/queries/*.rs` and `assemble.rs`). The SQL filter that excludes non-table parents must accept `relkind IN ('r','m')`. The assembly code must produce `IndexParent::Mv` when the parent's relkind is `'m'`.

- [ ] **Step 6.6: Run the round-trip tests**

```bash
cargo test -p pgevolve-core --test catalog_round_trip --features docker-tests -- catalog_reads_view catalog_reads_mv
```

Expected: both pass.

- [ ] **Step 6.7: Tier-3 catalog goldens**

Add catalog-snapshot fixtures for views and MVs to the tier-3 corpus. Locate the existing goldens (likely `crates/pgevolve-core/tests/catalog_goldens/` — adjust path to match the actual repo). Regenerate via `cargo xtask bless`. Commit the resulting `.json` snapshots.

- [ ] **Step 6.8: Commit**

```bash
git add crates/pgevolve-core/src/catalog/ crates/pgevolve-core/tests/
git commit -m "feat(catalog): read views, MVs, dep edges, index-on-MV

Adds catalog readers for views and materialized views including their
column lists, security_barrier/invoker reloptions, and column-level
dependency edges. Index reader now accepts relkind='m' parents."
```

---

## Task 7: Differ — view and MV change kinds + OR-REPLACE compatibility predicate

**Files:**
- Create: `crates/pgevolve-core/src/diff/views.rs`
- Modify: `crates/pgevolve-core/src/diff/change.rs` (add `ViewChange`, `MvChange` variants to the `Change` enum)
- Modify: `crates/pgevolve-core/src/diff/mod.rs` (`mod views; pub use views::diff_views;` and call it from the top-level diff)
- Modify: `crates/pgevolve-core/src/diff/destructiveness.rs` (mark `View::Drop` destructive; `Mv::Drop` non-destructive)
- Test: `crates/pgevolve-core/src/diff/views.rs` (inline unit tests)

- [ ] **Step 7.1: Define the change variants**

In `crates/pgevolve-core/src/diff/change.rs`, extend the `Change` enum:

```rust
pub enum Change {
    // ...existing variants...
    ViewChange(ViewChange),
    MvChange(MvChange),
}

pub enum ViewChange {
    Create(View),
    Drop { schema: SchemaName, name: ObjectName },
    ReplaceBody { source: View, catalog: View, compatible: bool },
    SetReloption { schema: SchemaName, name: ObjectName, security_barrier: Option<bool>, security_invoker: Option<bool> },
    SetComment { schema: SchemaName, name: ObjectName, comment: Option<String> },
    SetColumnComment { schema: SchemaName, name: ObjectName, column: ColumnName, comment: Option<String> },
}

pub enum MvChange {
    Create(MaterializedView),
    Drop { schema: SchemaName, name: ObjectName },
    ReplaceBody { source: MaterializedView, catalog: MaterializedView },
    SetComment { schema: SchemaName, name: ObjectName, comment: Option<String> },
    SetColumnComment { schema: SchemaName, name: ObjectName, column: ColumnName, comment: Option<String> },
}
```

- [ ] **Step 7.2: Write the OR-REPLACE compatibility predicate (TDD)**

In `crates/pgevolve-core/src/diff/views.rs`:

```rust
#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn identical_column_lists_are_compatible() {
        let a = cols(&[("id","int4"), ("email","text")]);
        let b = cols(&[("id","int4"), ("email","text")]);
        assert!(or_replace_compatible(&a, &b));
    }

    #[test]
    fn appending_columns_is_compatible() {
        let a = cols(&[("id","int4")]);
        let b = cols(&[("id","int4"), ("email","text")]);
        assert!(or_replace_compatible(&a, &b));
    }

    #[test]
    fn renaming_a_column_is_incompatible() {
        let a = cols(&[("id","int4")]);
        let b = cols(&[("uid","int4")]);
        assert!(!or_replace_compatible(&a, &b));
    }

    #[test]
    fn dropping_a_column_is_incompatible() {
        let a = cols(&[("id","int4"), ("email","text")]);
        let b = cols(&[("id","int4")]);
        assert!(!or_replace_compatible(&a, &b));
    }

    #[test]
    fn reordering_is_incompatible() {
        let a = cols(&[("id","int4"), ("email","text")]);
        let b = cols(&[("email","text"), ("id","int4")]);
        assert!(!or_replace_compatible(&a, &b));
    }

    #[test]
    fn type_change_is_incompatible() {
        let a = cols(&[("id","int4")]);
        let b = cols(&[("id","int8")]);
        assert!(!or_replace_compatible(&a, &b));
    }

    fn cols(specs: &[(&str, &str)]) -> Vec<(ColumnName, ColumnType)> {
        specs.iter().map(|(n, t)| (ColumnName::new(n).unwrap(), ColumnType::parse(t).unwrap())).collect()
    }
}
```

The compatibility predicate compares **resolved column types**, not just names. v0.1 already has a strongly-typed `ColumnType` enum in `crates/pgevolve-core/src/ir/column_type.rs` (used by table columns). `ViewColumn` as defined in Task 1 carries only `name` + `comment` — extend it to carry the `ColumnType` too.

**Decision for this task:** extend `ViewColumn` with `column_type: ColumnType`. Both sides populate it the same way they populate table-column types — from `pg_attribute.atttypid` + `atttypmod` resolved through `format_type` and then parsed by the existing `ColumnType::parse` (or whatever function table columns already use). Update Task 1's `ViewColumn` definition, Task 3's parser+fixture-expected-JSON (since shadow pass fills this, not the static parser — parser sets a sentinel `ColumnType::Unresolved` value; shadow pass resolves it), Task 4's shadow pass (one extra column read in the same query), and Task 6's catalog SELECT in `SELECT_VIEW_COLUMNS` (add `format_type(a.atttypid, a.atttypmod)`).

(This is a back-edit to Tasks 1, 3, 4, and 6. Make the edits, run their tests, then proceed. If `ColumnType` lacks an `Unresolved` variant, add one — it's a parser-internal sentinel, never serialized to the catalog or to the plan, only present on provisional source-side records between parse and shadow-load.)

- [ ] **Step 7.3: Implement the predicate**

```rust
pub(crate) fn or_replace_compatible(
    catalog: &[(ColumnName, ColumnType)],
    source:  &[(ColumnName, ColumnType)],
) -> bool {
    if source.len() < catalog.len() { return false; }
    for (i, (name, ty)) in catalog.iter().enumerate() {
        let (s_name, s_ty) = &source[i];
        if name != s_name || ty != s_ty { return false; }
    }
    true
}
```

- [ ] **Step 7.4: Write `diff_views` and `diff_materialized_views`**

```rust
pub fn diff_views(catalog: &[View], source: &[View], out: &mut Vec<Change>) {
    use std::collections::BTreeMap;
    let cat: BTreeMap<_, _> = catalog.iter().map(|v| ((&v.schema, &v.name), v)).collect();
    let src: BTreeMap<_, _> = source.iter().map(|v| ((&v.schema, &v.name), v)).collect();

    for key in cat.keys().chain(src.keys()).collect::<std::collections::BTreeSet<_>>() {
        match (cat.get(key), src.get(key)) {
            (None, Some(s)) => out.push(Change::ViewChange(ViewChange::Create((*s).clone()))),
            (Some(c), None) => out.push(Change::ViewChange(ViewChange::Drop { schema: c.schema.clone(), name: c.name.clone() })),
            (Some(c), Some(s)) => {
                if c.body_canonical != s.body_canonical {
                    let cat_cols: Vec<_> = c.columns.iter().map(|col| (col.name.clone(), col.column_type.clone())).collect();
                    let src_cols: Vec<_> = s.columns.iter().map(|col| (col.name.clone(), col.column_type.clone())).collect();
                    let compatible = or_replace_compatible(&cat_cols, &src_cols);
                    out.push(Change::ViewChange(ViewChange::ReplaceBody {
                        source: (*s).clone(), catalog: (*c).clone(), compatible,
                    }));
                }
                if c.security_barrier != s.security_barrier || c.security_invoker != s.security_invoker {
                    out.push(Change::ViewChange(ViewChange::SetReloption {
                        schema: c.schema.clone(), name: c.name.clone(),
                        security_barrier: s.security_barrier, security_invoker: s.security_invoker,
                    }));
                }
                if c.comment != s.comment {
                    out.push(Change::ViewChange(ViewChange::SetComment {
                        schema: c.schema.clone(), name: c.name.clone(), comment: s.comment.clone(),
                    }));
                }
                diff_view_column_comments(c, s, out);
            }
            (None, None) => unreachable!(),
        }
    }
}
```

`diff_materialized_views` is structurally identical except no `SetReloption` branch and no `compatible` flag.

- [ ] **Step 7.5: Write fixture-driven diff tests**

Add 8 unit-test cases in `crates/pgevolve-core/src/diff/views.rs`'s `tests` module — one per row in §6.1 of the spec. Use the fixture builders already present in v0.1 tests (`tests/diff/` likely has helpers).

- [ ] **Step 7.6: Run the tests**

```bash
cargo test -p pgevolve-core --lib diff::views
```

Expected: all view-diff tests pass.

- [ ] **Step 7.7: Mark destructiveness**

In `crates/pgevolve-core/src/diff/destructiveness.rs`, extend the `is_destructive` match: `ViewChange::Drop` → true; everything else (including `MvChange::Drop` and both `ReplaceBody` variants) → false.

Add a unit test:
```rust
#[test]
fn view_drop_is_destructive_but_mv_drop_is_not() {
    let v = ViewChange::Drop { schema: SchemaName::new("app").unwrap(), name: ObjectName::new("v").unwrap() };
    let mv = MvChange::Drop { schema: SchemaName::new("app").unwrap(), name: ObjectName::new("mv").unwrap() };
    assert!(is_destructive(&Change::ViewChange(v)));
    assert!(!is_destructive(&Change::MvChange(mv)));
}
```

- [ ] **Step 7.8: Commit**

```bash
git add crates/pgevolve-core/src/diff/ crates/pgevolve-core/src/ir/view.rs
git commit -m "feat(diff): view and MV change kinds + OR-REPLACE predicate

Adds ViewChange and MvChange variants and the column-list-superset
predicate that drives CREATE OR REPLACE vs drop+create. Extends
ViewColumn with column_type so the predicate can compare types."
```

---

## Task 8: Planner — new step kinds + SQL emission

**Files:**
- Modify: `crates/pgevolve-core/src/plan/raw_step.rs` (new `StepKind` variants)
- Create: `crates/pgevolve-core/src/plan/emit/views.rs` (SQL strings)
- Modify: `crates/pgevolve-core/src/plan/emit/mod.rs` (route view/MV changes)
- Modify: `crates/pgevolve-core/src/plan/serialize.rs` (step directive formatting accepts new kinds)
- Test: `crates/pgevolve-core/src/plan/emit/views.rs` (inline unit tests)
- Test: extend the existing determinism harness with view fixtures

- [ ] **Step 8.1: Extend `StepKind`**

In `crates/pgevolve-core/src/plan/raw_step.rs`:

```rust
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    // ...existing variants...
    CreateView { or_replace: bool },
    DropView,
    CreateMaterializedView,
    DropMaterializedView,
    RefreshMaterializedView { concurrently: bool },
    AlterViewSetReloption,
    CommentOnView,
}
```

The plan-format directive serializer (`serialize.rs`) flattens these to wire strings:
- `kind=create_view` (with `or_replace=true|false` as a directive field)
- `kind=drop_view`
- `kind=create_materialized_view`
- `kind=drop_materialized_view`
- `kind=refresh_materialized_view concurrently=true|false`
- `kind=alter_view_set_reloption`
- `kind=comment_on_view`

Update the directive serializer to emit `or_replace` and `concurrently` payload fields alongside the existing `step=`, `kind=`, `destructive=`, `intent_id=`, `targets=`.

Add unit tests asserting that each `StepKind` variant round-trips through the directive serializer.

- [ ] **Step 8.2: SQL emission**

`crates/pgevolve-core/src/plan/emit/views.rs`:

```rust
pub(crate) fn emit_create_view(v: &View, or_replace: bool) -> String {
    let mut sql = String::with_capacity(v.body_canonical.as_str().len() + 64);
    sql.push_str(if or_replace { "CREATE OR REPLACE VIEW " } else { "CREATE VIEW " });
    sql.push_str(&qualified(&v.schema, &v.name));
    if !v.columns.is_empty() {
        // Aliased column list. Only emit when names differ from what pg_get_viewdef
        // would derive — but the safest approach is to always emit when columns are
        // explicitly named in source. Match v0.1's "emit it if it's in the IR" rule.
        sql.push_str(" (");
        for (i, c) in v.columns.iter().enumerate() {
            if i > 0 { sql.push_str(", "); }
            sql.push_str(c.name.as_str());
        }
        sql.push(')');
    }
    let opts = view_with_clause(v);
    if let Some(opts) = opts {
        sql.push_str(" WITH (");
        sql.push_str(&opts);
        sql.push(')');
    }
    sql.push_str(" AS\n");
    sql.push_str(v.body_canonical.as_str().trim_end());
    if !sql.ends_with(';') { sql.push(';'); }
    sql
}

fn view_with_clause(v: &View) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = v.security_barrier { parts.push(format!("security_barrier = {b}")); }
    if let Some(i) = v.security_invoker { parts.push(format!("security_invoker = {i}")); }
    if parts.is_empty() { None } else { Some(parts.join(", ")) }
}

pub(crate) fn emit_drop_view(schema: &SchemaName, name: &ObjectName) -> String {
    format!("DROP VIEW {};", qualified(schema, name))
}

pub(crate) fn emit_create_materialized_view(mv: &MaterializedView) -> String {
    let mut sql = String::new();
    sql.push_str("CREATE MATERIALIZED VIEW ");
    sql.push_str(&qualified(&mv.schema, &mv.name));
    if !mv.columns.is_empty() {
        sql.push_str(" (");
        for (i, c) in mv.columns.iter().enumerate() {
            if i > 0 { sql.push_str(", "); }
            sql.push_str(c.name.as_str());
        }
        sql.push(')');
    }
    sql.push_str(" AS\n");
    sql.push_str(mv.body_canonical.as_str().trim_end());
    if sql.ends_with(';') { sql.pop(); }
    sql.push_str("\nWITH NO DATA;");
    sql
}

pub(crate) fn emit_drop_materialized_view(schema: &SchemaName, name: &ObjectName) -> String {
    format!("DROP MATERIALIZED VIEW {};", qualified(schema, name))
}

pub(crate) fn emit_refresh_mv(schema: &SchemaName, name: &ObjectName, concurrently: bool) -> String {
    if concurrently {
        format!("REFRESH MATERIALIZED VIEW CONCURRENTLY {};", qualified(schema, name))
    } else {
        format!("REFRESH MATERIALIZED VIEW {};", qualified(schema, name))
    }
}

pub(crate) fn emit_alter_view_set_reloption(
    schema: &SchemaName,
    name: &ObjectName,
    security_barrier: Option<bool>,
    security_invoker: Option<bool>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = security_barrier { parts.push(format!("security_barrier = {b}")); }
    if let Some(i) = security_invoker { parts.push(format!("security_invoker = {i}")); }
    format!("ALTER VIEW {} SET ({});", qualified(schema, name), parts.join(", "))
}

fn qualified(s: &SchemaName, n: &ObjectName) -> String {
    format!("{}.{}", quote_ident_if_needed(s.as_str()), quote_ident_if_needed(n.as_str()))
}

fn quote_ident_if_needed(s: &str) -> String {
    // Reuse the v0.1 helper if it exists in `plan/sql.rs` or similar; otherwise inline.
    crate::plan::ident::quote_if_needed(s)
}
```

Add unit tests asserting each emit function produces the expected SQL string for a handful of inputs.

- [ ] **Step 8.3: Wire view/MV changes into the planner's emit pass**

In `crates/pgevolve-core/src/plan/emit/mod.rs`, the function that maps `Change` → `Vec<RawStep>` gets new arms:

```rust
Change::ViewChange(ViewChange::Create(v)) => vec![RawStep {
    kind: StepKind::CreateView { or_replace: false },
    sql: emit_create_view(v, false),
    targets: vec![qname(&v.schema, &v.name)],
    intent_id: None,
}],

Change::ViewChange(ViewChange::Drop { schema, name }) => vec![RawStep {
    kind: StepKind::DropView,
    sql: emit_drop_view(schema, name),
    targets: vec![qname(schema, name)],
    intent_id: Some(NEXT_INTENT_ID), // resolved by the intent-allocation pass
}],

Change::ViewChange(ViewChange::ReplaceBody { source, catalog, compatible: true }) => vec![RawStep {
    kind: StepKind::CreateView { or_replace: true },
    sql: emit_create_view(source, true),
    targets: vec![qname(&source.schema, &source.name)],
    intent_id: None,
}],

Change::ViewChange(ViewChange::ReplaceBody { source, catalog, compatible: false }) => vec![
    RawStep { kind: StepKind::DropView, sql: emit_drop_view(&catalog.schema, &catalog.name), targets: vec![qname(&catalog.schema, &catalog.name)], intent_id: None },
    RawStep { kind: StepKind::CreateView { or_replace: false }, sql: emit_create_view(source, false), targets: vec![qname(&source.schema, &source.name)], intent_id: None },
],

Change::ViewChange(ViewChange::SetReloption { schema, name, security_barrier, security_invoker }) => vec![RawStep {
    kind: StepKind::AlterViewSetReloption,
    sql: emit_alter_view_set_reloption(schema, name, *security_barrier, *security_invoker),
    targets: vec![qname(schema, name)],
    intent_id: None,
}],

Change::ViewChange(ViewChange::SetComment { schema, name, comment }) => vec![RawStep {
    kind: StepKind::CommentOnView,
    sql: emit_comment_on_view(schema, name, comment.as_deref()),
    targets: vec![qname(schema, name)],
    intent_id: None,
}],

Change::ViewChange(ViewChange::SetColumnComment { schema, name, column, comment }) => vec![RawStep {
    kind: StepKind::CommentOnView,
    sql: emit_comment_on_view_column(schema, name, column, comment.as_deref()),
    targets: vec![qname_col(schema, name, column)],
    intent_id: None,
}],

Change::MvChange(MvChange::Create(mv)) => vec![
    RawStep { kind: StepKind::CreateMaterializedView, sql: emit_create_materialized_view(mv), targets: vec![qname(&mv.schema, &mv.name)], intent_id: None },
    RawStep { kind: StepKind::RefreshMaterializedView { concurrently: false }, sql: emit_refresh_mv(&mv.schema, &mv.name, false), targets: vec![qname(&mv.schema, &mv.name)], intent_id: None },
],

Change::MvChange(MvChange::Drop { schema, name }) => vec![RawStep {
    kind: StepKind::DropMaterializedView,
    sql: emit_drop_materialized_view(schema, name),
    targets: vec![qname(schema, name)],
    intent_id: None,
}],

Change::MvChange(MvChange::ReplaceBody { source, catalog }) => vec![
    RawStep { kind: StepKind::DropMaterializedView, sql: emit_drop_materialized_view(&catalog.schema, &catalog.name), targets: vec![qname(&catalog.schema, &catalog.name)], intent_id: None },
    RawStep { kind: StepKind::CreateMaterializedView, sql: emit_create_materialized_view(source), targets: vec![qname(&source.schema, &source.name)], intent_id: None },
    RawStep { kind: StepKind::RefreshMaterializedView { concurrently: false }, sql: emit_refresh_mv(&source.schema, &source.name, false), targets: vec![qname(&source.schema, &source.name)], intent_id: None },
],

Change::MvChange(MvChange::SetComment { schema, name, comment }) => vec![RawStep {
    kind: StepKind::CommentOnView, // reuses kind=comment_on_view; target qname makes the MV vs view distinction
    sql: emit_comment_on_materialized_view(schema, name, comment.as_deref()),
    targets: vec![qname(schema, name)],
    intent_id: None,
}],

Change::MvChange(MvChange::SetColumnComment { schema, name, column, comment }) => vec![RawStep {
    kind: StepKind::CommentOnView,
    sql: emit_comment_on_mv_column(schema, name, column, comment.as_deref()),
    targets: vec![qname_col(schema, name, column)],
    intent_id: None,
}],
```

Helper functions `emit_comment_on_view`, `emit_comment_on_view_column`, `emit_comment_on_materialized_view`, `emit_comment_on_mv_column` live in `plan/emit/views.rs` and follow the same trivial pattern as their counterparts in v0.1's `plan/emit/comments.rs` (or wherever existing COMMENT emission lives). Helpers `qname` and `qname_col` are already used elsewhere in `plan/emit/`.

The `concurrently` flag on `RefreshMaterializedView` is provisionally `false` here — Task 9's online-rewrite pass flips it to `true` when conditions allow.

`intent_id: None` initially even for `DropView`; the existing intent-allocation pass (it's how v0.1 handles destructive steps) walks the `RawStep`s after this emit and assigns IDs to every step where `destructiveness::is_destructive` says yes. Confirm by reading `plan/grouping.rs` and `plan/serialize.rs`. If `DropView` doesn't naturally land in the destructiveness pass because of a missing match arm there, add the arm.

- [ ] **Step 8.4: Determinism test**

Extend `crates/pgevolve-core/tests/determinism.rs` with a fixture containing two views and one MV, asserting that running the full pipeline N times produces byte-identical `plan.sql`.

- [ ] **Step 8.5: Run all plan/serialize/determinism tests**

```bash
cargo test -p pgevolve-core --lib plan
cargo test -p pgevolve-core --test determinism
```

Expected: existing tests still green + new view/MV cases green.

- [ ] **Step 8.6: Commit**

```bash
git add crates/pgevolve-core/src/plan/ crates/pgevolve-core/tests/determinism.rs
git commit -m "feat(plan): view/MV step kinds + SQL emission

Adds CreateView, DropView, CreateMaterializedView, DropMaterializedView,
RefreshMaterializedView, AlterViewSetReloption, CommentOnView step
kinds. Each maps to one SQL statement; determinism harness covers the
view + MV pipeline."
```

---

## Task 9: Planner — dependent-view recreation + online-rewrite policy

**Files:**
- Modify: `crates/pgevolve-core/src/plan/graph.rs` (extend the dependency graph with view→column edges)
- Create: `crates/pgevolve-core/src/plan/recreate_views.rs` (transitive recreation walk)
- Modify: `crates/pgevolve-core/src/plan/policy.rs` (add the two new online-rewrite toggles)
- Modify: `crates/pgevolve-core/src/plan/rewrite/mod.rs` (REFRESH CONCURRENTLY rewrite)
- Create: `crates/pgevolve-core/src/plan/rewrite/refresh_mv_concurrently.rs`
- Test: inline + extend conformance fixtures (Task 12)

- [ ] **Step 9.1: Plumb `body_dependencies` into the dependency graph**

In `crates/pgevolve-core/src/plan/graph.rs`, the graph builder already knows about table→column nodes. Add a builder pass that, for each view/MV in the catalog IR, walks its `body_dependencies` and adds an edge `Node::View(qname) → Node::Column(qname, col)` (or `Node::Function`, `Node::Type`, `Node::Sequence` for the other DepKind variants).

Unit test:

```rust
#[test]
fn view_dep_edges_appear_in_graph() {
    let view = view_with_deps("app", "v", &[
        ("app", "users", Some("email")),
    ]);
    let g = build_graph(&catalog_with(view), &Default::default());
    let edges: Vec<_> = g.edges_from(&Node::View("app.v".into())).collect();
    assert!(edges.iter().any(|e| matches!(e.to(), Node::Column { schema, table, column } if schema == "app" && table == "users" && column == "email")));
}
```

- [ ] **Step 9.2: Transitive recreation walk**

`crates/pgevolve-core/src/plan/recreate_views.rs`:

```rust
//! Identifies every catalog view (transitively) affected by upstream
//! changes in the diff, and emits drop+recreate steps for them.

use crate::diff::change::{Change, ColumnChange, MvChange, TableChange, ViewChange};
use crate::ir::{CatalogIr, View};

pub(crate) fn extend_with_dependent_recreations(
    changes: &mut Vec<Change>,
    catalog: &CatalogIr,
) {
    let triggers = collect_upstream_triggers(changes);
    let affected = walk_transitive(&triggers, catalog);
    for (schema, name) in affected {
        let cat = catalog.find_view(&schema, &name).expect("affected view exists in catalog");
        changes.push(Change::ViewChange(ViewChange::ReplaceBody {
            source: cat.clone(), // body unchanged — we're recreating because of upstream churn
            catalog: cat.clone(),
            compatible: false,
        }));
    }
}

fn collect_upstream_triggers(changes: &[Change]) -> Vec<DepTrigger> {
    // Walks the diff. Returns the set of (schema, object, optional_column) tuples
    // that, if a view depends on them, force the view to be recreated.
    //
    // Examples:
    //   ColumnChange::Drop { table, column } → (table.schema, table.name, Some(column))
    //   ColumnChange::Rename { table, old, new } → (table.schema, table.name, Some(old))
    //   ColumnChange::SetType { table, column, .. } → (table.schema, table.name, Some(column))
    //   TableChange::Drop { schema, name } → (schema, name, None) — any column dep matches
    //   ViewChange::Drop { schema, name } → (schema, name, None) — for dependent views
    //   ViewChange::ReplaceBody { compatible: false, .. } → (schema, name, None)
    //
    // Compatible body replacements DO NOT trigger dep recreation — that's the
    // whole point of CREATE OR REPLACE.
    todo!() // expand into explicit matches in implementation
}
```

The `todo!()` is a placeholder for the engineer — the spec's §6.4 lists the trigger conditions explicitly. Implement them one match arm at a time, with a unit test per trigger.

- [ ] **Step 9.3: Online rewrite — REFRESH CONCURRENTLY**

`crates/pgevolve-core/src/plan/rewrite/refresh_mv_concurrently.rs`:

```rust
//! Online rewrite: when planner strategy is "online", upgrade plain
//! REFRESH MATERIALIZED VIEW steps to REFRESH CONCURRENTLY for any MV
//! that has at least one unique index in the catalog IR.

use crate::ir::CatalogIr;
use crate::plan::policy::OnlineRewriteFlags;
use crate::plan::raw_step::{RawStep, StepKind};

pub(crate) fn rewrite(
    steps: &mut [RawStep],
    catalog: &CatalogIr,
    flags: &OnlineRewriteFlags,
    lint_findings: &mut Vec<crate::lint::Finding>,
) {
    if !flags.refresh_mv_concurrently { return; }
    for step in steps.iter_mut() {
        let StepKind::RefreshMaterializedView { concurrently } = &mut step.kind else { continue };
        if *concurrently { continue; }
        let target = step.targets.first().expect("refresh step must have a target");
        let has_unique = mv_has_unique_index(catalog, target);
        if has_unique {
            *concurrently = true;
            // Re-emit the SQL string so plan.sql matches the directive.
            step.sql = crate::plan::emit::views::emit_refresh_mv_from_qname(target, true);
        } else {
            lint_findings.push(crate::lint::Finding::mv_no_unique_index(target.clone()));
        }
    }
}

fn mv_has_unique_index(catalog: &CatalogIr, qname: &Qname) -> bool {
    catalog.indexes.iter().any(|ix| {
        matches!(&ix.on, IndexParent::Mv { schema, name } if qname.matches(schema, name))
            && ix.unique
    })
}
```

Note: the rewrite only takes effect under `strategy = "online"` — the existing rewrite framework gates on strategy at the top level; verify by reading `plan/rewrite/mod.rs`.

- [ ] **Step 9.4: Add the two new toggles to policy**

In `crates/pgevolve-core/src/plan/policy.rs`, extend `OnlineRewriteFlags`:

```rust
pub struct OnlineRewriteFlags {
    // ...existing fields...
    pub refresh_mv_concurrently: bool,
    pub view_drop_create_dependents: bool,
}

impl Default for OnlineRewriteFlags {
    fn default() -> Self {
        Self {
            // ...existing defaults...
            refresh_mv_concurrently: true,
            view_drop_create_dependents: true,
        }
    }
}
```

Wire `view_drop_create_dependents = false` into Task 9.2's `extend_with_dependent_recreations` — when false and the diff contains a trigger that would recreate dependents, raise a planner error naming each affected view.

- [ ] **Step 9.5: Config layer**

In `crates/pgevolve/src/config.rs`, the existing `[planner.online_rewrites]` parser gains the two new keys. Update the default `init` template (`crates/pgevolve/src/commands/init.rs`) to include them. Add a config-roundtrip unit test.

- [ ] **Step 9.6: Run plan tests**

```bash
cargo test -p pgevolve-core --lib plan
```

Expected: green, including new dependent-recreation tests and REFRESH CONCURRENTLY tests.

- [ ] **Step 9.7: Commit**

```bash
git add crates/pgevolve-core/src/plan/ crates/pgevolve/src/config.rs crates/pgevolve/src/commands/init.rs
git commit -m "feat(plan): dependent-view recreation + REFRESH CONCURRENTLY rewrite

Walks the dep graph for views transitively affected by upstream
changes and emits explicit drop+recreate steps for each. Online
strategy upgrades plain REFRESH to REFRESH CONCURRENTLY when the MV
has a unique index, surfaces lint warning otherwise."
```

---

## Task 10: `intent.toml` — `[[step_override]]` table

**Files:**
- Modify: `crates/pgevolve-core/src/plan/serialize.rs` (read/write `[[step_override]]`)
- Modify: `crates/pgevolve-core/src/plan/deserialize.rs` (parse `[[step_override]]`)
- Modify: `crates/pgevolve-core/src/plan/plan.rs` (add `step_overrides: Vec<StepOverride>`)
- Modify: `crates/pgevolve/src/executor/execute.rs` (honor `suppress = true` for `refresh_materialized_view`)
- Test: `crates/pgevolve-core/src/plan/serialize.rs` (roundtrip test); `crates/pgevolve/tests/executor_smoke.rs` (suppression test)

- [ ] **Step 10.1: Define the type**

```rust
// crates/pgevolve-core/src/plan/plan.rs
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StepOverride {
    pub kind: StepKindTag,           // re-uses the snake-case wire form
    pub target: String,              // qname string
    #[serde(default)]
    pub suppress: bool,
}
```

`StepKindTag` is the wire-form enum (the same `kind=create_view` strings used in plan.sql).

- [ ] **Step 10.2: Read/write `[[step_override]]`**

Extend the `IntentDoc` struct in `serialize.rs`:

```rust
#[derive(Serialize)]
struct IntentDoc<'a> {
    plan_id: &'a str,
    #[serde(rename = "intent")]
    intents: Vec<IntentRow<'a>>,
    #[serde(rename = "step_override", skip_serializing_if = "Vec::is_empty")]
    step_overrides: Vec<StepOverrideRow<'a>>,
}

#[derive(Serialize)]
struct StepOverrideRow<'a> {
    kind: &'a str,
    target: &'a str,
    suppress: bool,
}
```

Add the symmetric `IntentDocRead` struct on the deserialize side. Write a roundtrip test in `serialize.rs`'s test module.

- [ ] **Step 10.3: Executor honors suppression**

In `crates/pgevolve/src/executor/execute.rs`, before running each step, check if a `[[step_override]]` row matches (kind + target). If `suppress = true`, skip the step and record it in the audit log as "suppressed by intent". Add an integration test:

```rust
#[test]
#[cfg_attr(not(feature = "docker-tests"), ignore)]
fn refresh_mv_suppressed_by_step_override() {
    // 1. Build a plan with a CREATE MATERIALIZED VIEW + REFRESH step.
    // 2. Edit intent.toml to add a [[step_override]] for the refresh.
    // 3. Apply.
    // 4. Assert the MV exists but has zero rows (refresh was skipped).
}
```

- [ ] **Step 10.4: Run tests**

```bash
cargo test -p pgevolve-core --lib plan::serialize plan::deserialize
cargo test -p pgevolve --test executor_smoke --features docker-tests -- refresh_mv_suppressed
```

Expected: PASS.

- [ ] **Step 10.5: Commit**

```bash
git add crates/pgevolve-core/src/plan/ crates/pgevolve/src/executor/
git commit -m "feat(plan): [[step_override]] table in intent.toml

Non-destructive per-step modifier. First consumer: suppress the
auto-emitted REFRESH MATERIALIZED VIEW step (the WITH-NO-DATA use
case). Executor reads the override at apply time and records skipped
steps in the audit log."
```

---

## Task 11: Lints

**Files:**
- Modify: `crates/pgevolve-core/src/lint/universal.rs` (add the three rules)
- Modify: `crates/pgevolve-core/src/lint/finding.rs` (add finding kinds)
- Test: `crates/pgevolve-core/src/lint/universal.rs` (one fixture per rule)

- [ ] **Step 11.1: `view-shadows-table`**

Walk the source IR; flag when `(schema, name)` collides between `tables` and `views`/`materialized_views`. Severity: error.

```rust
#[test]
fn view_shadows_table_lints_when_name_collides() {
    let ir = source_with_table_and_view("app", "users");
    let findings = lint_universal(&ir);
    assert!(findings.iter().any(|f| f.id == "view-shadows-table"));
}
```

- [ ] **Step 11.2: `mv-no-unique-index`**

Severity: warning. Walk the catalog IR's MVs; for each, check whether any index in `catalog.indexes` has `unique = true` and `on = IndexParent::Mv { matching }`. If not, emit a finding pointing at the MV. (This lint only matters under `strategy = "online"` — but it's worth emitting always so users see it early.)

- [ ] **Step 11.3: `view-body-references-unmanaged-schema`**

Walk each view's `body_dependencies`; if any `target.schema` is not in `[managed].schemas` and not a built-in (`pg_catalog`, `information_schema`), emit a finding. Severity: warning.

- [ ] **Step 11.4: Run lint tests**

```bash
cargo test -p pgevolve-core --lib lint
```

Expected: green.

- [ ] **Step 11.5: Commit**

```bash
git add crates/pgevolve-core/src/lint/
git commit -m "feat(lint): view-shadows-table, mv-no-unique-index, view-body-references-unmanaged-schema

Three new universal lint rules for the views sub-spec."
```

---

## Task 12: Conformance fixtures

**Files:**
- Create: 15 fixture directories under `crates/pgevolve-conformance/tests/cases/views/` and `crates/pgevolve-conformance/tests/cases/matviews/`. One fixture per bullet in spec §10.1.

Each fixture follows the existing structure (see `tests/cases/tables/add-column-nullable/` for the canonical example): a `fixture.toml`, a `before.sql`, an `after.sql`, and optionally a `plan.sql.golden` (regenerable via `cargo xtask bless --conformance`).

- [ ] **Step 12.1: Author the 15 fixtures**

For each entry in spec §10.1, write the three (or four) files. Sample for `views/create-simple`:

`fixture.toml`:
```toml
[meta]
title     = "CREATE VIEW — simple"
spec_refs = ["views.create"]

[pg]
min = 14
max = 17

[expect.diff]
contains = ["app.users_summary"]

[expect.plan]
steps  = 1
golden = true

[expect.apply]
succeeds             = true
post_apply_equals_to = "after.sql"
```

`before.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.users (id int PRIMARY KEY, email text);
```

`after.sql`:
```sql
CREATE SCHEMA app;
CREATE TABLE app.users (id int PRIMARY KEY, email text);
CREATE VIEW app.users_summary AS SELECT id, email FROM app.users;
```

Repeat for each fixture in §10.1. The fixture-loader picks them up automatically; no `mod.rs` edits required.

- [ ] **Step 12.2: Regenerate plan goldens**

```bash
cargo xtask bless --conformance
```

This walks every new fixture, runs the diff+plan pipeline, and writes `plan.sql.golden`. Commit the resulting goldens.

- [ ] **Step 12.3: Run the conformance suite**

```bash
cargo test -p pgevolve-conformance --features docker-tests
```

Expected: all 15 new fixtures pass all 4 layers (diff invariant, plan structural invariant, plan.sql golden, apply roundtrip).

- [ ] **Step 12.4: Commit**

```bash
git add crates/pgevolve-conformance/tests/cases/views/ crates/pgevolve-conformance/tests/cases/matviews/
git commit -m "test(conformance): fixtures for views and matviews

15 deterministic fixtures across the four-layer assertion model.
Goldens regenerable via cargo xtask bless --conformance."
```

---

## Task 13: Property tests (nightly) + documentation

**Files:**
- Modify: `crates/pgevolve-testkit/src/ir_generator.rs` (add `arb_view_body`, `arb_view_dependency_graph`)
- Modify: `crates/pgevolve-core/tests/property_tests.rs` (new property tests, `#[ignore]`)
- Modify: `docs/spec/objects.md`, `docs/spec/lint-and-layout.md`, `docs/spec/cli.md`
- Modify: `docs/user/plan-format.md`, `docs/user/cookbook.md`
- Modify: `docs/system/planner.md`, `docs/system/ir.md`
- Modify: `README.md` (phase progress table — mention v0.2 views landing)

- [ ] **Step 13.1: `arb_view_body` generator**

Implement a proptest strategy that, given a generated table corpus, produces a syntactically valid SELECT body referencing columns from those tables. Constraint: only generate bodies pg_query will parse; no DDL inside.

```rust
pub fn arb_view_body(tables: &[Table]) -> impl Strategy<Value = String> { /* ... */ }
```

Property:
```rust
proptest! {
    #[test]
    #[ignore]
    fn view_canonicalization_is_idempotent(tables in arb_tables(1..4), body_seed in 0u64..1000) {
        let body = arb_view_body_from_seed(&tables, body_seed);
        let canonical1 = run_through_shadow(&tables, &body);
        let canonical2 = run_through_shadow(&tables, &canonical1);
        prop_assert_eq!(canonical1, canonical2);
    }
}
```

- [ ] **Step 13.2: `arb_view_dependency_graph` generator**

```rust
pub fn arb_view_dependency_graph(depth: usize, fanout: usize) -> impl Strategy<Value = ViewDependencyGraph> { /* ... */ }

proptest! {
    #[test]
    #[ignore]
    fn column_rename_recreates_exactly_dependent_views(
        graph in arb_view_dependency_graph(2..4, 1..3),
        rename_target in any::<usize>(),
    ) {
        let plan = compute_plan_for_column_rename(&graph, rename_target);
        let expected_recreations = graph.transitive_dependents_of_column(rename_target);
        let actual_recreations = plan.view_recreations();
        prop_assert_eq!(actual_recreations.into_iter().collect::<BTreeSet<_>>(), expected_recreations.into_iter().collect::<BTreeSet<_>>());
        prop_assert!(plan.is_topologically_valid());
    }
}
```

- [ ] **Step 13.3: Documentation updates**

For each file in spec §12, make the documented edits. Concretely:

- `docs/spec/objects.md`:
  - VIEW row: change status from 📋 to ✅; update the Notes to describe the canonicalization model.
  - MATERIALIZED VIEW row: same.
  - Add two new rows: `security_barrier` reloption, `security_invoker` reloption (PG 15+).
  - Keep `CREATE VIEW ... WITH CHECK OPTION` and recursive views at 🔮 (no change).
- `docs/spec/lint-and-layout.md`: add the three new lint rules to the universal-rules table.
- `docs/spec/cli.md`: document `refresh_mv_concurrently` and `view_drop_create_dependents` under `[planner.online_rewrites]`.
- `docs/user/plan-format.md`: document the seven new step kinds (Task 8.1) and the `[[step_override]]` table (Task 10).
- `docs/user/cookbook.md`: add a "Managing views" entry with a small example (CREATE VIEW + later compatible body change + later incompatible body change with dependent recreation).
- `docs/system/planner.md`: document the OR-REPLACE compatibility predicate and the dependent-recreation walk.
- `docs/system/ir.md`: document the `View`, `MaterializedView`, `ViewColumn`, `DepEdge`, `CanonicalViewBody` types.
- `README.md`: update the phase progress table or add a note that v0.2 views/MVs have landed.

- [ ] **Step 13.4: Run the entire suite**

```bash
cargo test --workspace --all-features
cargo test --workspace --all-features -- --ignored # nightly tests, locally for sanity
```

Expected: all green.

- [ ] **Step 13.5: Commit**

```bash
git add crates/pgevolve-testkit/src/ir_generator.rs crates/pgevolve-core/tests/property_tests.rs docs/ README.md
git commit -m "feat(testkit, docs): view property tests + spec updates

Nightly property tests for canonicalization idempotency and
dependency-graph recreation. Doc updates flip view/MV statuses to ✅
and document the new lint rules, step kinds, and [[step_override]]
shape."
```

---

## Self-Review Checklist

After implementing all tasks, run through this checklist:

1. **Spec §2 (Scope) coverage:** every "In scope" row maps to a task. ✓
2. **Spec §3 (Key design decisions) coverage:** every decision row has at least one task that implements it. Verify by grepping the plan for each decision name.
3. **Spec §4 (IR) coverage:** all five types from §4 are defined in Task 1; `IndexParent::Mv` in Task 2. ✓
4. **Spec §5 (Pipelines) coverage:** source pipeline (Tasks 3+4), catalog pipeline (Task 6), caching (Task 5). ✓
5. **Spec §6 (Diff + planner) coverage:** change kinds (Task 7), OR-REPLACE predicate (Task 7), step kinds (Task 8), dependent recreation (Task 9), online rewrites (Task 9). ✓
6. **Spec §7 (Lints) coverage:** Task 11 covers all three rules. ✓
7. **Spec §8 (intent.toml) coverage:** `[[step_override]]` in Task 10; `[[intent]]` for `drop_view` uses existing v0.1 machinery (just a new `kind` string). ✓
8. **Spec §9 (File layout):** parser change in Task 3 supports `schema/<schema>/views/*.sql` via the existing layout profiles; no new layout-profile code required.
9. **Spec §10 (Testing) coverage:** conformance (Task 12), tier-3 goldens (Task 6.7), property tests (Task 13). ✓
10. **Spec §11 (Edge cases):** each bullet either falls out of the design naturally or is covered by a conformance fixture in Task 12. Spot check:
    - "View body references an extension function" → Task 7 destructiveness/error machinery + Task 11's `view-body-references-unmanaged-schema` lint.
    - "MV with WITH NO DATA" → Task 10's `[[step_override]]`.
    - "View that selects *" → Task 4's shadow pass canonicalizes via pg_get_viewdef which expands *.
11. **Spec §12 (Documentation) coverage:** Task 13.3. ✓
12. **No placeholders left in the plan:** grep for "TODO", "TBD", "implement later" — only acceptable occurrence is the one annotated `todo!()` in Task 9.2 which is explicitly called out and accompanied by exhaustive trigger-condition spec references.
13. **Type consistency:** `ViewColumn` carries `type_signature` from Task 7's back-edit onward; `IndexParent` is the enum from Task 2; `StepKind` variants spelled identically wherever they appear (Tasks 8, 10, 12).
14. **Test-first discipline:** every task starts with a failing test (or fixture) before introducing implementation.

---
