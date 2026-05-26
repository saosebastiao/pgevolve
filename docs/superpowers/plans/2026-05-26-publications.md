# PUBLICATION Implementation Plan (v0.3.4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship v0.3.4 — first-class `Publication` IR object covering all five Postgres PUBLICATION syntactic forms (explicit `FOR TABLE`, `FOR ALL TABLES`, `FOR TABLES IN SCHEMA` PG15+, row filters PG15+, column lists PG15+), with the standard v0.3.x lenient drift behavior at the per-publication grain.

**Architecture:** Eleven sequential stages mirroring the v0.3.3 reloptions shape. `PublicationScope` is a sum-type encoding the mutual exclusion of `AllTables` vs `Selective`. Row filters reuse `NormalizedExpr` for canon. Lenient drift is at the *whole-publication* level: a publication in source is managed, one absent from source surfaces via `unmanaged-publication` lint. PG-version-gated features (schema-scope, row filter, column list) fail at lint time, not at apply time, via a new `[managed].min_pg_version` config key.

**Tech Stack:** Rust 1.95+, `pg_query` 6.x, `tokio_postgres`, `serde`, `proptest`. Builds on every v0.3.x pattern (no new cross-cutting concerns).

**Source spec:** `docs/superpowers/specs/2026-05-26-publications-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --tests
```

All green. v0.3.3 is committed; `main` is clean.

- [ ] **Step 2: Skim the spec once**

Open `docs/superpowers/specs/2026-05-26-publications-design.md`. Each stage below cites the spec section it implements.

- [ ] **Step 3: Skim the v0.3.3 reloptions plan as a structural template**

`docs/superpowers/plans/2026-05-22-table-reloptions.md`. The IR-foundation → canon → catalog → parse → diff → render+steps → lint → conformance → proptest cadence is identical; the differences are noted per-stage.

---

## File structure

```
crates/pgevolve-core/src/
├── ir/
│   ├── publication.rs                NEW — Stage 1 — Publication, PublicationScope, PublishedTable, PublishKinds
│   ├── catalog.rs                    MODIFY — Stage 2 — add publications field + empty() / canonicalize wiring
│   ├── mod.rs                        MODIFY — Stage 1 — re-export publication
│   └── canon/
│       ├── mod.rs                    MODIFY — Stage 3 — wire publications pass into orchestrator
│       └── publications.rs           NEW — Stage 3 — sort + validate non-empty Selective
├── catalog/
│   ├── publications.rs               NEW — Stage 5 — decode_publication + per-version SQL
│   ├── queries/
│   │   ├── shared.rs                 MODIFY — Stage 5 — add PUBLICATIONS_QUERY + PUBLICATION_REL_QUERY + PUBLICATION_NAMESPACE_QUERY
│   │   └── pg14.rs                   MODIFY — Stage 5 — PG14 variants (no namespace, no prqual/prattrs)
│   ├── assemble/
│   │   └── publications.rs           NEW — Stage 5 — assemble_publications
│   └── mod.rs                        MODIFY — Stage 5 — wire publications read into read_catalog
├── parse/
│   └── builder/
│       ├── publication_stmt.rs       NEW — Stage 6 — CREATE / ALTER PUBLICATION
│       └── mod.rs                    MODIFY — Stage 6 — register + dispatch
├── diff/
│   ├── publications.rs               NEW — Stage 7 — per-publication granular diff
│   ├── change.rs                     MODIFY — Stage 7 — 11 new Change variants
│   ├── mod.rs                        MODIFY — Stage 7 — call diff_publications in top-level diff
│   └── owner_op.rs                   MODIFY — Stage 7 — add OwnerObjectKind::Publication
├── plan/
│   ├── raw_step.rs                   MODIFY — Stage 8 — 11 new StepKind variants
│   ├── plan.rs                       MODIFY — Stage 8 — extend kind_name / parse_kind_name
│   ├── edges.rs                      MODIFY — Stage 8 — add NodeId::Publication + dep edges
│   └── rewrite/
│       ├── publications.rs           NEW — Stage 8 — SQL helpers
│       └── mod.rs                    MODIFY — Stage 8 — dispatch 11 emit arms
└── lint/
    ├── rules/
    │   ├── unmanaged_publication.rs                              NEW — Stage 9
    │   ├── publication_captures_unmanaged_table.rs               NEW — Stage 9
    │   ├── publication_row_filter_references_unmanaged_column.rs NEW — Stage 9
    │   ├── publication_feature_requires_pg_version.rs            NEW — Stage 9
    │   └── mod.rs                                                MODIFY — Stage 9
    └── universal.rs                  MODIFY — Stage 9 — wire 4 rules

crates/pgevolve/src/
├── config.rs                         MODIFY — Stage 4 — add [managed].min_pg_version
└── commands/diff.rs                  MODIFY — Stage 7 — print_human + change_kind_name for 11 variants

crates/pgevolve-conformance/tests/cases/objects/
└── publications/                     NEW — Stage 10 — 12 fixtures

crates/pgevolve-testkit/src/
└── ir_generator.rs                   MODIFY — Stage 11 — arb_publication strategies

docs/spec/
├── objects.md                        MODIFY — Stage 11 — PUBLICATION rows ✅ Supported
└── publications.md                   NEW — Stage 11 — capability page

CHANGELOG.md                          MODIFY — Stage 11 — [0.3.4] section
Cargo.toml                            MODIFY — Stage 11 — version 0.3.3 → 0.3.4
```

---

## Stage 1 — IR foundation

Pure data types in `ir::publication`. No behavior beyond derives.

**Files created:** `crates/pgevolve-core/src/ir/publication.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/mod.rs`.

**Spec ref:** "IR shape".

### Task 1.1: Create the module

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/publication.rs`**

```rust
//! Publication IR — declarative logical-replication source-side metadata.
//!
//! A `Publication` is a Postgres `CREATE PUBLICATION` object. It lives at
//! the Catalog top level (not schema-qualified) because Postgres treats
//! publications as a per-database global namespace.
//!
//! Spec: `docs/superpowers/specs/2026-05-26-publications-design.md`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;

/// Declarative model of a Postgres `PUBLICATION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publication {
    /// Publication name (not schema-qualified — publications are global).
    pub name: Identifier,
    /// Which tables / schemas are published.
    pub scope: PublicationScope,
    /// Which DML kinds are replicated.
    pub publish: PublishKinds,
    /// Whether `INSERT`/`UPDATE`/`DELETE` on partition children should be
    /// reported using the partition root's identity (PG 13+).
    pub publish_via_partition_root: bool,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER PUBLICATION ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

/// Target set of a publication. Encodes PG's mutual exclusion of
/// `FOR ALL TABLES` and the selective forms at the type level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicationScope {
    /// `CREATE PUBLICATION p FOR ALL TABLES`. Implicitly captures every
    /// current and future table in the database.
    AllTables,
    /// `CREATE PUBLICATION p FOR TABLE ..., TABLES IN SCHEMA ...`.
    /// Either list may be empty (but not both — canon rejects empty
    /// Selective). Schema-scope is PG 15+ only.
    Selective {
        /// Schemas published in their entirety. PG 15+.
        schemas: BTreeSet<Identifier>,
        /// Per-table publication entries with optional row filter and
        /// column list. Sorted by `qname` after canon.
        tables: Vec<PublishedTable>,
    },
}

/// A single table entry inside `PublicationScope::Selective`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedTable {
    /// Schema-qualified table name.
    pub qname: QualifiedName,
    /// Optional `WHERE` row filter (PG 15+). Canonicalized via
    /// `NormalizedExpr`.
    pub row_filter: Option<NormalizedExpr>,
    /// Optional explicit column list (PG 15+). Sorted by name after canon.
    /// `None` = all columns; `Some(empty)` is rejected by canon.
    pub columns: Option<Vec<Identifier>>,
}

/// Which DML kinds a publication replicates. Maps to PG's four
/// `pg_publication.pub{insert,update,delete,truncate}` booleans, and the
/// source SQL `publish = 'insert, update, delete, truncate'` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishKinds {
    /// `INSERT` is replicated.
    pub insert: bool,
    /// `UPDATE` is replicated.
    pub update: bool,
    /// `DELETE` is replicated.
    pub delete: bool,
    /// `TRUNCATE` is replicated.
    pub truncate: bool,
}

impl PublishKinds {
    /// PG's `CREATE PUBLICATION` default when `publish` is unspecified:
    /// all four DML kinds enabled.
    #[must_use]
    pub const fn pg_default() -> Self {
        Self {
            insert: true,
            update: true,
            delete: true,
            truncate: true,
        }
    }

    /// True iff at least one DML kind is enabled. An empty bitset is
    /// illegal at the IR level (canon rejects).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.insert && !self.update && !self.delete && !self.truncate
    }
}
```

- [ ] **Step 2: Add to `crates/pgevolve-core/src/ir/mod.rs`**

```rust
pub mod publication;
```

(Alphabetical position within the existing `pub mod` list.)

- [ ] **Step 3: Build to verify**

```bash
cargo build -p pgevolve-core
```

Expected: clean compile, no warnings.

- [ ] **Step 4: Write unit tests** (in the module's `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn publish_kinds_default_all_true() {
        let k = PublishKinds::pg_default();
        assert!(k.insert && k.update && k.delete && k.truncate);
        assert!(!k.is_empty());
    }

    #[test]
    fn publish_kinds_is_empty_when_all_false() {
        let k = PublishKinds { insert: false, update: false, delete: false, truncate: false };
        assert!(k.is_empty());
    }

    #[test]
    fn scope_all_tables_does_not_equal_empty_selective() {
        let a = PublicationScope::AllTables;
        let b = PublicationScope::Selective {
            schemas: BTreeSet::new(),
            tables: Vec::new(),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn selective_with_a_schema_equals_itself() {
        let s = PublicationScope::Selective {
            schemas: BTreeSet::from([id("app")]),
            tables: Vec::new(),
        };
        assert_eq!(s.clone(), s);
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p pgevolve-core --lib ir::publication
```

Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/ir/publication.rs crates/pgevolve-core/src/ir/mod.rs
git commit -m "$(cat <<'EOF'
feat(ir): Publication, PublicationScope, PublishedTable, PublishKinds

New top-level IR module for PUBLICATION. Pure data types; no
behavior beyond derives. PublicationScope is a sum-type encoding
PG's mutual exclusion of FOR ALL TABLES vs the selective forms;
row filters reuse NormalizedExpr for canon consistency with
CHECK / USING / WITH CHECK.

Stage 1 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — Add `publications` field to Catalog

**Files modified:** `crates/pgevolve-core/src/ir/catalog.rs`.

### Task 2.1: Backfill the new field

- [ ] **Step 1: Add the field to `Catalog` struct**

In `crates/pgevolve-core/src/ir/catalog.rs`, append to the struct definition (alphabetical / logical position — after `triggers`, before `default_privileges`):

```rust
    /// Publications (logical-replication source-side metadata).
    pub publications: Vec<crate::ir::publication::Publication>,
```

- [ ] **Step 2: Initialize in `Catalog::empty()`**

```rust
            publications: Vec::new(),
```

- [ ] **Step 3: Backfill every `Catalog { ... }` struct literal in the codebase**

```bash
grep -rln "Catalog {" crates/ | xargs grep -l "schemas:" | head
```

Each literal that constructs a `Catalog` by hand needs `publications: Vec::new()`. Use `Catalog::empty()` as a base where the literal is just-empty-with-overrides.

Expect 15–40 sites across tests, fixtures, and assemblers.

- [ ] **Step 4: Build**

```bash
cargo build -p pgevolve-core
cargo build --workspace
```

Expected: clean. Any "missing field publications" errors flag a site you missed in step 3.

- [ ] **Step 5: Run existing tests**

```bash
cargo test --workspace --lib
```

Expected: all pass. (No behavior change yet; this is pure plumbing.)

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/
git commit -m "$(cat <<'EOF'
feat(ir): add Catalog::publications

Backfills every Catalog struct literal in the workspace with
publications: Vec::new(). Pure plumbing — no behavior change.

Stage 2 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Canon pass

Validate and sort. Two invariants enforced:

1. `Selective { schemas: empty, tables: empty }` → `IrError::EmptyPublication`.
2. `PublishedTable.columns = Some(empty)` → `IrError::EmptyColumnList`.
3. Tables sorted by `qname`. Column lists sorted by name with duplicates rejected.
4. `PublishKinds::is_empty()` → `IrError::EmptyPublishBitset`.

**Files created:** `crates/pgevolve-core/src/ir/canon/publications.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/canon/mod.rs`, `crates/pgevolve-core/src/ir/error.rs` (add error variants).

### Task 3.1: Add error variants

- [ ] **Step 1: Extend `IrError`**

In `crates/pgevolve-core/src/ir/error.rs`:

```rust
    /// A `PublicationScope::Selective` had no schemas and no tables.
    #[error("publication {0:?}: empty Selective scope (no tables, no schemas)")]
    EmptyPublication(crate::identifier::Identifier),
    /// A `PublishedTable.columns` was `Some(vec![])`.
    #[error("publication {0:?} table {1:?}: empty column list (use None to publish all columns)")]
    EmptyColumnList(crate::identifier::Identifier, crate::identifier::QualifiedName),
    /// A `PublishKinds` had all four DML flags false.
    #[error("publication {0:?}: empty publish bitset (must enable at least one DML kind)")]
    EmptyPublishBitset(crate::identifier::Identifier),
    /// A `PublishedTable.columns` contained a duplicate column name.
    #[error("publication {0:?} table {1:?}: duplicate column {2:?} in column list")]
    DuplicateColumnInPublication(
        crate::identifier::Identifier,
        crate::identifier::QualifiedName,
        crate::identifier::Identifier,
    ),
```

- [ ] **Step 2: Build**

```bash
cargo build -p pgevolve-core
```

### Task 3.2: Create the canon pass

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/canon/publications.rs`**

```rust
//! Canon pass for publications. Validates and sorts.
//!
//! Invariants enforced:
//! - `Selective` with no tables and no schemas → error.
//! - `PublishKinds` with no enabled DML kinds → error.
//! - `PublishedTable.columns = Some(empty)` → error.
//! - Duplicate column in a `PublishedTable.columns` → error.
//!
//! Sorts:
//! - `Selective.tables` by `qname`.
//! - Each `PublishedTable.columns` by name (when `Some`).
//! - The publications collection itself is sorted by `sort_and_dedupe`,
//!   not here.

use crate::ir::catalog::Catalog;
use crate::ir::error::IrError;
use crate::ir::publication::{Publication, PublicationScope};

pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for p in &mut cat.publications {
        validate_and_sort(p)?;
    }
    Ok(())
}

fn validate_and_sort(p: &mut Publication) -> Result<(), IrError> {
    if p.publish.is_empty() {
        return Err(IrError::EmptyPublishBitset(p.name.clone()));
    }
    if let PublicationScope::Selective { schemas, tables } = &mut p.scope {
        if schemas.is_empty() && tables.is_empty() {
            return Err(IrError::EmptyPublication(p.name.clone()));
        }
        // Tables: sort by qname; per-table column lists: validate + sort.
        tables.sort_by(|a, b| a.qname.cmp(&b.qname));
        for t in tables.iter_mut() {
            if let Some(cols) = &mut t.columns {
                if cols.is_empty() {
                    return Err(IrError::EmptyColumnList(p.name.clone(), t.qname.clone()));
                }
                cols.sort();
                for w in cols.windows(2) {
                    if w[0] == w[1] {
                        return Err(IrError::DuplicateColumnInPublication(
                            p.name.clone(),
                            t.qname.clone(),
                            w[0].clone(),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::publication::{PublishKinds, PublishedTable};
    use std::collections::BTreeSet;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn pub_with_scope(scope: PublicationScope) -> Publication {
        Publication {
            name: id("p"),
            scope,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn rejects_empty_selective() {
        let mut cat = Catalog::empty();
        cat.publications.push(pub_with_scope(PublicationScope::Selective {
            schemas: BTreeSet::new(),
            tables: Vec::new(),
        }));
        let err = run(&mut cat).unwrap_err();
        assert!(matches!(err, IrError::EmptyPublication(_)));
    }

    #[test]
    fn rejects_empty_publish_bitset() {
        let mut cat = Catalog::empty();
        let mut p = pub_with_scope(PublicationScope::AllTables);
        p.publish = PublishKinds { insert: false, update: false, delete: false, truncate: false };
        cat.publications.push(p);
        assert!(matches!(run(&mut cat).unwrap_err(), IrError::EmptyPublishBitset(_)));
    }

    #[test]
    fn rejects_empty_column_list() {
        let mut cat = Catalog::empty();
        cat.publications.push(pub_with_scope(PublicationScope::Selective {
            schemas: BTreeSet::new(),
            tables: vec![PublishedTable {
                qname: qn("app", "t"),
                row_filter: None,
                columns: Some(vec![]),
            }],
        }));
        assert!(matches!(run(&mut cat).unwrap_err(), IrError::EmptyColumnList(_, _)));
    }

    #[test]
    fn rejects_duplicate_columns() {
        let mut cat = Catalog::empty();
        cat.publications.push(pub_with_scope(PublicationScope::Selective {
            schemas: BTreeSet::new(),
            tables: vec![PublishedTable {
                qname: qn("app", "t"),
                row_filter: None,
                columns: Some(vec![id("a"), id("a")]),
            }],
        }));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateColumnInPublication(_, _, _)
        ));
    }

    #[test]
    fn sorts_tables_and_columns() {
        let mut cat = Catalog::empty();
        cat.publications.push(pub_with_scope(PublicationScope::Selective {
            schemas: BTreeSet::new(),
            tables: vec![
                PublishedTable {
                    qname: qn("app", "z"),
                    row_filter: None,
                    columns: Some(vec![id("c"), id("a"), id("b")]),
                },
                PublishedTable {
                    qname: qn("app", "a"),
                    row_filter: None,
                    columns: None,
                },
            ],
        }));
        run(&mut cat).unwrap();
        let PublicationScope::Selective { tables, .. } = &cat.publications[0].scope else {
            panic!("expected Selective")
        };
        assert_eq!(tables[0].qname.name.as_str(), "a");
        assert_eq!(tables[1].qname.name.as_str(), "z");
        let cols = tables[1].columns.as_ref().unwrap();
        assert_eq!(cols[0].as_str(), "a");
        assert_eq!(cols[1].as_str(), "b");
        assert_eq!(cols[2].as_str(), "c");
    }

    #[test]
    fn all_tables_skips_selective_validation() {
        let mut cat = Catalog::empty();
        cat.publications.push(pub_with_scope(PublicationScope::AllTables));
        assert!(run(&mut cat).is_ok());
    }
}
```

- [ ] **Step 2: Wire into orchestrator**

In `crates/pgevolve-core/src/ir/canon/mod.rs`, add `pub mod publications;` and call `publications::run(cat)?;` between `policies::run_on_table` and `reloptions::run` (alphabetical / pipeline-order position).

- [ ] **Step 3: Build + test**

```bash
cargo test -p pgevolve-core --lib ir::canon::publications
```

Expected: 6 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/ir/canon/
git commit -m "$(cat <<'EOF'
feat(ir): canon pass for publications

Validates empty Selective / empty publish bitset / empty column
list / duplicate columns. Sorts tables by qname, per-table column
lists by name.

Stage 3 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — `[managed].min_pg_version` config

Add a new optional config key so PG-version-gated features can fail at lint time instead of apply time. Default `14` (the workspace minimum).

**Files modified:** `crates/pgevolve/src/config.rs`.

### Task 4.1: Add the config field

- [ ] **Step 1: Extend `Managed` struct in `crates/pgevolve/src/config.rs`**

Find the existing `pub struct Managed { ... }` and add:

```rust
    /// Minimum Postgres major version the project targets. Default 14.
    /// Used to gate PG-version-specific source features (e.g., publication
    /// row filters require PG 15+). When source uses a feature newer than
    /// `min_pg_version`, lint fires `publication-feature-requires-pg-version`
    /// (Error) instead of letting the apply hit a Postgres syntax error.
    #[serde(default = "default_min_pg_version")]
    pub min_pg_version: u32,
```

Then add the default function near the others in the file:

```rust
fn default_min_pg_version() -> u32 {
    14
}
```

- [ ] **Step 2: Add unit test**

In `crates/pgevolve/src/config.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn min_pg_version_defaults_to_14() {
        let cfg: PgevolveConfig = toml::from_str(
            r#"
[project]
name = "t"
schema_dir = "schema"
plan_dir = "plans"
[managed]
schemas = ["app"]
[environments.dev]
url = "postgres://localhost"
"#,
        )
        .unwrap();
        assert_eq!(cfg.managed.min_pg_version, 14);
    }

    #[test]
    fn min_pg_version_can_be_raised() {
        let cfg: PgevolveConfig = toml::from_str(
            r#"
[project]
name = "t"
schema_dir = "schema"
plan_dir = "plans"
[managed]
schemas = ["app"]
min_pg_version = 16
[environments.dev]
url = "postgres://localhost"
"#,
        )
        .unwrap();
        assert_eq!(cfg.managed.min_pg_version, 16);
    }
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p pgevolve --lib config
```

Expected: passes including 2 new tests.

- [ ] **Step 4: Update `docs/user/configuration.md`**

Find the `[managed]` section, add the row under the existing key documentation:

```markdown
| `min_pg_version` | `14` | Minimum PG major version the project targets. Gates PG-version-specific source features (e.g., publication row filters need PG 15+). |
```

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve/src/config.rs docs/user/configuration.md
git commit -m "$(cat <<'EOF'
feat(config): [managed].min_pg_version

Default 14 (workspace minimum). Used to gate PG-version-specific
source features at lint time instead of apply time. Specifically
needed by v0.3.4 PUBLICATION for the PG15+ features
(row filters, column lists, FOR TABLES IN SCHEMA).

Stage 4 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — Catalog reader

Three pg_catalog tables read per-version. Decoder reconstructs `Publication` IR from rows.

**Files created:** `crates/pgevolve-core/src/catalog/publications.rs`, `crates/pgevolve-core/src/catalog/assemble/publications.rs`.
**Files modified:** `crates/pgevolve-core/src/catalog/queries/{shared,pg14}.rs`, `crates/pgevolve-core/src/catalog/mod.rs`.

### Task 5.1: Per-version SQL strings

- [ ] **Step 1: Add to `crates/pgevolve-core/src/catalog/queries/shared.rs`**

```rust
/// Publications (PG 15+ schema-scope + row filter + column list support).
pub const PUBLICATIONS_QUERY: &str = "\
    SELECT \
        p.oid::bigint AS oid, \
        p.pubname::text AS name, \
        coalesce(a.rolname, '') AS owner, \
        p.puballtables AS all_tables, \
        p.pubinsert AS pub_insert, \
        p.pubupdate AS pub_update, \
        p.pubdelete AS pub_delete, \
        p.pubtruncate AS pub_truncate, \
        p.pubviaroot AS publish_via_partition_root, \
        coalesce(d.description, '') AS comment \
    FROM pg_publication p \
    JOIN pg_authid a ON a.oid = p.pubowner \
    LEFT JOIN pg_description d \
        ON d.classoid = 'pg_publication'::regclass AND d.objoid = p.oid AND d.objsubid = 0 \
    ORDER BY p.pubname";

/// Per-table publication entries with PG 15+ row filter (`prqual`) and
/// column list (`prattrs`). Decoded with `pg_get_expr` for the row filter.
pub const PUBLICATION_REL_QUERY: &str = "\
    SELECT \
        pr.prpubid::bigint AS pub_oid, \
        ns.nspname::text AS schema, \
        c.relname::text AS table_name, \
        pg_get_expr(pr.prqual, pr.prrelid) AS row_filter, \
        pr.prattrs::int2[] AS col_attnums, \
        c.oid::bigint AS rel_oid \
    FROM pg_publication_rel pr \
    JOIN pg_class c ON c.oid = pr.prrelid \
    JOIN pg_namespace ns ON ns.oid = c.relnamespace \
    ORDER BY pr.prpubid, ns.nspname, c.relname";

/// Schema-scope publication entries (PG 15+ only).
pub const PUBLICATION_NAMESPACE_QUERY: &str = "\
    SELECT \
        pn.pnpubid::bigint AS pub_oid, \
        ns.nspname::text AS schema \
    FROM pg_publication_namespace pn \
    JOIN pg_namespace ns ON ns.oid = pn.pnnspid \
    ORDER BY pn.pnpubid, ns.nspname";
```

- [ ] **Step 2: Add column-attnum resolver query to `shared.rs`**

```rust
/// Resolve a published table's column attnums to names. Joined per-row.
pub const PUBLICATION_COLUMN_NAMES_QUERY: &str = "\
    SELECT attnum, attname::text \
    FROM pg_attribute \
    WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped \
    ORDER BY attnum";
```

- [ ] **Step 3: Override in `crates/pgevolve-core/src/catalog/queries/pg14.rs`**

PG 14 lacks `prqual` (added PG 15), `prattrs` (added PG 15), and `pg_publication_namespace` (added PG 15).

```rust
pub const PUBLICATION_REL_QUERY_PG14: &str = "\
    SELECT \
        pr.prpubid::bigint AS pub_oid, \
        ns.nspname::text AS schema, \
        c.relname::text AS table_name, \
        NULL::text AS row_filter, \
        NULL::int2[] AS col_attnums, \
        c.oid::bigint AS rel_oid \
    FROM pg_publication_rel pr \
    JOIN pg_class c ON c.oid = pr.prrelid \
    JOIN pg_namespace ns ON ns.oid = c.relnamespace \
    ORDER BY pr.prpubid, ns.nspname, c.relname";

/// PG 14 has no pg_publication_namespace. Returns no rows.
pub const PUBLICATION_NAMESPACE_QUERY_PG14: &str = "SELECT NULL::bigint AS pub_oid, NULL::text AS schema WHERE false";
```

`PUBLICATIONS_QUERY` works as-is on PG 14 (all referenced columns exist since PG 10–13 era; `pubviaroot` added PG 13).

### Task 5.2: Add `CatalogQuery` variants

- [ ] **Step 1: Find the existing `CatalogQuery` enum**

```bash
grep -n "pub enum CatalogQuery" crates/pgevolve-core/src/catalog/*.rs
```

Add three new variants and the per-version dispatch in `crates/pgevolve-core/src/catalog/queries/mod.rs`:

```rust
    Publications,
    PublicationRel,
    PublicationNamespace,
```

In whichever per-version dispatch function exists (mirror how `Indexes` does it):

```rust
            CatalogQuery::Publications => shared::PUBLICATIONS_QUERY,
            CatalogQuery::PublicationRel => match major {
                14 => pg14::PUBLICATION_REL_QUERY_PG14,
                _ => shared::PUBLICATION_REL_QUERY,
            },
            CatalogQuery::PublicationNamespace => match major {
                14 => pg14::PUBLICATION_NAMESPACE_QUERY_PG14,
                _ => shared::PUBLICATION_NAMESPACE_QUERY,
            },
```

### Task 5.3: Decoder module

- [ ] **Step 1: Create `crates/pgevolve-core/src/catalog/publications.rs`**

```rust
//! Decode pg_catalog publication rows into `Publication` IR.
//!
//! Three queries:
//!   - `pg_publication`           → name + owner + scope flag + publish + comment
//!   - `pg_publication_rel`       → per-table membership (+ row filter PG15+, + column list PG15+)
//!   - `pg_publication_namespace` → per-schema membership (PG15+)
//!
//! Row filter text is fed through `NormalizedExpr::from_sql` so source-side
//! and catalog-side canonical forms compare equal.

use std::collections::BTreeMap;

use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::publication::{PublicationScope, PublishKinds, PublishedTable};

/// Decoded `pg_publication` row plus the not-yet-assembled scope inputs.
pub struct PartialPublication {
    pub oid: i64,
    pub name: Identifier,
    pub owner: Option<Identifier>,
    pub all_tables: bool,
    pub publish: PublishKinds,
    pub publish_via_partition_root: bool,
    pub comment: Option<String>,
}

pub fn decode_publication_row(row: &Row) -> Result<PartialPublication, CatalogError> {
    let name_str: String = row.get_text("name")?;
    let owner_str: String = row.get_text("owner")?;
    let comment_str: String = row.get_text("comment")?;
    Ok(PartialPublication {
        oid: row.get_i64("oid")?,
        name: Identifier::from_unquoted(&name_str)
            .map_err(|e| CatalogError::InvalidIdentifier(name_str.clone(), e.to_string()))?,
        owner: if owner_str.is_empty() {
            None
        } else {
            Some(
                Identifier::from_unquoted(&owner_str)
                    .map_err(|e| CatalogError::InvalidIdentifier(owner_str.clone(), e.to_string()))?,
            )
        },
        all_tables: row.get_bool("all_tables")?,
        publish: PublishKinds {
            insert: row.get_bool("pub_insert")?,
            update: row.get_bool("pub_update")?,
            delete: row.get_bool("pub_delete")?,
            truncate: row.get_bool("pub_truncate")?,
        },
        publish_via_partition_root: row.get_bool("publish_via_partition_root")?,
        comment: if comment_str.is_empty() {
            None
        } else {
            Some(comment_str)
        },
    })
}

/// One `pg_publication_rel` row decoded but not yet attached to its parent
/// publication (caller groups by `pub_oid`).
pub struct PartialPublicationRel {
    pub pub_oid: i64,
    pub qname: QualifiedName,
    pub row_filter_sql: Option<String>,
    pub col_attnums: Option<Vec<i16>>,
    pub rel_oid: i64,
}

pub fn decode_publication_rel_row(row: &Row) -> Result<PartialPublicationRel, CatalogError> {
    let schema: String = row.get_text("schema")?;
    let table: String = row.get_text("table_name")?;
    Ok(PartialPublicationRel {
        pub_oid: row.get_i64("pub_oid")?,
        qname: QualifiedName::new(
            Identifier::from_unquoted(&schema)
                .map_err(|e| CatalogError::InvalidIdentifier(schema.clone(), e.to_string()))?,
            Identifier::from_unquoted(&table)
                .map_err(|e| CatalogError::InvalidIdentifier(table.clone(), e.to_string()))?,
        ),
        row_filter_sql: row.get_optional_text("row_filter")?,
        col_attnums: row.get_optional_int2_array("col_attnums")?,
        rel_oid: row.get_i64("rel_oid")?,
    })
}

pub struct PartialPublicationNamespace {
    pub pub_oid: i64,
    pub schema: Identifier,
}

pub fn decode_publication_namespace_row(row: &Row) -> Result<PartialPublicationNamespace, CatalogError> {
    let schema: String = row.get_text("schema")?;
    Ok(PartialPublicationNamespace {
        pub_oid: row.get_i64("pub_oid")?,
        schema: Identifier::from_unquoted(&schema)
            .map_err(|e| CatalogError::InvalidIdentifier(schema, e.to_string()))?,
    })
}

/// Convert a decoded row's column-attnum vec into resolved column names.
/// Caller queries `PUBLICATION_COLUMN_NAMES_QUERY` per `rel_oid` and passes
/// the resulting attnum → name map here.
pub fn resolve_column_names(
    attnums: &[i16],
    attname_by_attnum: &BTreeMap<i16, String>,
) -> Result<Vec<Identifier>, CatalogError> {
    attnums
        .iter()
        .map(|n| {
            let name = attname_by_attnum
                .get(n)
                .ok_or_else(|| CatalogError::DecodeError(format!("attnum {n} not in pg_attribute")))?;
            Identifier::from_unquoted(name)
                .map_err(|e| CatalogError::InvalidIdentifier(name.clone(), e.to_string()))
        })
        .collect()
}

/// Build a `PublishedTable` from a decoded rel row and (already-resolved)
/// column names. Row filter is fed through `NormalizedExpr::from_sql`.
pub fn assemble_published_table(
    rel: PartialPublicationRel,
    columns: Option<Vec<Identifier>>,
) -> Result<PublishedTable, CatalogError> {
    let row_filter = rel
        .row_filter_sql
        .map(|sql| {
            NormalizedExpr::from_sql(&sql)
                .map_err(|e| CatalogError::DecodeError(format!("row filter parse: {e}")))
        })
        .transpose()?;
    Ok(PublishedTable {
        qname: rel.qname,
        row_filter,
        columns,
    })
}

/// Construct `PublicationScope` from grouped rel/namespace rows.
pub fn build_scope(
    all_tables: bool,
    rels: Vec<PartialPublicationRel>,
    column_resolver: impl Fn(&PartialPublicationRel) -> Result<Option<Vec<Identifier>>, CatalogError>,
    namespaces: Vec<PartialPublicationNamespace>,
) -> Result<PublicationScope, CatalogError> {
    if all_tables {
        return Ok(PublicationScope::AllTables);
    }
    let mut tables = Vec::with_capacity(rels.len());
    for r in rels {
        let cols = column_resolver(&r)?;
        tables.push(assemble_published_table(r, cols)?);
    }
    let schemas = namespaces.into_iter().map(|n| n.schema).collect();
    Ok(PublicationScope::Selective { schemas, tables })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_columns_handles_simple_case() {
        let map = BTreeMap::from([(1i16, "id".to_string()), (2i16, "name".to_string())]);
        let attnums = vec![2, 1];
        let cols = resolve_column_names(&attnums, &map).unwrap();
        assert_eq!(cols.iter().map(|i| i.as_str()).collect::<Vec<_>>(), vec!["name", "id"]);
    }

    #[test]
    fn resolve_columns_fails_on_missing_attnum() {
        let map = BTreeMap::from([(1i16, "id".to_string())]);
        let err = resolve_column_names(&[3], &map).unwrap_err();
        assert!(format!("{err}").contains("attnum 3"));
    }

    #[test]
    fn build_scope_all_tables_ignores_rels_and_namespaces() {
        let scope = build_scope(true, vec![], |_| Ok(None), vec![]).unwrap();
        assert!(matches!(scope, PublicationScope::AllTables));
    }
}
```

If `Row::get_optional_int2_array` / `get_optional_text` don't exist, add them to `rows.rs` (5-line wrappers around `tokio_postgres::Row::try_get`).

- [ ] **Step 2: Create `crates/pgevolve-core/src/catalog/assemble/publications.rs`**

```rust
//! Orchestrate the three publication queries into `Vec<Publication>`.

use std::collections::BTreeMap;

use crate::catalog::error::CatalogError;
use crate::catalog::publications::{
    PartialPublication, PartialPublicationNamespace, PartialPublicationRel, build_scope,
    decode_publication_namespace_row, decode_publication_rel_row, decode_publication_row,
};
use crate::catalog::queries::CatalogQuery;
use crate::catalog::querier::CatalogQuerier;
use crate::identifier::Identifier;
use crate::ir::publication::Publication;

pub fn assemble_publications(
    q: &dyn CatalogQuerier,
) -> Result<Vec<Publication>, CatalogError> {
    let pub_rows = q.run(CatalogQuery::Publications)?;
    let rel_rows = q.run(CatalogQuery::PublicationRel)?;
    let ns_rows = q.run(CatalogQuery::PublicationNamespace)?;

    // Group by pub_oid.
    let mut rels_by_oid: BTreeMap<i64, Vec<PartialPublicationRel>> = BTreeMap::new();
    let mut all_rels: Vec<PartialPublicationRel> = Vec::with_capacity(rel_rows.len());
    for r in &rel_rows {
        let pr = decode_publication_rel_row(r)?;
        rels_by_oid.entry(pr.pub_oid).or_default().push(pr.clone_shallow());
        all_rels.push(pr);
    }
    let mut ns_by_oid: BTreeMap<i64, Vec<PartialPublicationNamespace>> = BTreeMap::new();
    for r in &ns_rows {
        let pn = decode_publication_namespace_row(r)?;
        ns_by_oid.entry(pn.pub_oid).or_default().push(pn);
    }

    // Resolve column names for each rel row that has col_attnums.
    let mut cols_by_rel_oid: BTreeMap<i64, Vec<Identifier>> = BTreeMap::new();
    for r in &all_rels {
        if let Some(attnums) = &r.col_attnums {
            let map = q.run_with_params(
                "SELECT attnum, attname::text FROM pg_attribute WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped ORDER BY attnum",
                &[&r.rel_oid],
            )?;
            let attname_by_attnum: BTreeMap<i16, String> = map
                .iter()
                .map(|row| {
                    let n = row.get_i16("attnum")?;
                    let name = row.get_text("attname")?;
                    Ok::<_, CatalogError>((n, name))
                })
                .collect::<Result<_, _>>()?;
            let names = crate::catalog::publications::resolve_column_names(attnums, &attname_by_attnum)?;
            cols_by_rel_oid.insert(r.rel_oid, names);
        }
    }

    // Build each Publication.
    let mut publications = Vec::with_capacity(pub_rows.len());
    for row in &pub_rows {
        let pp: PartialPublication = decode_publication_row(row)?;
        let rels = rels_by_oid.remove(&pp.oid).unwrap_or_default();
        let nss = ns_by_oid.remove(&pp.oid).unwrap_or_default();
        let scope = build_scope(
            pp.all_tables,
            rels,
            |r| Ok(cols_by_rel_oid.get(&r.rel_oid).cloned()),
            nss,
        )?;
        publications.push(Publication {
            name: pp.name,
            scope,
            publish: pp.publish,
            publish_via_partition_root: pp.publish_via_partition_root,
            owner: pp.owner,
            comment: pp.comment,
        });
    }

    Ok(publications)
}
```

`clone_shallow` is a small `impl PartialPublicationRel` method:

```rust
impl PartialPublicationRel {
    pub fn clone_shallow(&self) -> Self {
        Self {
            pub_oid: self.pub_oid,
            qname: self.qname.clone(),
            row_filter_sql: self.row_filter_sql.clone(),
            col_attnums: self.col_attnums.clone(),
            rel_oid: self.rel_oid,
        }
    }
}
```

If `CatalogQuerier::run_with_params` doesn't exist, add it (the test querier mirrors this pattern; see how `assemble/indexes.rs` does per-row follow-up queries).

- [ ] **Step 3: Wire into `read_catalog`**

In `crates/pgevolve-core/src/catalog/mod.rs`'s `read_catalog`, add:

```rust
catalog.publications = crate::catalog::assemble::publications::assemble_publications(querier)?;
```

After the existing object-kind assemblers, before the canonicalize call.

- [ ] **Step 4: Docker integration test**

Create `crates/pgevolve-core/tests/publication_round_trip.rs`:

```rust
//! Round-trip: CREATE PUBLICATION → read back → assert equal IR.
//! Requires Docker (tier-3 pattern; skips cleanly when docker unavailable).

#![cfg(all(test, feature = "testkit"))]

use anyhow::Result;
use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::publication::{PublicationScope, PublishKinds};
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};
use pgevolve_testkit::PgCatalogQuerier;

#[tokio::test(flavor = "multi_thread")]
async fn read_publication_for_all_tables() -> Result<()> {
    if !docker_available() {
        return Ok(());
    }
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let client = pg.connect().await?;
    client.batch_execute("CREATE SCHEMA app;").await?;
    client.batch_execute("CREATE PUBLICATION p FOR ALL TABLES;").await?;

    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(
        vec![Identifier::from_unquoted("app").unwrap()],
        vec![],
    )?;
    let (catalog, _) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter)).await??;
    assert_eq!(catalog.publications.len(), 1);
    assert_eq!(catalog.publications[0].name.as_str(), "p");
    assert!(matches!(catalog.publications[0].scope, PublicationScope::AllTables));
    assert_eq!(catalog.publications[0].publish, PublishKinds::pg_default());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn read_publication_for_explicit_tables_with_filter() -> Result<()> {
    if !docker_available() {
        return Ok(());
    }
    // Skip on PG14 (row filter requires PG15+).
    if default_pg_version() < 15 {
        return Ok(());
    }
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let client = pg.connect().await?;
    client.batch_execute(
        "CREATE SCHEMA app; \
         CREATE TABLE app.orders (id bigint PRIMARY KEY, status text);",
    ).await?;
    client.batch_execute(
        "CREATE PUBLICATION p FOR TABLE app.orders (id) WHERE (status = 'active');",
    ).await?;

    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(
        vec![Identifier::from_unquoted("app").unwrap()],
        vec![],
    )?;
    let (catalog, _) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter)).await??;
    let pub_ = &catalog.publications[0];
    let PublicationScope::Selective { tables, schemas } = &pub_.scope else {
        panic!("expected Selective");
    };
    assert!(schemas.is_empty());
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].qname.name.as_str(), "orders");
    assert_eq!(
        tables[0].columns.as_ref().unwrap().iter().map(|i| i.as_str()).collect::<Vec<_>>(),
        vec!["id"],
    );
    assert!(tables[0].row_filter.is_some());
    Ok(())
}
```

- [ ] **Step 5: Build + run**

```bash
cargo build -p pgevolve-core
cargo test -p pgevolve-core --lib catalog::publications
cargo test -p pgevolve-core --test publication_round_trip
```

Expected: unit tests pass; integration tests pass against the running Docker.

- [ ] **Step 6: Commit**

```bash
git add crates/pgevolve-core/src/catalog/ crates/pgevolve-core/tests/publication_round_trip.rs
git commit -m "$(cat <<'EOF'
feat(catalog): read publications from pg_catalog

Three joined queries:
  - pg_publication           (name, owner, mode flag, publish, comment)
  - pg_publication_rel       (per-table, + row filter + column list PG15+)
  - pg_publication_namespace (per-schema; PG15+ only)

PG 14 query variants strip the PG15+ columns and skip the namespace
query entirely (it returns zero rows). Row filter text canonicalizes
through NormalizedExpr::from_sql so source/catalog comparisons are
case- and whitespace-insensitive (same canon as CHECK / USING).

Tier-3 round-trip test covers FOR ALL TABLES and (PG15+) explicit
FOR TABLE with row filter + column list.

Stage 5 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 6 — Source parser

Parse `CREATE PUBLICATION` and `ALTER PUBLICATION` SQL into the `Publication` IR. Fold the inline scope + per-publication ALTER statements into one canonical record per publication name.

**Files created:** `crates/pgevolve-core/src/parse/builder/publication_stmt.rs`.
**Files modified:** `crates/pgevolve-core/src/parse/builder/mod.rs`.

**Spec ref:** "Source surface".

### Task 6.1: Create the parser module

- [ ] **Step 1: Sketch the module**

```rust
//! Parser for CREATE PUBLICATION and ALTER PUBLICATION statements.
//!
//! pg_query emits PublicationStmt for CREATE, AlterPublicationStmt for ALTER.
//! We fold both into one Publication per name (last-write-wins per attribute,
//! same as v0.3.3 reloptions folds CREATE … WITH (…) with later ALTER … SET).

use std::collections::BTreeMap;

use pg_query::protobuf::*;

use crate::error::ParseError;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};

pub fn parse_create_publication(
    stmt: &CreatePublicationStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Publication>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.pubname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.pubname.clone(), e.to_string()))?;

    if existing.contains_key(&name) {
        return Err(ParseError::DuplicatePublication(name, source_loc));
    }

    // Parse the scope:
    //   - stmt.for_all_tables = true → AllTables
    //   - stmt.pubobjects has PublicationObjSpec entries → Selective
    let scope = if stmt.for_all_tables {
        if !stmt.pubobjects.is_empty() {
            return Err(ParseError::PublicationAllTablesWithObjects(name, source_loc));
        }
        PublicationScope::AllTables
    } else {
        parse_selective_scope(&stmt.pubobjects, &name, source_loc)?
    };

    // Parse the WITH (...) options.
    let (publish, via_root) = parse_publication_options(&stmt.options, &name, source_loc)?;

    existing.insert(
        name.clone(),
        Publication {
            name,
            scope,
            publish: publish.unwrap_or_else(PublishKinds::pg_default),
            publish_via_partition_root: via_root.unwrap_or(false),
            owner: None,
            comment: None,
        },
    );
    Ok(())
}

pub fn parse_alter_publication(
    stmt: &AlterPublicationStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<Identifier, Publication>,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&stmt.pubname)
        .map_err(|e| ParseError::InvalidIdentifier(stmt.pubname.clone(), e.to_string()))?;

    // RENAME is not supported.
    if !stmt.options.is_empty() && is_rename(&stmt.options) {
        return Err(ParseError::PublicationRenameNotSupported(name, source_loc));
    }

    let pub_ = existing
        .get_mut(&name)
        .ok_or_else(|| ParseError::AlterPublicationBeforeCreate(name.clone(), source_loc))?;

    // Apply the alter:
    //   - Action 0 (ADD) → add tables/schemas to Selective
    //   - Action 1 (SET) → replace entire scope
    //   - Action 2 (DROP) → remove tables/schemas from Selective
    //   - options non-empty → update publish / publish_via_partition_root
    if !stmt.pubobjects.is_empty() {
        apply_scope_change(stmt.action, &stmt.pubobjects, pub_, source_loc)?;
    }
    if !stmt.options.is_empty() {
        let (publish, via_root) = parse_publication_options(&stmt.options, &name, source_loc)?;
        if let Some(k) = publish {
            pub_.publish = k;
        }
        if let Some(v) = via_root {
            pub_.publish_via_partition_root = v;
        }
    }
    Ok(())
}

// --- helpers --------------------------------------------------------------

fn parse_selective_scope(
    objs: &[Node],
    name: &Identifier,
    loc: SourceLocation,
) -> Result<PublicationScope, ParseError> {
    let mut tables: Vec<PublishedTable> = Vec::new();
    let mut schemas = std::collections::BTreeSet::new();
    for obj in objs {
        let spec = obj.node.as_ref()
            .and_then(|n| if let node::Node::PublicationObject(s) = n { Some(s.as_ref()) } else { None })
            .ok_or_else(|| ParseError::PublicationObjectMalformed(name.clone(), loc))?;
        match spec.pubobjtype {
            // PUBLICATIONOBJ_TABLE
            1 => {
                let pt = extract_table_spec(spec, name, loc)?;
                tables.push(pt);
            }
            // PUBLICATIONOBJ_TABLES_IN_SCHEMA
            2 => {
                let sn = extract_schema_name(spec, name, loc)?;
                schemas.insert(sn);
            }
            // PUBLICATIONOBJ_TABLES_IN_CUR_SCHEMA — current schema; not declarative
            3 => return Err(ParseError::PublicationCurrentSchemaForm(name.clone(), loc)),
            other => return Err(ParseError::UnknownPublicationObjectType(other, name.clone(), loc)),
        }
    }
    if schemas.is_empty() && tables.is_empty() {
        return Err(ParseError::EmptyPublicationScope(name.clone(), loc));
    }
    Ok(PublicationScope::Selective { schemas, tables })
}

fn extract_table_spec(
    spec: &PublicationObjSpec,
    pub_name: &Identifier,
    loc: SourceLocation,
) -> Result<PublishedTable, ParseError> {
    let pt = spec.pubtable.as_ref()
        .ok_or_else(|| ParseError::PublicationObjectMalformed(pub_name.clone(), loc))?;
    let relation = pt.relation.as_ref()
        .ok_or_else(|| ParseError::PublicationObjectMalformed(pub_name.clone(), loc))?;
    let schema = if relation.schemaname.is_empty() {
        return Err(ParseError::UnqualifiedPublicationTable(pub_name.clone(), loc));
    } else {
        Identifier::from_unquoted(&relation.schemaname)
            .map_err(|e| ParseError::InvalidIdentifier(relation.schemaname.clone(), e.to_string()))?
    };
    let table = Identifier::from_unquoted(&relation.relname)
        .map_err(|e| ParseError::InvalidIdentifier(relation.relname.clone(), e.to_string()))?;
    let qname = QualifiedName::new(schema, table);

    // Column list (PG15+).
    let columns = if pt.columns.is_empty() {
        None
    } else {
        let cols: Result<Vec<_>, _> = pt.columns.iter().map(|c| {
            let s = string_value(c)?;
            Identifier::from_unquoted(&s)
                .map_err(|e| ParseError::InvalidIdentifier(s, e.to_string()))
        }).collect();
        Some(cols?)
    };

    // Row filter (PG15+).
    let row_filter = if let Some(where_node) = &pt.where_clause {
        Some(NormalizedExpr::from_node(where_node)
            .map_err(|e| ParseError::PublicationFilterParse(pub_name.clone(), qname.clone(), e.to_string(), loc))?)
    } else {
        None
    };

    Ok(PublishedTable { qname, row_filter, columns })
}

fn parse_publication_options(
    options: &[Node],
    name: &Identifier,
    loc: SourceLocation,
) -> Result<(Option<PublishKinds>, Option<bool>), ParseError> {
    let mut publish: Option<PublishKinds> = None;
    let mut via_root: Option<bool> = None;
    for opt in options {
        let def_elem = opt.node.as_ref()
            .and_then(|n| if let node::Node::DefElem(d) = n { Some(d.as_ref()) } else { None })
            .ok_or_else(|| ParseError::PublicationOptionMalformed(name.clone(), loc))?;
        match def_elem.defname.as_str() {
            "publish" => {
                let s = def_elem_text(def_elem)?;
                publish = Some(parse_publish_string(&s, name, loc)?);
            }
            "publish_via_partition_root" => {
                via_root = Some(def_elem_bool(def_elem)?);
            }
            other => return Err(ParseError::UnknownPublicationOption(other.to_string(), name.clone(), loc)),
        }
    }
    Ok((publish, via_root))
}

fn parse_publish_string(s: &str, name: &Identifier, loc: SourceLocation) -> Result<PublishKinds, ParseError> {
    let mut k = PublishKinds { insert: false, update: false, delete: false, truncate: false };
    for part in s.split(',') {
        match part.trim().to_ascii_lowercase().as_str() {
            "insert" => k.insert = true,
            "update" => k.update = true,
            "delete" => k.delete = true,
            "truncate" => k.truncate = true,
            other => return Err(ParseError::UnknownPublishKind(other.to_string(), name.clone(), loc)),
        }
    }
    if k.is_empty() {
        return Err(ParseError::EmptyPublishBitset(name.clone(), loc));
    }
    Ok(k)
}

// ... apply_scope_change, extract_schema_name, string_value, def_elem_text,
//     def_elem_bool, is_rename — mechanical helpers, mirror policy_stmt.rs
//     and reloptions parse builders.

#[cfg(test)]
mod tests {
    // 8–12 tests covering:
    // - CREATE PUBLICATION p FOR ALL TABLES
    // - CREATE PUBLICATION p FOR TABLE app.t
    // - CREATE PUBLICATION p FOR TABLE app.t (col1, col2) WHERE (filter)
    // - CREATE PUBLICATION p FOR TABLES IN SCHEMA app
    // - CREATE PUBLICATION p WITH (publish = 'insert, update', publish_via_partition_root = true)
    // - ALTER PUBLICATION p ADD TABLE app.t (folded with prior CREATE)
    // - ALTER PUBLICATION p DROP TABLE app.t
    // - ALTER PUBLICATION p SET (publish = 'insert')
    // - ALTER PUBLICATION p RENAME TO q → ParseError::PublicationRenameNotSupported
    // - CREATE PUBLICATION p FOR ALL TABLES, TABLE app.t → ParseError::PublicationAllTablesWithObjects
    // - CREATE PUBLICATION p (no scope) → ParseError::EmptyPublicationScope
    // - CREATE PUBLICATION p WITH (publish = 'bogus') → ParseError::UnknownPublishKind
    // Each test parses via parse_directory_with_inline_sql helper and asserts the resulting Publication.
}
```

The exact pg_query field names (`pubname`, `for_all_tables`, `pubobjects`, `pubobjtype`, `pubtable`, `where_clause`, etc.) need to be confirmed against the pg_query 6.x protobuf. Read `parse/builder/policy_stmt.rs` and `parse/builder/alter_table_stmt.rs` as templates — they use the same parse-time idioms.

`ParseError` variants to add (mirror existing variant style):
- `DuplicatePublication(Identifier, SourceLocation)`
- `PublicationAllTablesWithObjects(Identifier, SourceLocation)`
- `PublicationObjectMalformed(Identifier, SourceLocation)`
- `PublicationCurrentSchemaForm(Identifier, SourceLocation)`
- `UnknownPublicationObjectType(i32, Identifier, SourceLocation)`
- `UnqualifiedPublicationTable(Identifier, SourceLocation)`
- `PublicationFilterParse(Identifier, QualifiedName, String, SourceLocation)`
- `PublicationOptionMalformed(Identifier, SourceLocation)`
- `UnknownPublicationOption(String, Identifier, SourceLocation)`
- `UnknownPublishKind(String, Identifier, SourceLocation)`
- `EmptyPublishBitset(Identifier, SourceLocation)`
- `EmptyPublicationScope(Identifier, SourceLocation)`
- `PublicationRenameNotSupported(Identifier, SourceLocation)`
- `AlterPublicationBeforeCreate(Identifier, SourceLocation)`

- [ ] **Step 2: Wire into `parse/builder/mod.rs` dispatch**

```rust
            node::Node::CreatePublicationStmt(s) => {
                publication_stmt::parse_create_publication(s, loc, &mut publications)?;
            }
            node::Node::AlterPublicationStmt(s) => {
                publication_stmt::parse_alter_publication(s, loc, &mut publications)?;
            }
```

Plus add `mut publications: BTreeMap<Identifier, Publication>` to the parser state and copy `publications.into_values().collect()` into `catalog.publications` at the end.

- [ ] **Step 3: Build + test**

```bash
cargo test -p pgevolve-core --lib parse::builder::publication_stmt
```

Expected: 12 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/pgevolve-core/src/parse/ crates/pgevolve-core/src/error.rs
git commit -m "$(cat <<'EOF'
feat(parse): CREATE PUBLICATION and ALTER PUBLICATION

Folds CREATE PUBLICATION's inline scope + WITH (...) options and
subsequent ALTER PUBLICATION add/drop/set operations into one
canonical Publication record per name. Mirrors the v0.3.3 reloptions
fold of CREATE TABLE WITH (...) + ALTER TABLE SET (...).

Rejects: ALTER PUBLICATION p RENAME (no renames in pgevolve);
FOR ALL TABLES combined with FOR TABLE / FOR TABLES IN SCHEMA;
empty scope clause; FOR TABLES IN CURRENT SCHEMA form (not
declarative).

Stage 6 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 7 — Differ

Granular per-publication diff with 11 new Change variants.

**Files created:** `crates/pgevolve-core/src/diff/publications.rs`.
**Files modified:** `crates/pgevolve-core/src/diff/change.rs`, `crates/pgevolve-core/src/diff/mod.rs`, `crates/pgevolve-core/src/diff/owner_op.rs`.

### Task 7.1: Add Change variants

- [ ] **Step 1: Extend `crates/pgevolve-core/src/diff/change.rs`**

```rust
    /// `CREATE PUBLICATION ...`
    CreatePublication(crate::ir::publication::Publication),
    /// `DROP PUBLICATION ...` — destructive.
    DropPublication { name: crate::identifier::Identifier },
    /// `DROP PUBLICATION old; CREATE PUBLICATION new;` — destructive; used
    /// when the publication's scope mode switches (AllTables ↔ Selective).
    ReplacePublication {
        from: crate::ir::publication::Publication,
        to: crate::ir::publication::Publication,
    },
    /// `ALTER PUBLICATION p ADD TABLE x [(cols)] [WHERE (filter)]`
    AlterPublicationAddTable {
        publication: crate::identifier::Identifier,
        table: crate::ir::publication::PublishedTable,
    },
    /// `ALTER PUBLICATION p DROP TABLE x`
    AlterPublicationDropTable {
        publication: crate::identifier::Identifier,
        qname: crate::identifier::QualifiedName,
    },
    /// `ALTER PUBLICATION p SET TABLE x (cols) WHERE (filter)`
    AlterPublicationSetTable {
        publication: crate::identifier::Identifier,
        table: crate::ir::publication::PublishedTable,
    },
    /// `ALTER PUBLICATION p ADD TABLES IN SCHEMA s` (PG15+)
    AlterPublicationAddSchema {
        publication: crate::identifier::Identifier,
        schema: crate::identifier::Identifier,
    },
    /// `ALTER PUBLICATION p DROP TABLES IN SCHEMA s` (PG15+)
    AlterPublicationDropSchema {
        publication: crate::identifier::Identifier,
        schema: crate::identifier::Identifier,
    },
    /// `ALTER PUBLICATION p SET (publish = '...')`
    AlterPublicationSetPublish {
        publication: crate::identifier::Identifier,
        kinds: crate::ir::publication::PublishKinds,
    },
    /// `ALTER PUBLICATION p SET (publish_via_partition_root = ...)`
    AlterPublicationSetViaRoot {
        publication: crate::identifier::Identifier,
        value: bool,
    },
    /// `COMMENT ON PUBLICATION p IS '...'`
    CommentOnPublication {
        name: crate::identifier::Identifier,
        comment: Option<String>,
    },
```

- [ ] **Step 2: Add `OwnerObjectKind::Publication`**

In `crates/pgevolve-core/src/diff/owner_op.rs`:

```rust
    Publication,
```

Plus the `Display` arm:

```rust
            OwnerObjectKind::Publication => write!(f, "PUBLICATION"),
```

### Task 7.2: Implement the differ

- [ ] **Step 1: Create `crates/pgevolve-core/src/diff/publications.rs`**

```rust
//! Differ for publications. Pair by name; per-publication granular diff.

use std::collections::BTreeMap;

use crate::diff::change::{Change, ChangeSet};
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, OwnerObjectKind};
use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::publication::{Publication, PublicationScope, PublishedTable};

pub fn diff_publications(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&Identifier, &Publication> =
        target.publications.iter().map(|p| (&p.name, p)).collect();
    let source_map: BTreeMap<&Identifier, &Publication> =
        source.publications.iter().map(|p| (&p.name, p)).collect();

    // Creates: in source but not in target.
    for (name, src) in &source_map {
        if !target_map.contains_key(name) {
            out.push(
                Change::CreatePublication((*src).clone()),
                Destructiveness::Safe,
            );
        }
    }

    // Drops: in target but not in source.
    for (name, _) in &target_map {
        if !source_map.contains_key(name) {
            // Lenient: no auto-drop. Surfaces via unmanaged-publication lint.
            // We do NOT emit Change::DropPublication on missing-in-source.
            // To drop a managed publication, the user must explicitly remove
            // it AND issue the DROP out-of-band, or accept the warning.
            //
            // NOTE: this matches the lenient drift pattern shared with
            // unmanaged-grant / unmanaged-policy / unmanaged-reloption.
            let _ = name;
        }
    }

    // Modifies: in both.
    for (name, src) in &source_map {
        let Some(tgt) = target_map.get(name) else { continue; };
        diff_one_publication(tgt, src, out);
    }
}

fn diff_one_publication(target: &Publication, source: &Publication, out: &mut ChangeSet) {
    // Mode mismatch → ReplacePublication.
    let target_mode = std::mem::discriminant(&target.scope);
    let source_mode = std::mem::discriminant(&source.scope);
    if target_mode != source_mode {
        out.push(
            Change::ReplacePublication {
                from: target.clone(),
                to: source.clone(),
            },
            Destructiveness::RequiresApproval {
                reason: format!("publication {} mode swap (AllTables ↔ Selective)", source.name),
            },
        );
        return;
    }

    // Same mode. For Selective, do granular table/schema diff.
    if let (
        PublicationScope::Selective { schemas: t_schemas, tables: t_tables },
        PublicationScope::Selective { schemas: s_schemas, tables: s_tables },
    ) = (&target.scope, &source.scope)
    {
        diff_selective_tables(&source.name, t_tables, s_tables, out);
        diff_selective_schemas(&source.name, t_schemas, s_schemas, out);
    }

    // Per-publication scalar diffs.
    if target.publish != source.publish {
        out.push(
            Change::AlterPublicationSetPublish {
                publication: source.name.clone(),
                kinds: source.publish,
            },
            Destructiveness::Safe,
        );
    }
    if target.publish_via_partition_root != source.publish_via_partition_root {
        out.push(
            Change::AlterPublicationSetViaRoot {
                publication: source.name.clone(),
                value: source.publish_via_partition_root,
            },
            Destructiveness::Safe,
        );
    }
    if target.comment != source.comment {
        out.push(
            Change::CommentOnPublication {
                name: source.name.clone(),
                comment: source.comment.clone(),
            },
            Destructiveness::Safe,
        );
    }

    // Owner: lenient (v0.3.1 pattern).
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        let from = target.owner.clone().unwrap_or_else(|| {
            Identifier::from_unquoted("__unknown_owner__").expect("literal valid")
        });
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Publication,
                qname: crate::identifier::QualifiedName::new(
                    Identifier::from_unquoted("__cluster__").expect("literal valid"),
                    source.name.clone(),
                ),
                signature: String::new(),
                from,
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

fn diff_selective_tables(
    pub_name: &Identifier,
    target_tables: &[PublishedTable],
    source_tables: &[PublishedTable],
    out: &mut ChangeSet,
) {
    let t_map: BTreeMap<_, _> = target_tables.iter().map(|t| (&t.qname, t)).collect();
    let s_map: BTreeMap<_, _> = source_tables.iter().map(|t| (&t.qname, t)).collect();

    // Added.
    for (qname, t) in &s_map {
        if !t_map.contains_key(qname) {
            out.push(
                Change::AlterPublicationAddTable {
                    publication: pub_name.clone(),
                    table: (*t).clone(),
                },
                Destructiveness::Safe,
            );
        }
    }
    // Dropped.
    for (qname, _) in &t_map {
        if !s_map.contains_key(qname) {
            out.push(
                Change::AlterPublicationDropTable {
                    publication: pub_name.clone(),
                    qname: (*qname).clone(),
                },
                Destructiveness::Safe,
            );
        }
    }
    // Changed (in both, but row_filter or columns differ).
    for (qname, src_table) in &s_map {
        let Some(tgt_table) = t_map.get(qname) else { continue; };
        if tgt_table.row_filter != src_table.row_filter || tgt_table.columns != src_table.columns {
            out.push(
                Change::AlterPublicationSetTable {
                    publication: pub_name.clone(),
                    table: (*src_table).clone(),
                },
                Destructiveness::Safe,
            );
        }
    }
}

fn diff_selective_schemas(
    pub_name: &Identifier,
    target_schemas: &std::collections::BTreeSet<Identifier>,
    source_schemas: &std::collections::BTreeSet<Identifier>,
    out: &mut ChangeSet,
) {
    for s in source_schemas.difference(target_schemas) {
        out.push(
            Change::AlterPublicationAddSchema {
                publication: pub_name.clone(),
                schema: s.clone(),
            },
            Destructiveness::Safe,
        );
    }
    for s in target_schemas.difference(source_schemas) {
        out.push(
            Change::AlterPublicationDropSchema {
                publication: pub_name.clone(),
                schema: s.clone(),
            },
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    // 10+ tests covering each Change variant emission path.
}
```

- [ ] **Step 2: Wire into top-level `diff` in `crates/pgevolve-core/src/diff/mod.rs`**

```rust
crate::diff::publications::diff_publications(target, source, &mut changes);
```

After the other per-object-kind diff calls.

- [ ] **Step 3: Build + test**

```bash
cargo test -p pgevolve-core --lib diff::publications
cargo test -p pgevolve-core --lib diff
```

Expected: new tests pass; existing diff tests stay green.

- [ ] **Step 4: Add 11 stub emit arms in `plan/rewrite/mod.rs`**

(Real emit lands in Stage 8; for now, no-op stubs so the workspace compiles.)

```rust
            Change::CreatePublication(_)
            | Change::DropPublication { .. }
            | Change::ReplacePublication { .. }
            | Change::AlterPublicationAddTable { .. }
            | Change::AlterPublicationDropTable { .. }
            | Change::AlterPublicationSetTable { .. }
            | Change::AlterPublicationAddSchema { .. }
            | Change::AlterPublicationDropSchema { .. }
            | Change::AlterPublicationSetPublish { .. }
            | Change::AlterPublicationSetViaRoot { .. }
            | Change::CommentOnPublication { .. } => {
                // Stage 8 fills this in.
            }
```

Also add stubs in the 4 other Change consumers (mirror Stage 6 of v0.3.3 reloptions plan):
- `plan/ordering.rs::partition`
- `plan/ordering.rs::change_node`
- `commands/diff.rs::print_human`
- `commands/diff.rs::change_kind_name`

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/diff/ crates/pgevolve-core/src/plan/rewrite/mod.rs crates/pgevolve-core/src/plan/ordering.rs crates/pgevolve/src/commands/diff.rs
git commit -m "$(cat <<'EOF'
feat(diff): publications — 11 granular Change variants

Pair by name; per-publication granular diff. Mode mismatch
(AllTables ↔ Selective) emits ReplacePublication; same-mode Selective
diffs tables and schemas granularly (add/drop/set). Per-publication
scalars (publish, publish_via_partition_root, comment, owner) flow
through dedicated variants. Owner uses the v0.3.1 lenient pattern.

11 Change variants added; 4 stub arms in downstream Change consumers
let the workspace compile (real emit lands Stage 8).

Stage 7 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 8 — Render + emit + 11 StepKinds + dep edges

Fill in the SQL helpers, the StepKind variants, and the dep-graph edges. Replace Stage 7's stubs with real emit code.

**Files created:** `crates/pgevolve-core/src/plan/rewrite/publications.rs`.
**Files modified:** `crates/pgevolve-core/src/plan/raw_step.rs`, `crates/pgevolve-core/src/plan/plan.rs`, `crates/pgevolve-core/src/plan/rewrite/mod.rs`, `crates/pgevolve-core/src/plan/edges.rs`, `crates/pgevolve/src/commands/diff.rs`.

### Task 8.1: StepKind variants + kind_name

- [ ] **Step 1: Extend `crates/pgevolve-core/src/plan/raw_step.rs::StepKind`**

```rust
    CreatePublication,
    DropPublication,
    ReplacePublication,
    AlterPublicationAddTable,
    AlterPublicationDropTable,
    AlterPublicationSetTable,
    AlterPublicationAddSchema,
    AlterPublicationDropSchema,
    AlterPublicationSetPublish,
    AlterPublicationSetViaRoot,
    CommentOnPublication,
```

Extend the round-trip serialization test array with all 11.

- [ ] **Step 2: Extend `kind_name` / `parse_kind_name` in `crates/pgevolve-core/src/plan/plan.rs`**

```rust
    StepKind::CreatePublication              => "create_publication",
    StepKind::DropPublication                => "drop_publication",
    StepKind::ReplacePublication             => "replace_publication",
    StepKind::AlterPublicationAddTable       => "alter_publication_add_table",
    StepKind::AlterPublicationDropTable      => "alter_publication_drop_table",
    StepKind::AlterPublicationSetTable       => "alter_publication_set_table",
    StepKind::AlterPublicationAddSchema      => "alter_publication_add_schema",
    StepKind::AlterPublicationDropSchema     => "alter_publication_drop_schema",
    StepKind::AlterPublicationSetPublish     => "alter_publication_set_publish",
    StepKind::AlterPublicationSetViaRoot     => "alter_publication_set_via_root",
    StepKind::CommentOnPublication           => "comment_on_publication",
```

Mirror entries in `parse_kind_name`.

### Task 8.2: SQL helpers

- [ ] **Step 1: Create `crates/pgevolve-core/src/plan/rewrite/publications.rs`**

```rust
//! SQL rendering for publication operations.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};

/// `CREATE PUBLICATION p ... WITH (...);`
#[must_use]
pub fn create_publication(p: &Publication) -> String {
    let mut s = format!("CREATE PUBLICATION {}", p.name.render_sql());
    match &p.scope {
        PublicationScope::AllTables => s.push_str(" FOR ALL TABLES"),
        PublicationScope::Selective { schemas, tables } => {
            s.push_str(" FOR ");
            let mut first = true;
            if !tables.is_empty() {
                s.push_str("TABLE ");
                for t in tables {
                    if !first { s.push_str(", "); }
                    s.push_str(&render_published_table(t));
                    first = false;
                }
            }
            if !schemas.is_empty() {
                if !first { s.push_str(", "); }
                s.push_str("TABLES IN SCHEMA ");
                let names: Vec<String> = schemas.iter().map(|n| n.render_sql()).collect();
                s.push_str(&names.join(", "));
            }
        }
    }
    s.push_str(&render_with_options(p));
    s.push(';');
    s
}

/// `DROP PUBLICATION p;`
#[must_use]
pub fn drop_publication(name: &Identifier) -> String {
    format!("DROP PUBLICATION {};", name.render_sql())
}

/// Two-step replace (DROP + CREATE).
#[must_use]
pub fn replace_publication(from: &Publication, to: &Publication) -> [String; 2] {
    [drop_publication(&from.name), create_publication(to)]
}

/// `ALTER PUBLICATION p ADD TABLE x [(cols)] [WHERE (filter)];`
#[must_use]
pub fn alter_publication_add_table(pname: &Identifier, t: &PublishedTable) -> String {
    format!(
        "ALTER PUBLICATION {} ADD TABLE {};",
        pname.render_sql(),
        render_published_table(t),
    )
}

/// `ALTER PUBLICATION p DROP TABLE x;`
#[must_use]
pub fn alter_publication_drop_table(pname: &Identifier, qname: &QualifiedName) -> String {
    format!(
        "ALTER PUBLICATION {} DROP TABLE {};",
        pname.render_sql(),
        qname.render_sql(),
    )
}

/// `ALTER PUBLICATION p SET TABLE x (cols) WHERE (filter);`
/// Per-table SET replaces just that one table's spec without affecting others.
#[must_use]
pub fn alter_publication_set_table(pname: &Identifier, t: &PublishedTable) -> String {
    format!(
        "ALTER PUBLICATION {} SET TABLE {};",
        pname.render_sql(),
        render_published_table(t),
    )
}

/// `ALTER PUBLICATION p ADD TABLES IN SCHEMA s;`
#[must_use]
pub fn alter_publication_add_schema(pname: &Identifier, schema: &Identifier) -> String {
    format!(
        "ALTER PUBLICATION {} ADD TABLES IN SCHEMA {};",
        pname.render_sql(),
        schema.render_sql(),
    )
}

/// `ALTER PUBLICATION p DROP TABLES IN SCHEMA s;`
#[must_use]
pub fn alter_publication_drop_schema(pname: &Identifier, schema: &Identifier) -> String {
    format!(
        "ALTER PUBLICATION {} DROP TABLES IN SCHEMA {};",
        pname.render_sql(),
        schema.render_sql(),
    )
}

/// `ALTER PUBLICATION p SET (publish = '...');`
#[must_use]
pub fn alter_publication_set_publish(pname: &Identifier, k: PublishKinds) -> String {
    format!(
        "ALTER PUBLICATION {} SET (publish = '{}');",
        pname.render_sql(),
        render_publish_kinds(k),
    )
}

/// `ALTER PUBLICATION p SET (publish_via_partition_root = ...);`
#[must_use]
pub fn alter_publication_set_via_root(pname: &Identifier, value: bool) -> String {
    format!(
        "ALTER PUBLICATION {} SET (publish_via_partition_root = {});",
        pname.render_sql(),
        value,
    )
}

/// `COMMENT ON PUBLICATION p IS '...';`
#[must_use]
pub fn comment_on_publication(name: &Identifier, comment: Option<&str>) -> String {
    let body = comment.map_or_else(|| "NULL".to_string(), |c| format!("'{}'", c.replace('\'', "''")));
    format!("COMMENT ON PUBLICATION {} IS {};", name.render_sql(), body)
}

// ---- helpers ----

fn render_published_table(t: &PublishedTable) -> String {
    let mut s = t.qname.render_sql();
    if let Some(cols) = &t.columns {
        s.push_str(" (");
        let names: Vec<String> = cols.iter().map(|c| c.render_sql()).collect();
        s.push_str(&names.join(", "));
        s.push(')');
    }
    if let Some(filter) = &t.row_filter {
        s.push_str(" WHERE (");
        s.push_str(&filter.canonical_text);
        s.push(')');
    }
    s
}

fn render_with_options(p: &Publication) -> String {
    // Only emit WITH (...) if non-default values are present.
    let mut parts = Vec::new();
    if p.publish != PublishKinds::pg_default() {
        parts.push(format!("publish = '{}'", render_publish_kinds(p.publish)));
    }
    if p.publish_via_partition_root {
        parts.push("publish_via_partition_root = true".to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" WITH ({})", parts.join(", "))
    }
}

fn render_publish_kinds(k: PublishKinds) -> String {
    let mut parts = Vec::new();
    if k.insert   { parts.push("insert"); }
    if k.update   { parts.push("update"); }
    if k.delete   { parts.push("delete"); }
    if k.truncate { parts.push("truncate"); }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    #[test]
    fn renders_create_for_all_tables() {
        let p = Publication {
            name: id("p"),
            scope: PublicationScope::AllTables,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        assert_eq!(create_publication(&p), "CREATE PUBLICATION p FOR ALL TABLES;");
    }

    #[test]
    fn renders_create_for_table_with_columns_and_filter() {
        // ... build NormalizedExpr via NormalizedExpr::from_sql("(status = 'active')")
        // ... build PublishedTable with columns and row_filter
        // ... assert SQL contains "FOR TABLE app.t (id, name) WHERE (status = 'active')"
    }

    #[test]
    fn renders_publish_kinds_subset() {
        let k = PublishKinds { insert: true, update: true, delete: false, truncate: false };
        assert_eq!(render_publish_kinds(k), "insert, update");
    }

    #[test]
    fn renders_with_options_omits_pg_defaults() {
        let p = Publication {
            name: id("p"),
            scope: PublicationScope::AllTables,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        assert_eq!(render_with_options(&p), "");
    }

    #[test]
    fn renders_alter_set_publish() {
        let k = PublishKinds { insert: true, update: false, delete: false, truncate: false };
        assert_eq!(
            alter_publication_set_publish(&id("p"), k),
            "ALTER PUBLICATION p SET (publish = 'insert');",
        );
    }

    // ... 5 more tests for the remaining helpers.
}
```

- [ ] **Step 2: Replace Stage 7's stub arms in `plan/rewrite/mod.rs`**

```rust
        Change::CreatePublication(p) => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::CreatePublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::create_publication(p),
                transactional: TransactionConstraint::InTransaction,
            });
            // COMMENT step if present.
            if let Some(c) = &p.comment {
                raw_steps.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnPublication,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![],
                    sql: publications::comment_on_publication(&p.name, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        Change::DropPublication { name } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::DropPublication,
                destructive: true,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![],
                sql: publications::drop_publication(name),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::ReplacePublication { from, to } => {
            let [drop_sql, create_sql] = publications::replace_publication(from, to);
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::ReplacePublication,
                destructive: true,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![],
                sql: format!("{drop_sql}\n{create_sql}"),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPublicationAddTable { publication, table } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterPublicationAddTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.qname.clone()],
                sql: publications::alter_publication_add_table(publication, table),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPublicationDropTable { publication, qname } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterPublicationDropTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: publications::alter_publication_drop_table(publication, qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPublicationSetTable { publication, table } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterPublicationSetTable,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![table.qname.clone()],
                sql: publications::alter_publication_set_table(publication, table),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPublicationAddSchema { publication, schema } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterPublicationAddSchema,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_add_schema(publication, schema),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPublicationDropSchema { publication, schema } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterPublicationDropSchema,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_drop_schema(publication, schema),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPublicationSetPublish { publication, kinds } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterPublicationSetPublish,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_set_publish(publication, *kinds),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterPublicationSetViaRoot { publication, value } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterPublicationSetViaRoot,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::alter_publication_set_via_root(publication, *value),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::CommentOnPublication { name, comment } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnPublication,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: publications::comment_on_publication(name, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
```

Add `pub mod publications;` to `plan/rewrite/mod.rs`.

### Task 8.3: Dependency edges

- [ ] **Step 1: Extend `crates/pgevolve-core/src/plan/edges.rs::NodeId`**

```rust
    Publication(Identifier),
```

- [ ] **Step 2: Add edge construction in the dep-graph builder**

(Find the existing `build_dep_graph` or equivalent; mirror how View edges are added.)

```rust
for p in &source.publications {
    let pub_node = NodeId::Publication(p.name.clone());
    if let PublicationScope::Selective { schemas, tables } = &p.scope {
        for t in tables {
            graph.add_edge(NodeId::Table(t.qname.clone()), pub_node.clone(), DepSource::Structural);
        }
        for s in schemas {
            graph.add_edge(NodeId::Schema(s.clone()), pub_node.clone(), DepSource::Structural);
        }
    }
    // AllTables: planner enforces "publications after all table creates"
    // via a tier rule, no explicit edges.
}
```

### Task 8.4: Update CLI display

In `crates/pgevolve/src/commands/diff.rs`, replace the stub arms with human-readable display lines:

```rust
        Change::CreatePublication(p) => format!("+ CREATE PUBLICATION {}", p.name),
        Change::DropPublication { name } => format!("- DROP PUBLICATION {name}"),
        Change::ReplacePublication { from, to } => format!("~ REPLACE PUBLICATION {} (mode {} -> {})", from.name, scope_name(&from.scope), scope_name(&to.scope)),
        Change::AlterPublicationAddTable { publication, table } => format!("~ ALTER PUBLICATION {publication} ADD TABLE {}", table.qname),
        Change::AlterPublicationDropTable { publication, qname } => format!("~ ALTER PUBLICATION {publication} DROP TABLE {qname}"),
        Change::AlterPublicationSetTable { publication, table } => format!("~ ALTER PUBLICATION {publication} SET TABLE {}", table.qname),
        Change::AlterPublicationAddSchema { publication, schema } => format!("~ ALTER PUBLICATION {publication} ADD TABLES IN SCHEMA {schema}"),
        Change::AlterPublicationDropSchema { publication, schema } => format!("~ ALTER PUBLICATION {publication} DROP TABLES IN SCHEMA {schema}"),
        Change::AlterPublicationSetPublish { publication, .. } => format!("~ ALTER PUBLICATION {publication} SET (publish = ...)"),
        Change::AlterPublicationSetViaRoot { publication, value } => format!("~ ALTER PUBLICATION {publication} SET (publish_via_partition_root = {value})"),
        Change::CommentOnPublication { name, .. } => format!("~ COMMENT ON PUBLICATION {name}"),
```

Update `change_kind_name` to mirror.

- [ ] **Step 3: Run + commit**

```bash
cargo test -p pgevolve-core --lib plan::rewrite::publications
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/pgevolve-core/src/plan/ crates/pgevolve/src/commands/
git commit -m "$(cat <<'EOF'
feat(plan): publications render + emit + 11 new StepKinds + dep edges

plan::rewrite::publications renders CREATE/DROP/ALTER PUBLICATION
SQL. 11 new StepKind variants registered in kind_name /
parse_kind_name. Dep-graph edges: Publication → Table (Selective
tables) and Publication → Schema (Selective schemas); AllTables
relies on planner tier-rule ordering.

Stage 8 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 9 — Lint rules (4)

Three drift/correctness rules plus one PG-version-gate rule.

**Files created:**
- `crates/pgevolve-core/src/lint/rules/unmanaged_publication.rs`
- `crates/pgevolve-core/src/lint/rules/publication_captures_unmanaged_table.rs`
- `crates/pgevolve-core/src/lint/rules/publication_row_filter_references_unmanaged_column.rs`
- `crates/pgevolve-core/src/lint/rules/publication_feature_requires_pg_version.rs`

**Files modified:** `crates/pgevolve-core/src/lint/rules/mod.rs`, `crates/pgevolve-core/src/lint/universal.rs`.

### Task 9.1: `unmanaged-publication`

- [ ] **Step 1: Write the rule module**

```rust
//! `unmanaged-publication` (Warning) — catalog has a publication source doesn't.
//!
//! Per the lenient drift policy, the differ does not auto-drop publications
//! that source doesn't declare. This lint surfaces them so operators can
//! decide whether to bring under management or accept the drift.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-publication";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let source_names: std::collections::BTreeSet<_> =
        source.publications.iter().map(|p| &p.name).collect();
    target
        .publications
        .iter()
        .filter(|p| !source_names.contains(&p.name))
        .map(|p| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!("catalog has publication {} not declared in source", p.name),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // 4 tests:
    //   - empty + empty → silent
    //   - source has p, target has p → silent
    //   - source has p, target has p + q → fires for q
    //   - source has p, target has q → fires for q (not p, even though missing in target)
}
```

### Task 9.2: `publication-captures-unmanaged-table`

- [ ] **Step 1: Write the rule module**

```rust
//! `publication-captures-unmanaged-table` (Warning) — a FOR ALL TABLES or
//! FOR TABLES IN SCHEMA publication implicitly captures every current and
//! future table in scope. Lint surfaces every catalog-reported published
//! table whose qname falls outside the source IR's managed set.

use crate::ir::catalog::Catalog;
use crate::ir::publication::PublicationScope;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "publication-captures-unmanaged-table";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Build the "managed table" set from source.
    let source_tables: std::collections::BTreeSet<_> =
        source.tables.iter().map(|t| t.qname.clone()).collect();
    let source_schemas: std::collections::BTreeSet<_> =
        source.schemas.iter().map(|s| s.name.clone()).collect();

    for p in &target.publications {
        match &p.scope {
            PublicationScope::AllTables => {
                // Every catalog table NOT in source → captured but unmanaged.
                for t in &target.tables {
                    if !source_tables.contains(&t.qname) {
                        findings.push(Finding {
                            rule: RULE_ID,
                            severity: Severity::Warning,
                            message: format!(
                                "publication {} (FOR ALL TABLES) captures unmanaged table {}",
                                p.name, t.qname,
                            ),
                            location: None,
                        });
                    }
                }
            }
            PublicationScope::Selective { schemas, .. } => {
                for s in schemas {
                    if !source_schemas.contains(s) {
                        findings.push(Finding {
                            rule: RULE_ID,
                            severity: Severity::Warning,
                            message: format!(
                                "publication {} (FOR TABLES IN SCHEMA {}) references unmanaged schema",
                                p.name, s,
                            ),
                            location: None,
                        });
                        continue;
                    }
                    // Walk target.tables in this schema; any not in source.
                    for t in &target.tables {
                        if t.qname.schema == *s && !source_tables.contains(&t.qname) {
                            findings.push(Finding {
                                rule: RULE_ID,
                                severity: Severity::Warning,
                                message: format!(
                                    "publication {} (FOR TABLES IN SCHEMA {}) captures unmanaged table {}",
                                    p.name, s, t.qname,
                                ),
                                location: None,
                            });
                        }
                    }
                }
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    // 5 tests:
    //   - source declares all the tables the publication captures → silent
    //   - publication FOR ALL TABLES but catalog has table source doesn't → fires
    //   - publication FOR TABLES IN SCHEMA s; source doesn't declare s → fires (schema-level)
    //   - publication FOR TABLES IN SCHEMA s; source declares s but not table within → fires (table-level)
    //   - publication is Selective with explicit table list (no AllTables / no schema) → never fires
}
```

### Task 9.3: `publication-row-filter-references-unmanaged-column`

- [ ] **Step 1: Write the rule module**

Walks each `PublishedTable.row_filter`'s AST (using the existing `parse::ast_canon` helpers) to extract `ColumnRef` nodes, then verifies each referenced column exists on the target table in source.

```rust
//! `publication-row-filter-references-unmanaged-column` (Warning)
//! — a row filter references a column that source doesn't declare on the
//! target table.

use crate::ir::catalog::Catalog;
use crate::ir::publication::PublicationScope;
use crate::lint::finding::{Finding, Severity};
use crate::parse::ast_canon::extract_column_refs;  // reuse view-body walker

pub const RULE_ID: &str = "publication-row-filter-references-unmanaged-column";

pub fn check(source: &Catalog, _target: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();
    for p in &source.publications {
        let PublicationScope::Selective { tables, .. } = &p.scope else { continue; };
        for pt in tables {
            let Some(filter) = &pt.row_filter else { continue; };
            let refs = extract_column_refs(&filter.canonical_text);
            // Find the target table in source.
            let Some(table) = source.tables.iter().find(|t| t.qname == pt.qname) else {
                continue;  // table not in source → captured by another lint
            };
            let column_names: std::collections::BTreeSet<_> =
                table.columns.iter().map(|c| c.name.clone()).collect();
            for col_ref in refs {
                if !column_names.contains(&col_ref) {
                    findings.push(Finding {
                        rule: RULE_ID,
                        severity: Severity::Warning,
                        message: format!(
                            "publication {} row filter on {} references unmanaged column {}",
                            p.name, pt.qname, col_ref,
                        ),
                        location: None,
                    });
                }
            }
        }
    }
    findings
}
```

If `extract_column_refs` doesn't yet exist, add it next to the view-body machinery in `parse/ast_canon.rs` — read that module first to understand the existing AST-walker style.

### Task 9.4: `publication-feature-requires-pg-version`

- [ ] **Step 1: Write the rule module**

```rust
//! `publication-feature-requires-pg-version` (Error, not waivable) —
//! source uses a PG-version-gated feature but the project's declared
//! min_pg_version is too low.

use crate::ir::catalog::Catalog;
use crate::ir::publication::PublicationScope;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "publication-feature-requires-pg-version";

pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    let mut findings = Vec::new();
    for p in &source.publications {
        let PublicationScope::Selective { schemas, tables } = &p.scope else { continue; };
        if !schemas.is_empty() && min_pg_version < 15 {
            findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Error,
                message: format!(
                    "publication {} uses FOR TABLES IN SCHEMA which requires PG 15+; \
                     raise [managed].min_pg_version to 15 or remove the schema-scope clause",
                    p.name,
                ),
                location: None,
            });
        }
        for pt in tables {
            if pt.row_filter.is_some() && min_pg_version < 15 {
                findings.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Error,
                    message: format!(
                        "publication {} table {} uses a row filter which requires PG 15+; \
                         raise [managed].min_pg_version to 15 or remove the WHERE clause",
                        p.name, pt.qname,
                    ),
                    location: None,
                });
            }
            if pt.columns.is_some() && min_pg_version < 15 {
                findings.push(Finding {
                    rule: RULE_ID,
                    severity: Severity::Error,
                    message: format!(
                        "publication {} table {} uses an explicit column list which requires PG 15+; \
                         raise [managed].min_pg_version to 15 or remove the column list",
                        p.name, pt.qname,
                    ),
                    location: None,
                });
            }
        }
    }
    findings
}
```

The signature differs from the other rules because it takes `min_pg_version`. The lint dispatcher in `universal.rs` already has access to the config; thread `cfg.managed.min_pg_version` through to the call site.

### Task 9.5: Register all 4 rules

- [ ] **Step 1: Extend `crates/pgevolve-core/src/lint/rules/mod.rs`**

```rust
pub mod unmanaged_publication;
pub mod publication_captures_unmanaged_table;
pub mod publication_row_filter_references_unmanaged_column;
pub mod publication_feature_requires_pg_version;
```

- [ ] **Step 2: Wire into `crates/pgevolve-core/src/lint/universal.rs`**

The first three rules go into the drift-aware dispatcher alongside `unmanaged-grant` / `unmanaged-policy` / `unmanaged-reloption` — find that function (likely `run_drift_lints` or similar from v0.3.x) and add the calls.

The fourth (`publication-feature-requires-pg-version`) is source-only and depends on `min_pg_version`. Add it to the existing source-only-lint dispatcher with a new `min_pg_version: u32` parameter threaded through.

### Task 9.6: Commit

- [ ] **Step 1: Verify**

```bash
cargo test -p pgevolve-core --lib lint::rules::unmanaged_publication
cargo test -p pgevolve-core --lib lint::rules::publication_
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] **Step 2: Commit**

```bash
git add crates/pgevolve-core/src/lint/ crates/pgevolve-core/src/parse/ast_canon.rs crates/pgevolve/src/
git commit -m "$(cat <<'EOF'
feat(lint): 4 publication rules

  - unmanaged-publication (Warning, waivable) — catalog has a
    publication source doesn't.
  - publication-captures-unmanaged-table (Warning, waivable) — FOR
    ALL TABLES / FOR TABLES IN SCHEMA implicitly capture tables.
  - publication-row-filter-references-unmanaged-column (Warning,
    waivable) — row filter references a column source doesn't
    declare on the target table.
  - publication-feature-requires-pg-version (Error, not waivable)
    — source uses a PG15+ feature with min_pg_version < 15.

The first three are drift-aware lints; the fourth is source-only
and uses the new [managed].min_pg_version from Stage 4.

Stage 9 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 10 — Conformance fixtures (12)

Under `crates/pgevolve-conformance/tests/cases/objects/publications/`. All `authoring = "objects"`. Mirror v0.3.x fixture patterns.

### Task 10.1: Create fixtures

For each fixture: directory with `before.sql`, `after.sql`, `fixture.toml`, empty `expected/` (bless populates).

**1. `for-table-list/`** (min PG 14)

```sql
-- before.sql
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY);
CREATE TABLE app.customers (id bigint PRIMARY KEY);

-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY);
CREATE TABLE app.customers (id bigint PRIMARY KEY);
CREATE PUBLICATION main FOR TABLE app.orders, app.customers;
```

`fixture.toml`:
```toml
[meta]
title = "CREATE PUBLICATION FOR TABLE explicit list"
authoring = "objects"
spec_refs = ["objects.publication"]
[pg]
min = 14
max = 17
[expect.plan]
steps = 1
```

**2. `for-table-with-filter/`** (min PG 15) — explicit table + row filter.

**3. `for-table-with-columns/`** (min PG 15) — explicit table + column list.

**4. `for-table-with-filter-and-columns/`** (min PG 15) — combo.

**5. `for-all-tables/`** (min PG 14) — `CREATE PUBLICATION audit FOR ALL TABLES;`

**6. `for-tables-in-schema/`** (min PG 15) — `FOR TABLES IN SCHEMA app, billing`.

**7. `mixed-tables-and-schema/`** (min PG 15) — both `TABLE` and `TABLES IN SCHEMA`.

**8. `alter-add-table/`** — `before.sql` has publication with 1 table; `after.sql` has same publication with 2 tables. Expected plan: single `AlterPublicationAddTable` step.

**9. `alter-drop-table/`** — inverse of #8.

**10. `alter-set-publish/`** — change `publish` bitset.

**11. `mode-swap-replaces/`** — `before.sql` has `FOR ALL TABLES`; `after.sql` has `FOR TABLE app.x`. Expected: `ReplacePublication` (destructive, intent required).

**12. `lint/unmanaged-publication/`** — `before.sql` has publication; `after.sql` doesn't. Expected: 0 plan steps, advisory `unmanaged-publication`.

### Task 10.2: Bless + verify

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

Inspect blessed `expected/plan.sql` for:
- `for-table-list/` contains `CREATE PUBLICATION main FOR TABLE app.orders, app.customers;` — single step.
- `mode-swap-replaces/` contains a destructive `DROP PUBLICATION` + `CREATE PUBLICATION`.
- `lint/unmanaged-publication/` produces the advisory but 0 plan steps.

### Task 10.3: Commit

```bash
git add crates/pgevolve-conformance/tests/cases/objects/publications/
git commit -m "$(cat <<'EOF'
test(conformance): 12 publication fixtures

cases/objects/publications/ covering all 5 syntactic forms (explicit
table, all-tables, schema-scope, row filter, column list), plus
ALTER variants (add/drop/set), mode-swap, and the
unmanaged-publication lint.

Stage 10 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 11 — Proptest + docs + v0.3.4 release

### Task 11.1: Property test extensions

- [ ] **Step 1: Extend `crates/pgevolve-testkit/src/ir_generator.rs`**

```rust
fn arb_publish_kinds() -> impl Strategy<Value = PublishKinds> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>())
        .prop_filter("at least one DML kind", |(i, u, d, t)| *i || *u || *d || *t)
        .prop_map(|(insert, update, delete, truncate)| PublishKinds {
            insert, update, delete, truncate,
        })
}

fn arb_publication_scope(
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> BoxedStrategy<PublicationScope> {
    prop_oneof![
        Just(PublicationScope::AllTables),
        (
            proptest::sample::subsequence(schema_pool.clone(), 0..=schema_pool.len()),
            proptest::sample::subsequence(table_pool.clone(), 0..=table_pool.len()),
        )
            .prop_filter("non-empty Selective", |(s, t)| !s.is_empty() || !t.is_empty())
            .prop_map(|(schemas, tables)| {
                let schemas = schemas.into_iter().collect();
                let tables = tables
                    .into_iter()
                    .map(|qname| PublishedTable {
                        qname,
                        row_filter: None, // small strategy; deeper variation a v0.3.4.1 follow-up
                        columns: None,
                    })
                    .collect();
                PublicationScope::Selective { schemas, tables }
            })
    ]
    .boxed()
}

pub fn arb_publication(
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> impl Strategy<Value = Publication> {
    (
        identifier_strategy("pub"),
        arb_publication_scope(schema_pool, table_pool),
        arb_publish_kinds(),
        any::<bool>(),
    )
        .prop_map(|(name, scope, publish, via_root)| Publication {
            name,
            scope,
            publish,
            publish_via_partition_root: via_root,
            owner: None,
            comment: None,
        })
}
```

Plumb into `arbitrary_catalog`: generate 0–2 publications per catalog by drawing from the catalog's actual schemas + tables.

- [ ] **Step 2: Run 10× per constitution §9**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    echo "=== Run $i ==="
    PROPTEST_CASES=512 cargo test --workspace --release 2>&1 | tail -3
done
```

All 10 green.

- [ ] **Step 3: Commit**

```bash
git add crates/pgevolve-testkit/
git commit -m "$(cat <<'EOF'
test(proptest): publications in arbitrary_catalog

arb_publication / arb_publication_scope / arb_publish_kinds draw
schema + table targets from the catalog's actual contents so
generated publications always reference real objects. Row filters
and column lists are left None for now — deeper variation is a
v0.3.4.1 follow-up.

10× per §9; all green.

Stage 11.1 of docs/superpowers/plans/2026-05-26-publications.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 11.2: Docs

- [ ] **Step 1: Update `docs/spec/objects.md`**

Find the "Replication and federation" section (around line 258 — search for `PUBLICATION`). Replace:

```markdown
| `PUBLICATION` | 🔮 Future | Logical replication source-side metadata. |
```

with:

```markdown
| `PUBLICATION` | ✅ Supported | Logical-replication source-side metadata. All 5 forms (explicit FOR TABLE, FOR ALL TABLES, FOR TABLES IN SCHEMA PG15+, row filters PG15+, column lists PG15+). publish bitset + publish_via_partition_root. Lenient drift via unmanaged-publication. change_kinds: [create, drop, replace, alter_add_table, alter_drop_table, alter_set_table, alter_add_schema, alter_drop_schema, alter_set_publish, alter_set_via_root, comment_on] |
```

- [ ] **Step 2: Create `docs/spec/publications.md`**

A new capability page modeled on `docs/spec/reloptions.md`. Cover:
- Source surface (all 5 forms with examples)
- Semantics — lenient at the publication grain (unlike per-field reloption lenient)
- Mode-swap → ReplacePublication
- PG-version gating via `[managed].min_pg_version`
- Supported `publish` options + `publish_via_partition_root`
- Lints (4)
- Out of scope (SUBSCRIPTION → v0.3.5, no GRANT, no RENAME)

- [ ] **Step 3: CHANGELOG**

Add `[0.3.4]` section above `[0.3.3]`. Use today's date.

```markdown
## [0.3.4] — 2026-05-26

### Added

- **PUBLICATION as a first-class IR object.** All 5 PG syntactic
  forms (explicit FOR TABLE, FOR ALL TABLES, FOR TABLES IN SCHEMA
  PG15+, row filters PG15+, column lists PG15+). `PublicationScope`
  sum-type encodes PG's mutual exclusion of AllTables vs Selective.
- **Granular ALTER PUBLICATION semantics.** 11 new StepKind
  variants (add/drop/set per table, add/drop per schema, set publish,
  etc.) — each plan step is independently auditable and rollback-safe.
- **`[managed].min_pg_version` config key.** Defaults to 14;
  raise to 15+ to use row filters, column lists, or schema-scope.
  PG-version-gated source features fail at lint time
  (`publication-feature-requires-pg-version`, Error) instead of at
  apply with a Postgres syntax error.
- **4 lint rules**: `unmanaged-publication` (Warning),
  `publication-captures-unmanaged-table` (Warning),
  `publication-row-filter-references-unmanaged-column` (Warning),
  `publication-feature-requires-pg-version` (Error, not waivable).
- **12 conformance fixtures** under `objects/publications/`.

### Closes

Slipped from the v0.3 roadmap commitment (next: v0.3.5 SUBSCRIPTION).
```

### Task 11.3: Version bump

```bash
# Root Cargo.toml → workspace.package.version = "0.3.4"
# crates/pgevolve-core-macros/Cargo.toml → "0.2.1" (stays — proc-macro independent)
cargo build --workspace

v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
echo "version: $v"
grep -q "^## \[$v\] — " CHANGELOG.md && echo OK || echo MISMATCH
```

### Task 11.4: §9 verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"  # expect 0
```

### Task 11.5: Re-bless conformance

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

### Task 11.6: Release commit

```bash
git add docs/spec/objects.md docs/spec/publications.md CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pgevolve-conformance/tests/cases/
git commit -m "$(cat <<'EOF'
release: v0.3.4 — PUBLICATION

First-class declarative model for Postgres PUBLICATION across all
5 syntactic forms (explicit FOR TABLE, FOR ALL TABLES, FOR TABLES
IN SCHEMA PG15+, row filters PG15+, column lists PG15+).
PublicationScope sum-type enforces PG's mutual exclusion of
AllTables vs Selective at the type level.

11 new StepKind variants for granular ALTER PUBLICATION operations
(add/drop/set per table or schema, set publish, etc.). New
[managed].min_pg_version config key gates PG15+ source features at
lint time. 4 new lint rules. 12 new conformance fixtures.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 11.7: STOP

Do NOT `git tag`, `git push`, or close GH issues. The user handles those independently. Report DONE.

---

## Done.

After Stage 11, v0.3.4 is committed locally and ready for tagging.

Next plan target: **v0.3.5 SUBSCRIPTION** — needs its own design pass for the secrets/connection problem before plan-writing begins.
