# T13: --shadow-validate View Body Cross-Check Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `cross_check` in `shadow/validate.rs` to actually verify view/MV body canonicalization and body_dependencies against a live shadow Postgres.

**Architecture:** Boot a shadow PG, apply the source catalog via `render_catalog` (extended to emit `CREATE VIEW`/`CREATE MATERIALIZED VIEW`), then query `pg_get_viewdef` and `pg_depend` to cross-check body canonical text and dependency edges. Mismatches warn by default; `--shadow-strict` promotes to errors.

**Tech Stack:** Rust, tokio-postgres, pgevolve-core (render, parse, IR, plan/edges), testcontainers for Docker-gated tests.

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| Modify | `crates/pgevolve-core/src/render/mod.rs` | Extend `render_catalog` to emit views and MVs after sequences |
| Create | `crates/pgevolve-core/src/render/view.rs` | `render_view` and `render_materialized_view` functions |
| Modify | `crates/pgevolve/src/shadow/validate.rs` | Replace stub `cross_check` with view/MV body cross-checks |
| Create | `crates/pgevolve/tests/shadow_validate_views.rs` | Docker-gated happy-path cross-check test |

---

### Task 1: Create `render/view.rs` with `render_view` and `render_materialized_view`

**Files:**
- Create: `crates/pgevolve-core/src/render/view.rs`

- [ ] **Step 1: Write the failing unit tests in a new file**

Create `crates/pgevolve-core/src/render/view.rs` with tests first:

```rust
//! View and materialized-view renderer.
//!
//! Produces `CREATE VIEW` and `CREATE MATERIALIZED VIEW` statements for use
//! in `render_catalog` (shadow-validate path) and `pgevolve dump`.

use crate::ir::view::{MaterializedView, View};

/// Render a `View` as a `CREATE VIEW ... AS <body>;` SQL string.
///
/// Intentionally omits `OR REPLACE` — this renderer targets a fresh shadow
/// DB (apply path), so the simpler `CREATE VIEW` form is correct.
#[must_use]
pub fn render_view(v: &View) -> String {
    use crate::plan::rewrite::views as emit;
    emit::emit_create_view(v, false)
}

/// Render a `MaterializedView` as a `CREATE MATERIALIZED VIEW ... AS <body> WITH NO DATA;` string.
#[must_use]
pub fn render_materialized_view(mv: &MaterializedView) -> String {
    use crate::plan::rewrite::views as emit;
    emit::emit_create_materialized_view(mv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column_type::ColumnType;
    use crate::ir::view::{MaterializedView, View, ViewColumn};
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn body(sql: &str) -> NormalizedBody {
        NormalizedBody::from_sql(sql).unwrap()
    }

    #[test]
    fn render_view_produces_create_view() {
        let v = View {
            qname: qn("app", "active_users"),
            columns: vec![],
            body_canonical: body("SELECT id FROM app.users WHERE active = true"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
        };
        let sql = render_view(&v);
        assert!(sql.starts_with("CREATE VIEW"), "got: {sql}");
        assert!(sql.contains("app.active_users"), "got: {sql}");
        assert!(sql.contains("SELECT"), "got: {sql}");
        assert!(sql.ends_with(';'), "got: {sql}");
    }

    #[test]
    fn render_view_no_or_replace() {
        let v = View {
            qname: qn("app", "v"),
            columns: vec![],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
        };
        let sql = render_view(&v);
        assert!(
            !sql.contains("OR REPLACE"),
            "render_view should not emit OR REPLACE: {sql}"
        );
    }

    #[test]
    fn render_materialized_view_produces_create_materialized_view() {
        let mv = MaterializedView {
            qname: qn("app", "user_summary"),
            columns: vec![ViewColumn {
                name: id("cnt"),
                column_type: ColumnType::BigInt,
                comment: None,
            }],
            body_canonical: body("SELECT count(*) FROM app.users"),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
        };
        let sql = render_materialized_view(&mv);
        assert!(sql.starts_with("CREATE MATERIALIZED VIEW"), "got: {sql}");
        assert!(sql.contains("app.user_summary"), "got: {sql}");
        assert!(sql.contains("SELECT"), "got: {sql}");
        assert!(sql.ends_with(';'), "got: {sql}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (module not in lib yet)**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test -p pgevolve-core --lib render::view 2>&1 | tail -10
```

Expected: compile error — module `view` is private/not found in `render`.

- [ ] **Step 3: Check that `emit_create_materialized_view` is accessible**

```bash
grep -n "pub.*emit_create_materialized_view\|pub(crate).*emit_create_materialized_view" /Users/danieltoone/ws/pgevolve/crates/pgevolve-core/src/plan/rewrite/views.rs | head -5
```

If it says `pub(crate)` (not `pub`), note that `render/view.rs` is in the same crate, so `pub(crate)` is fine.

- [ ] **Step 4: Register the module in `render/mod.rs`**

Open `crates/pgevolve-core/src/render/mod.rs` and add `pub mod view;` to the existing module list:

Current section (lines 28-31):
```
pub mod index;
pub mod schema;
pub mod sequence;
pub mod table;
```

Change to:
```
pub mod index;
pub mod schema;
pub mod sequence;
pub mod table;
pub mod view;
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test -p pgevolve-core --lib render::view 2>&1 | tail -10
```

Expected: all 3 tests pass.

- [ ] **Step 6: Commit**

```bash
cd /Users/danieltoone/ws/pgevolve && git add crates/pgevolve-core/src/render/view.rs crates/pgevolve-core/src/render/mod.rs && git commit -m "feat(render): add render/view.rs with render_view and render_materialized_view"
```

---

### Task 2: Extend `render_catalog` to emit views and MVs

**Files:**
- Modify: `crates/pgevolve-core/src/render/mod.rs`

- [ ] **Step 1: Write the failing test first**

In `crates/pgevolve-core/src/render/mod.rs`, add to the `#[cfg(test)]` module at the bottom:

```rust
#[test]
fn views_rendered_after_sequences() {
    use crate::ir::view::{View};
    use crate::parse::normalize_body::NormalizedBody;

    let mut cat = Catalog::empty();
    cat.sequences.push(Sequence {
        qname: qn("app", "users_id_seq"),
        data_type: ColumnType::BigInt,
        start: 1,
        increment: 1,
        min_value: None,
        max_value: None,
        cache: 1,
        cycle: false,
        owned_by: None,
        comment: None,
    });
    cat.views.push(View {
        qname: qn("app", "active_users"),
        columns: vec![],
        body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
        body_dependencies: vec![],
        security_barrier: None,
        security_invoker: None,
        comment: None,
        raw_body: String::new(),
    });

    let rendered = render_catalog(&cat);
    let seq_pos = rendered.find("CREATE SEQUENCE").unwrap();
    let view_pos = rendered.find("CREATE VIEW").unwrap();
    assert!(seq_pos < view_pos, "views must come after sequences");
}

#[test]
fn materialized_views_rendered_in_catalog() {
    use crate::ir::view::MaterializedView;
    use crate::parse::normalize_body::NormalizedBody;

    let mut cat = Catalog::empty();
    cat.materialized_views.push(MaterializedView {
        qname: qn("app", "summary"),
        columns: vec![],
        body_canonical: NormalizedBody::from_sql("SELECT count(*) FROM app.users").unwrap(),
        body_dependencies: vec![],
        comment: None,
        raw_body: String::new(),
    });

    let rendered = render_catalog(&cat);
    assert!(
        rendered.contains("CREATE MATERIALIZED VIEW"),
        "expected CREATE MATERIALIZED VIEW: {rendered}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test -p pgevolve-core --lib "render::tests::views_rendered_after_sequences\|render::tests::materialized_views_rendered_in_catalog" 2>&1 | tail -10
```

Expected: FAIL — `render_catalog` doesn't emit views yet.

- [ ] **Step 3: Extend `render_catalog` to emit views and MVs**

In `crates/pgevolve-core/src/render/mod.rs`, after step 5 (sequences), add:

```rust
    // 6. Views (after sequences — views may reference sequence defaults).
    for v in &catalog.views {
        out.push_str(&view::render_view(v));
        out.push('\n');
    }
    if !catalog.views.is_empty() {
        out.push('\n');
    }

    // 7. Materialized views.
    for mv in &catalog.materialized_views {
        out.push_str(&view::render_materialized_view(mv));
        out.push('\n');
    }
```

Place this before the `// Strip trailing blank line.` block.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test -p pgevolve-core --lib render 2>&1 | tail -10
```

Expected: all render tests pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/danieltoone/ws/pgevolve && git add crates/pgevolve-core/src/render/mod.rs && git commit -m "feat(render): render_catalog now emits views and materialized views"
```

---

### Task 3: Extend `CrossCheckReport` and implement `cross_check`

**Files:**
- Modify: `crates/pgevolve/src/shadow/validate.rs`

This is the main task. We replace the stub with a working cross-check that:
1. Applies the source catalog to shadow via `render_catalog`.
2. For each view/MV, queries `pg_get_viewdef` and compares canonicalized body text.
3. For each view/MV, queries `pg_depend` and cross-checks `AstExtracted` body_dependencies.
4. Returns `CrossCheckReport` with mismatches; under `strict`, bails.

- [ ] **Step 1: Replace the entire file with the new implementation**

Write `crates/pgevolve/src/shadow/validate.rs`:

```rust
//! Cross-check the source IR against a live shadow Postgres.
//!
//! For each Structural edge in the source dep graph, the shadow's
//! `pg_depend` should report a corresponding entry. For each view/MV,
//! `pg_get_viewdef` is used to round-trip the body through Postgres and
//! compare the catalog-canonical form against the source-side `NormalizedBody`.

use std::collections::BTreeSet;

use anyhow::Result;
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId, build_create_graph};
use pgevolve_core::render::render_catalog;

use crate::shadow::ShadowBackend;

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// Summary of a cross-check run.
#[derive(Debug, Default)]
pub struct CrossCheckReport {
    /// Number of Structural edges examined.
    pub structural_edges_checked: usize,
    /// Body text mismatches: source canonical ≠ catalog canonical.
    pub canonical_mismatches: Vec<CanonicalMismatch>,
    /// Edges present in pg_depend but absent from source body_dependencies
    /// (filtered to AstExtracted — AstDeclared covers dynamic SQL intentionally).
    pub missing_ast_edges: Vec<MissingEdge>,
    /// Edges present in source body_dependencies (AstExtracted) but absent
    /// from pg_depend.
    pub extra_ast_edges: Vec<ExtraEdge>,
}

/// A view/MV whose body canonical text differs between source and pg_depend.
#[derive(Debug)]
pub struct CanonicalMismatch {
    pub view_qname: String,
    pub source_canonical: String,
    pub catalog_canonical: String,
}

/// An edge present in pg_depend but absent from AST-extracted body_dependencies.
#[derive(Debug)]
pub struct MissingEdge {
    pub view_qname: String,
    pub ref_schema: String,
    pub ref_name: String,
}

/// An AST-extracted edge present in source but absent from pg_depend.
#[derive(Debug)]
pub struct ExtraEdge {
    pub view_qname: String,
    pub dep_node: String,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the cross-check against a shadow backend.
///
/// If the source catalog contains no views or materialized views, this is a
/// fast no-op (counts structural edges only — identical to v0.1 behaviour).
///
/// When views/MVs are present:
/// 1. Boots shadow, applies source via `render_catalog`.
/// 2. Queries `pg_get_viewdef` for each view/MV; re-canonicalizes; compares.
/// 3. Queries `pg_depend` for each view/MV; cross-checks AstExtracted edges.
/// 4. Populates `canonical_mismatches`, `extra_ast_edges`, `missing_ast_edges`.
///
/// Under `strict`, returns `Err` if any mismatch list is non-empty.
pub async fn cross_check(
    backend: &dyn ShadowBackend,
    source: &Catalog,
    pg_major: u32,
    strict: bool,
) -> Result<CrossCheckReport> {
    let mut report = CrossCheckReport::default();

    // Always count structural edges (v0.1 compatibility).
    let graph = build_create_graph(source);
    for edge in graph.dep_edges() {
        if matches!(edge.source, DepSource::Structural) {
            report.structural_edges_checked += 1;
        }
    }

    // Only run view cross-checks when views or MVs are present.
    if source.views.is_empty() && source.materialized_views.is_empty() {
        return Ok(report);
    }

    // Boot shadow, apply source.
    let mut guard = backend.checkout(pg_major).await?;
    apply_source_to_shadow(guard.url(), source).await?;

    let (client, conn) = tokio_postgres::connect(guard.url(), tokio_postgres::NoTls).await?;
    tokio::spawn(conn);

    // Cross-check regular views.
    for view in &source.views {
        let qname = view.qname.to_string();
        let qname_sql = view.qname.render_sql();

        // 1. Body canonicalization check.
        let pg_body: String = client
            .query_one(
                &format!("SELECT pg_get_viewdef('{qname_sql}'::regclass, true)"),
                &[],
            )
            .await
            .map(|row| row.get::<_, String>(0))
            .map_err(|e| anyhow::anyhow!("pg_get_viewdef failed for {qname}: {e}"))?;

        match NormalizedBody::from_sql(&pg_body) {
            Ok(catalog_canonical) => {
                if catalog_canonical.canonical_text() != view.body_canonical.canonical_text() {
                    report.canonical_mismatches.push(CanonicalMismatch {
                        view_qname: qname.clone(),
                        source_canonical: view.body_canonical.canonical_text().to_string(),
                        catalog_canonical: catalog_canonical.canonical_text().to_string(),
                    });
                }
            }
            Err(e) => {
                report.canonical_mismatches.push(CanonicalMismatch {
                    view_qname: qname.clone(),
                    source_canonical: view.body_canonical.canonical_text().to_string(),
                    catalog_canonical: format!("<parse error: {e}>"),
                });
            }
        }

        // 2. body_dependencies vs pg_depend.
        check_dep_edges(
            &client,
            &qname,
            &qname_sql,
            &view.body_dependencies,
            &mut report,
        )
        .await?;
    }

    // Cross-check materialized views.
    for mv in &source.materialized_views {
        let qname = mv.qname.to_string();
        let qname_sql = mv.qname.render_sql();

        // 1. Body canonicalization check.
        let pg_body: String = client
            .query_one(
                &format!("SELECT pg_get_viewdef('{qname_sql}'::regclass, true)"),
                &[],
            )
            .await
            .map(|row| row.get::<_, String>(0))
            .map_err(|e| anyhow::anyhow!("pg_get_viewdef failed for {qname}: {e}"))?;

        match NormalizedBody::from_sql(&pg_body) {
            Ok(catalog_canonical) => {
                if catalog_canonical.canonical_text() != mv.body_canonical.canonical_text() {
                    report.canonical_mismatches.push(CanonicalMismatch {
                        view_qname: qname.clone(),
                        source_canonical: mv.body_canonical.canonical_text().to_string(),
                        catalog_canonical: catalog_canonical.canonical_text().to_string(),
                    });
                }
            }
            Err(e) => {
                report.canonical_mismatches.push(CanonicalMismatch {
                    view_qname: qname.clone(),
                    source_canonical: mv.body_canonical.canonical_text().to_string(),
                    catalog_canonical: format!("<parse error: {e}>"),
                });
            }
        }

        // 2. body_dependencies vs pg_depend.
        check_dep_edges(
            &client,
            &qname,
            &qname_sql,
            &mv.body_dependencies,
            &mut report,
        )
        .await?;
    }

    if strict
        && (!report.canonical_mismatches.is_empty()
            || !report.extra_ast_edges.is_empty()
            || !report.missing_ast_edges.is_empty())
    {
        anyhow::bail!(
            "shadow-strict: {} canonical mismatch(es), {} extra AST edge(s), {} missing AST edge(s)",
            report.canonical_mismatches.len(),
            report.extra_ast_edges.len(),
            report.missing_ast_edges.len()
        );
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Apply the source catalog to the shadow DB by executing the output of
/// `render_catalog` via `batch_execute`.
async fn apply_source_to_shadow(url: &str, source: &Catalog) -> Result<()> {
    let sql = render_catalog(source);
    if sql.trim().is_empty() {
        return Ok(());
    }
    let (client, conn) = tokio_postgres::connect(url, tokio_postgres::NoTls).await?;
    tokio::spawn(conn);
    client
        .batch_execute(&sql)
        .await
        .map_err(|e| anyhow::anyhow!("apply_source_to_shadow failed: {e}\nSQL:\n{sql}"))?;
    Ok(())
}

/// Cross-check `body_dependencies` for a single view/MV against `pg_depend`.
///
/// Filters body_dependencies to `AstExtracted` edges only — `AstDeclared`
/// edges cover dynamic SQL that pg_depend will never see, so they are tolerated
/// without comparison.
async fn check_dep_edges(
    client: &tokio_postgres::Client,
    qname: &str,
    qname_sql: &str,
    body_dependencies: &[DepEdge],
    report: &mut CrossCheckReport,
) -> Result<()> {
    // Build AST-extracted set from source IR.
    let ast_extracted: BTreeSet<String> = body_dependencies
        .iter()
        .filter(|e| e.source == DepSource::AstExtracted)
        .map(|e| node_id_to_key(&e.to))
        .collect();

    if ast_extracted.is_empty() {
        // No AstExtracted edges to verify — skip pg_depend query.
        return Ok(());
    }

    // Query pg_depend for normal (type 'n') dependencies of this view on
    // pg_class objects (tables, views, MVs).
    let rows = client
        .query(
            &format!(
                "SELECT ref_n.nspname AS ref_schema, ref_c.relname AS ref_name \
                 FROM pg_depend d \
                 JOIN pg_class ref_c ON ref_c.oid = d.refobjid \
                 JOIN pg_namespace ref_n ON ref_n.oid = ref_c.relnamespace \
                 WHERE d.objid = '{qname_sql}'::regclass \
                   AND d.classid = 'pg_class'::regclass \
                   AND d.refclassid = 'pg_class'::regclass \
                   AND d.deptype = 'n'"
            ),
            &[],
        )
        .await
        .map_err(|e| anyhow::anyhow!("pg_depend query failed for {qname}: {e}"))?;

    let pg_deps: BTreeSet<String> = rows
        .iter()
        .map(|row| {
            let schema: String = row.get("ref_schema");
            let name: String = row.get("ref_name");
            format!("{schema}.{name}")
        })
        .collect();

    // extra_ast_edges: in AST set, not in pg_depend.
    for key in &ast_extracted {
        if !pg_deps.contains(key) {
            report.extra_ast_edges.push(ExtraEdge {
                view_qname: qname.to_string(),
                dep_node: key.clone(),
            });
        }
    }

    // missing_ast_edges: in pg_depend, not in AST set.
    for key in &pg_deps {
        if !ast_extracted.contains(key) {
            report.missing_ast_edges.push(MissingEdge {
                view_qname: qname.to_string(),
                ref_schema: key.split('.').next().unwrap_or("").to_string(),
                ref_name: key.split('.').nth(1).unwrap_or("").to_string(),
            });
        }
    }

    Ok(())
}

/// Convert a `NodeId` to a `schema.name` key string for set membership tests.
fn node_id_to_key(node: &NodeId) -> String {
    match node {
        NodeId::Table(q) | NodeId::View(q) | NodeId::Mv(q) => q.to_string(),
        NodeId::Index(q) | NodeId::Sequence(q) => q.to_string(),
        NodeId::Schema(id) => id.to_string(),
        NodeId::Constraint { table, name } => format!("{table}.{name}"),
    }
}
```

- [ ] **Step 2: Run library tests to check it compiles**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo build -p pgevolve 2>&1 | tail -15
```

Expected: compiles without errors. Fix any import issues (e.g., `render_sql` needs the trait in scope — add `use pgevolve_core::identifier::QualifiedName;` if needed, but `render_sql()` is a method on `QualifiedName` directly).

- [ ] **Step 3: Run existing shadow_validate tests (non-Docker ones) to confirm no regressions**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test -p pgevolve --test shadow_validate_flag 2>&1 | tail -10
```

Expected: all 4 tests pass (these don't require Docker).

- [ ] **Step 4: Commit**

```bash
cd /Users/danieltoone/ws/pgevolve && git add crates/pgevolve/src/shadow/validate.rs && git commit -m "feat(shadow): implement cross_check — body canonicalization + pg_depend cross-checks (T13)"
```

---

### Task 4: Update `validate` and `plan` command output for new report fields

**Files:**
- Modify: `crates/pgevolve/src/commands/validate.rs`
- Modify: `crates/pgevolve/src/commands/plan.rs`

The `plan.rs` and `validate.rs` commands reference `report.warnings` and `report.errors` from the old `CrossCheckReport`. The new report uses `canonical_mismatches`, `extra_ast_edges`, `missing_ast_edges`. We must update both callers.

- [ ] **Step 1: Update `validate.rs` to use new report fields**

In `crates/pgevolve/src/commands/validate.rs`, replace the block that references `report.warnings` and `report.errors`:

Old code (lines 77–95):
```rust
        if !report.warnings.is_empty() {
            eprintln!("shadow-validate: {} warning(s):", report.warnings.len());
            for w in &report.warnings {
                eprintln!("  - {w}");
            }
            if args.shadow_strict {
                anyhow::bail!("shadow-validate --strict: warnings treated as errors");
            }
        }
        if !report.errors.is_empty() {
            for e in &report.errors {
                eprintln!("  - {e}");
            }
            anyhow::bail!("shadow-validate: {} error(s)", report.errors.len());
        }
        eprintln!(
            "shadow-validate: ok ({} structural edge(s))",
            report.structural_edges_checked
        );
```

New code:
```rust
        let mismatch_count = report.canonical_mismatches.len()
            + report.extra_ast_edges.len()
            + report.missing_ast_edges.len();
        if mismatch_count > 0 {
            for m in &report.canonical_mismatches {
                eprintln!(
                    "  canonical mismatch {}: source={:?} catalog={:?}",
                    m.view_qname, m.source_canonical, m.catalog_canonical
                );
            }
            for e in &report.extra_ast_edges {
                eprintln!("  extra AST edge {}: {}", e.view_qname, e.dep_node);
            }
            for m in &report.missing_ast_edges {
                eprintln!(
                    "  missing AST edge {}: {}.{}",
                    m.view_qname, m.ref_schema, m.ref_name
                );
            }
            if args.shadow_strict {
                anyhow::bail!(
                    "shadow-validate --strict: {} mismatch(es)",
                    mismatch_count
                );
            }
        }
        eprintln!(
            "shadow-validate: ok ({} structural edge(s), {} canonical mismatch(es))",
            report.structural_edges_checked,
            mismatch_count,
        );
```

- [ ] **Step 2: Update `plan.rs` similarly**

In `crates/pgevolve/src/commands/plan.rs`, replace the block referencing `report.warnings` and `report.errors` in `run_shadow_cross_check`:

Old code (lines 167–181):
```rust
    if !report.warnings.is_empty() {
        eprintln!("shadow-validate: {} warning(s):", report.warnings.len());
        for w in &report.warnings {
            eprintln!("  - {w}");
        }
        if strict {
            anyhow::bail!("shadow-validate --strict: warnings treated as errors");
        }
    }
    if !report.errors.is_empty() {
        for e in &report.errors {
            eprintln!("  - {e}");
        }
        anyhow::bail!("shadow-validate: {} error(s)", report.errors.len());
    }
    Ok(())
```

New code:
```rust
    let mismatch_count = report.canonical_mismatches.len()
        + report.extra_ast_edges.len()
        + report.missing_ast_edges.len();
    if mismatch_count > 0 {
        for m in &report.canonical_mismatches {
            eprintln!(
                "  canonical mismatch {}: source={:?} catalog={:?}",
                m.view_qname, m.source_canonical, m.catalog_canonical
            );
        }
        for e in &report.extra_ast_edges {
            eprintln!("  extra AST edge {}: {}", e.view_qname, e.dep_node);
        }
        for m in &report.missing_ast_edges {
            eprintln!(
                "  missing AST edge {}: {}.{}",
                m.view_qname, m.ref_schema, m.ref_name
            );
        }
        if strict {
            anyhow::bail!("shadow-validate --strict: {} mismatch(es)", mismatch_count);
        }
    }
    Ok(())
```

- [ ] **Step 3: Build to confirm no compile errors**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo build --workspace 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 4: Run non-Docker tests to confirm no regressions**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test --workspace --lib --tests -- --skip shadow_validate_views --skip shadow_round_trip 2>&1 | tail -15
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/danieltoone/ws/pgevolve && git add crates/pgevolve/src/commands/validate.rs crates/pgevolve/src/commands/plan.rs && git commit -m "fix(shadow): update validate/plan commands to use new CrossCheckReport fields"
```

---

### Task 5: Write Docker-gated integration test for happy-path view cross-check

**Files:**
- Create: `crates/pgevolve/tests/shadow_validate_views.rs`

This test directly calls `shadow::validate::cross_check` with a constructed `Catalog` (one table + one view) against a live shadow PG. No CLI subprocess needed.

- [ ] **Step 1: Write the test file**

Create `crates/pgevolve/tests/shadow_validate_views.rs`:

```rust
//! Docker-gated tests for --shadow-validate covering views.
//!
//! These tests call `shadow::validate::cross_check` directly (no CLI subprocess)
//! with a constructed Catalog containing one table and one view.
//!
//! Skipped automatically when Docker is unavailable.

use pgevolve::shadow::testcontainers::TestcontainersBackend;
use pgevolve::shadow::validate::cross_check;
use pgevolve_core::catalog::PgVersion;
use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::column::Column;
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::constraint::{Constraint, ConstraintKind, Deferrable};
use pgevolve_core::ir::schema::Schema;
use pgevolve_core::ir::table::Table;
use pgevolve_core::ir::view::{View, ViewColumn};
use pgevolve_core::parse::normalize_body::NormalizedBody;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};
use pgevolve_testkit::ephemeral_pg::{default_pg_version, docker_available};

fn id(s: &str) -> Identifier {
    Identifier::from_unquoted(s).unwrap()
}

fn qn(schema: &str, name: &str) -> QualifiedName {
    QualifiedName::new(id(schema), id(name))
}

fn body(sql: &str) -> NormalizedBody {
    NormalizedBody::from_sql(sql).unwrap()
}

fn make_shadow_config_for_version(version: PgVersion) -> pgevolve::config::ShadowConfig {
    let pg_version_str = match version {
        PgVersion::Pg14 => "14",
        PgVersion::Pg15 => "15",
        PgVersion::Pg16 => "16",
        PgVersion::Pg17 => "17",
    };
    pgevolve::config::ShadowConfig {
        backend: Some("testcontainers".to_string()),
        postgres_version: Some(pg_version_str.to_string()),
        url: None,
        url_env: None,
        extensions: vec![],
    }
}

/// Build a small catalog: `app` schema, `app.users` table, `app.active_users` view.
fn small_catalog_with_view() -> Catalog {
    let mut cat = Catalog::empty();

    cat.schemas.push(Schema::new(id("app")));

    cat.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![
            Column {
                name: id("id"),
                ty: ColumnType::BigInt,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                comment: None,
            },
            Column {
                name: id("active"),
                ty: ColumnType::Boolean,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                comment: None,
            },
        ],
        constraints: vec![Constraint {
            qname: qn("app", "users_pkey"),
            kind: ConstraintKind::PrimaryKey {
                columns: vec![id("id")],
                include: vec![],
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }],
        comment: None,
    });

    // A view that selects from app.users.
    // The body_dependencies include an AstExtracted edge view → table.
    cat.views.push(View {
        qname: qn("app", "active_users"),
        columns: vec![ViewColumn {
            name: id("id"),
            column_type: ColumnType::BigInt,
            comment: None,
        }],
        body_canonical: body("SELECT id FROM app.users WHERE active"),
        body_dependencies: vec![DepEdge {
            from: NodeId::View(qn("app", "active_users")),
            to: NodeId::Table(qn("app", "users")),
            source: DepSource::AstExtracted,
        }],
        security_barrier: None,
        security_invoker: None,
        comment: None,
        raw_body: String::new(),
    });

    cat
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadow_validate_views_happy_path() {
    if !docker_available() {
        eprintln!("Docker unavailable; skipping shadow_validate_views_happy_path");
        return;
    }

    let version = default_pg_version();
    let shadow_cfg = make_shadow_config_for_version(version);
    let backend = pgevolve::shadow::resolve(&shadow_cfg).expect("resolve backend");

    let source = small_catalog_with_view();
    let pg_major = match version {
        PgVersion::Pg14 => 14u32,
        PgVersion::Pg15 => 15,
        PgVersion::Pg16 => 16,
        PgVersion::Pg17 => 17,
    };

    let report = cross_check(backend.as_ref(), &source, pg_major, false)
        .await
        .expect("cross_check must succeed on happy-path catalog");

    assert_eq!(
        report.canonical_mismatches.len(),
        0,
        "expected 0 canonical mismatches: {:?}",
        report.canonical_mismatches
    );
    // extra_ast_edges: we declared app.users as a dependency; pg_depend should confirm it.
    assert_eq!(
        report.extra_ast_edges.len(),
        0,
        "expected 0 extra AST edges: {:?}",
        report.extra_ast_edges
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadow_validate_strict_passes_on_clean_catalog() {
    if !docker_available() {
        eprintln!("Docker unavailable; skipping shadow_validate_strict_passes_on_clean_catalog");
        return;
    }

    let version = default_pg_version();
    let shadow_cfg = make_shadow_config_for_version(version);
    let backend = pgevolve::shadow::resolve(&shadow_cfg).expect("resolve backend");

    let source = small_catalog_with_view();
    let pg_major = match version {
        PgVersion::Pg14 => 14u32,
        PgVersion::Pg15 => 15,
        PgVersion::Pg16 => 16,
        PgVersion::Pg17 => 17,
    };

    // strict=true should not bail when everything matches.
    let result = cross_check(backend.as_ref(), &source, pg_major, true).await;
    assert!(
        result.is_ok(),
        "strict cross_check should succeed on clean catalog: {:?}",
        result
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadow_validate_strict_fails_on_body_mismatch() {
    if !docker_available() {
        eprintln!("Docker unavailable; skipping shadow_validate_strict_fails_on_body_mismatch");
        return;
    }

    let version = default_pg_version();
    let shadow_cfg = make_shadow_config_for_version(version);
    let backend = pgevolve::shadow::resolve(&shadow_cfg).expect("resolve backend");

    let pg_major = match version {
        PgVersion::Pg14 => 14u32,
        PgVersion::Pg15 => 15,
        PgVersion::Pg16 => 16,
        PgVersion::Pg17 => 17,
    };

    // Build a catalog where the stored body is intentionally wrong
    // (we store "SELECT 1" but the shadow will get the real view body injected
    // from render_catalog, which uses body_canonical).
    // To trigger a real mismatch we would need to tamper with the stored
    // body_canonical without changing what render_catalog emits — tricky.
    // Instead, test a simpler scenario: a view with no AstExtracted edges
    // but pg_depend sees a real dependency → missing_ast_edges is non-empty
    // → strict should bail.
    //
    // Build a view that actually depends on app.users but declares NO body_dependencies
    // (simulating a missing AstExtracted edge). pg_depend will see the dep; our AST set won't.

    let mut cat = Catalog::empty();
    cat.schemas.push(Schema::new(id("app")));
    cat.tables.push(Table {
        qname: qn("app", "users"),
        columns: vec![Column {
            name: id("id"),
            ty: ColumnType::BigInt,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            comment: None,
        }],
        constraints: vec![],
        comment: None,
    });
    // View depends on app.users in SQL but declares one AstExtracted dep
    // so we trigger the pg_depend check path. We claim it depends on a
    // non-existent table to produce an extra_ast_edges entry.
    cat.views.push(View {
        qname: qn("app", "v"),
        columns: vec![],
        // This body actually references app.users, but we declare a fake dep.
        body_canonical: body("SELECT id FROM app.users"),
        body_dependencies: vec![DepEdge {
            from: NodeId::View(qn("app", "v")),
            to: NodeId::Table(qn("app", "no_such_table")), // intentionally wrong
            source: DepSource::AstExtracted,
        }],
        security_barrier: None,
        security_invoker: None,
        comment: None,
        raw_body: String::new(),
    });

    let result = cross_check(backend.as_ref(), &cat, pg_major, true).await;
    assert!(
        result.is_err(),
        "strict cross_check should fail when extra/missing AST edges exist"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("shadow-strict"),
        "error message should mention shadow-strict: {msg}"
    );
}
```

- [ ] **Step 2: Check what `ShadowConfig` looks like to ensure the struct is correct**

```bash
grep -n "pub struct ShadowConfig\|pub.*backend\|pub.*postgres_version\|pub.*url\|pub.*extensions" /Users/danieltoone/ws/pgevolve/crates/pgevolve/src/config.rs | head -20
```

Adjust the `make_shadow_config_for_version` function if field names differ.

- [ ] **Step 3: Build the test binary to check it compiles**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test -p pgevolve --test shadow_validate_views --no-run 2>&1 | tail -15
```

Expected: compiles (even without Docker the binary builds). Fix any field name / import issues.

- [ ] **Step 4: Run the tests (Docker-gated)**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test -p pgevolve --test shadow_validate_views 2>&1 | tail -20
```

Expected:
- If Docker available: all 3 tests pass (or 2/3 if the mismatch test needs adjustment).
- If Docker unavailable: all 3 tests print "Docker unavailable; skipping" and pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/danieltoone/ws/pgevolve && git add crates/pgevolve/tests/shadow_validate_views.rs && git commit -m "test(shadow): Docker-gated cross-check tests for views (T13)"
```

---

### Task 6: Final verification — build, test, clippy, fmt

- [ ] **Step 1: Full workspace build**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo build --workspace 2>&1 | tail -5
```

Expected: `Finished` with no errors.

- [ ] **Step 2: All lib + non-Docker tests**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo test --workspace --lib --tests 2>&1 | tail -10
```

Expected: all pass (Docker tests may skip if Docker not available).

- [ ] **Step 3: Clippy**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: clean (no warnings promoted to errors). Fix any clippy issues, commit.

- [ ] **Step 4: Fmt check**

```bash
cd /Users/danieltoone/ws/pgevolve && cargo fmt --check 2>&1 | tail -5
```

If it reports formatting issues, run `cargo fmt` then commit.

- [ ] **Step 5: Final commit if fmt/clippy required fixes**

```bash
cd /Users/danieltoone/ws/pgevolve && git add -p && git commit -m "style: apply cargo fmt and fix clippy warnings for T13"
```

---

## Self-Review Notes

**Spec coverage:**
- [x] Task 3 implements `pg_get_viewdef` body round-trip check → `canonical_mismatches`
- [x] Task 3 implements `pg_depend` cross-check → `missing_ast_edges` / `extra_ast_edges`
- [x] `AstDeclared` edges are filtered out in `check_dep_edges` (only `AstExtracted` compared)
- [x] `--shadow-strict` bails via anyhow in `cross_check` when any mismatch list is non-empty
- [x] Tasks 1–2 extend `render_catalog` to apply views/MVs to shadow via option 3 (render path)
- [x] Task 5 provides happy-path test (0 mismatches) + strict-pass + strict-fail tests
- [x] Existing v0.1 shadow_validate tests remain unbroken (Task 4 updates callers)

**Potential gotchas:**
- `apply_source_to_shadow` calls `render_catalog` then `batch_execute`. If the view body contains syntax that pg_query's deparser normalizes differently from what Postgres expects, this could fail at apply time — but that's a signal the body_canonical is corrupt, not a T13 bug.
- `pg_get_viewdef` with `pretty=true` returns indented SQL. `NormalizedBody::from_sql` will collapse whitespace, so the canonical comparison is whitespace-insensitive. This is correct.
- The `pg_depend` query uses `deptype = 'n'` (normal). Postgres also records `'a'` (auto) deps for view columns. We filter to `'n'` only to match relation-level deps; column deps would create spurious `missing_ast_edges`.
- The `ShadowConfig` struct must be verified in Task 5 Step 2 — field names like `extensions` default to `vec![]`.
