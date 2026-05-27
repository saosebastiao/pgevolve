# v0.3.7 Implementation Plan — STATISTICS + VIEW WITH CHECK OPTION

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship v0.3.7 — adds first-class IR for Postgres `CREATE STATISTICS` (multi-column statistics objects with `ndistinct`/`dependencies`/`mcv` kinds + PG14+ expression statistics) and a per-view `check_option: Option<CheckOption>` field on existing `View` IR (`Local`/`Cascaded`).

**Architecture:** Eleven sequential stages. Stage 1 ships VIEW WITH CHECK OPTION end-to-end (small drop-in field). Stages 2–9 ship STATISTICS following the v0.3.4/v0.3.5 cadence (IR → canon → catalog reader → parser → differ → render+StepKinds+NodeId → lint). Stage 10 bundles all conformance fixtures (3 view + 6 statistic = 9 total). Stage 11 wraps with proptest + docs + the v0.3.7 release. Both features integrate with v0.3.1 lenient owner, v0.3.4 `[managed].min_pg_version` (none of v0.3.7's surface needs PG-version gating; statistics is stable across 14–18).

**Tech Stack:** Rust 1.95+, `pg_query` 6.x, `tokio_postgres`, `serde`, `proptest`. Builds on every v0.3.x pattern — no new cross-cutting concerns. Source spec: `docs/superpowers/specs/2026-05-27-statistics-and-check-option-design.md`.

---

## Pre-flight

- [ ] **Step 1: Confirm clean baseline**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
```

All green. v0.3.6 is committed and tagged; `main` is clean.

- [ ] **Step 2: Skim the spec once**

Open `docs/superpowers/specs/2026-05-27-statistics-and-check-option-design.md`. Each stage below cites the spec section it implements.

- [ ] **Step 3: Skim the v0.3.5 SUBSCRIPTION plan as the structural template**

`docs/superpowers/plans/2026-05-26-subscriptions.md`. STATISTICS follows the identical cadence; the differences are spec-specific (pg_statistic_ext catalog table, granular Replace-on-internal-change semantics, NormalizedExpr-canonicalized expression entries).

---

## File structure

```
crates/pgevolve-core/src/
├── ir/
│   ├── view.rs                      MODIFY — Stage 1 — add check_option field + CheckOption enum
│   ├── statistic.rs                 NEW — Stage 2 — Statistic, StatisticKinds, StatisticColumn
│   ├── catalog.rs                   MODIFY — Stage 3 — add statistics field
│   ├── mod.rs                       MODIFY — Stage 2 — re-export statistic + IrError variants
│   └── canon/
│       ├── mod.rs                   MODIFY — Stage 4 — wire statistics pass
│       └── statistics.rs            NEW — Stage 4 — validate + sort
├── catalog/
│   ├── statistics.rs                NEW — Stage 5 — decoder
│   ├── queries/shared.rs            MODIFY — Stage 1 (views query gets check_option) + Stage 5 (STATISTICS_QUERY)
│   ├── assemble/
│   │   └── statistics.rs            NEW — Stage 5 — assembler
│   └── mod.rs                       MODIFY — Stage 5 — wire into read_catalog
├── parse/
│   └── builder/
│       ├── view_stmt.rs             MODIFY — Stage 1 — extract WITH CHECK OPTION + folded form
│       ├── statistic_stmt.rs        NEW — Stage 6 — CREATE/ALTER/COMMENT STATISTICS + reject anonymous/INCLUDE/rename
│       └── mod.rs                   MODIFY — Stage 6 — dispatch
├── diff/
│   ├── views.rs                     MODIFY — Stage 1 — emit AlterViewSetCheckOption
│   ├── statistics.rs                NEW — Stage 7 — per-statistic granular diff
│   ├── change.rs                    MODIFY — Stage 1 + Stage 7 — 6 new variants (1 view + 5 statistic)
│   ├── mod.rs                       MODIFY — Stage 7 — call diff_statistics
│   └── owner_op.rs                  MODIFY — Stage 7 — OwnerObjectKind::Statistic
├── plan/
│   ├── raw_step.rs                  MODIFY — Stage 8 — 6 new StepKind variants
│   ├── plan.rs                      MODIFY — Stage 8 — extend kind_name / parse_kind_name
│   ├── edges.rs                     MODIFY — Stage 8 — add NodeId::Statistic + dep edges
│   └── rewrite/
│       ├── views.rs                 MODIFY — Stage 1 — render WITH CHECK OPTION + alter helper
│       ├── statistics.rs            NEW — Stage 8 — SQL helpers
│       └── mod.rs                   MODIFY — Stage 8 — dispatch new variants
└── lint/
    ├── rules/
    │   ├── unmanaged_statistic.rs   NEW — Stage 9
    │   └── mod.rs                   MODIFY — Stage 9
    └── universal.rs                 MODIFY — Stage 9 — wire unmanaged-statistic into run_drift_lints

crates/pgevolve/src/
└── commands/diff.rs                 MODIFY — Stage 8 — print_human + change_kind_name for 6 new variants

crates/pgevolve-conformance/tests/cases/objects/
├── views/                           MODIFY — Stage 10 — 3 new check-option fixtures
└── statistics/                      NEW — Stage 10 — 6 fixtures

crates/pgevolve-testkit/src/
└── ir_generator.rs                  MODIFY — Stage 11 — arb_statistic + plumb into arbitrary_catalog; arb_view check_option

docs/spec/
├── objects.md                       MODIFY — Stage 11 — flip STATISTICS + VIEW CHECK OPTION rows to ✅
├── statistics.md                    NEW — Stage 11 — capability page
└── README.md                        MODIFY — Stage 11 — index statistics.md

CHANGELOG.md                          MODIFY — Stage 11 — [0.3.7] section
Cargo.toml                            MODIFY — Stage 11 — version 0.3.6 → 0.3.7
```

---

## Stage 1 — VIEW WITH CHECK OPTION (end-to-end)

Small drop-in feature: adds `check_option: Option<CheckOption>` to `View`, parser extracts it from both source forms, catalog reader joins `information_schema.views`, differ emits `Change::AlterViewSetCheckOption`, planner emits `CREATE OR REPLACE VIEW`. Fixtures land in Stage 10 with everything else.

**Files modified:** `crates/pgevolve-core/src/ir/view.rs`, `catalog/queries/shared.rs`, `catalog/assemble/views.rs` (or wherever views are assembled), `parse/builder/view_stmt.rs`, `diff/views.rs`, `diff/change.rs`, `plan/raw_step.rs`, `plan/plan.rs`, `plan/rewrite/views.rs`, `plan/rewrite/mod.rs`, `commands/diff.rs`.

**Spec ref:** "Sub-spec 1: VIEW WITH CHECK OPTION".

### Task 1.1: Add `check_option` field + `CheckOption` enum

- [ ] **Step 1: Extend `crates/pgevolve-core/src/ir/view.rs`**

Add the enum near the top of the file:

```rust
/// `WITH [LOCAL | CASCADED] CHECK OPTION` setting on a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckOption {
    /// `WITH LOCAL CHECK OPTION` — applies only to this view's predicate.
    Local,
    /// `WITH CASCADED CHECK OPTION` — applies through chained updatable views.
    Cascaded,
}
```

Add field to `View` struct (after `security_invoker`, alphabetical / logical position):

```rust
    /// `WITH [LOCAL | CASCADED] CHECK OPTION`, when set in source.
    /// `None` = unmanaged (lenient — operator may have set it out-of-band;
    /// pgevolve neither sets nor resets unless source declares).
    pub check_option: Option<CheckOption>,
```

`MaterializedView` does **not** get this field (PG doesn't support CHECK OPTION on MVs).

- [ ] **Step 2: Backfill struct literals**

```bash
grep -rln "View {" crates/ | xargs grep -l "body_canonical:" | head
```

Each literal that doesn't use `..View::default()` (if such a constructor exists) or otherwise spread-update gets `check_option: None,`. Expect 5-15 sites across tests + assemblers.

- [ ] **Step 3: Build**

```bash
cargo build --workspace
```

Expected: clean. Any "missing field check_option" errors flag sites missed in step 2.

- [ ] **Step 4: Add unit tests** (in `view.rs::tests`)

```rust
#[test]
fn check_option_local_does_not_equal_cascaded() {
    assert_ne!(CheckOption::Local, CheckOption::Cascaded);
}

#[test]
fn check_option_implements_copy() {
    let a = CheckOption::Local;
    let _b = a; // copies
    let _c = a; // still usable
    assert_eq!(a, CheckOption::Local);
}
```

- [ ] **Step 5: Run + commit**

```bash
cargo test -p pgevolve-core --lib ir::view
git add crates/pgevolve-core/src/
git commit -m "$(cat <<'EOF'
feat(ir): View::check_option (Local | Cascaded)

Adds CheckOption enum and Option<CheckOption> field to View. None =
unmanaged (lenient — pgevolve neither sets nor resets unless source
declares). MaterializedView intentionally excluded (PG doesn't
support CHECK OPTION on MVs).

Stage 1.1 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Catalog reader extension

- [ ] **Step 1: Extend the views query in `crates/pgevolve-core/src/catalog/queries/shared.rs`**

Find the existing `VIEWS_QUERY` (or equivalent constant). Add a `check_option` column via `information_schema.views`:

```sql
-- Existing query (illustrative — adapt to actual shape):
SELECT v.schemaname, v.viewname, v.definition, ...
FROM pg_views v
WHERE ...

-- Add: LEFT JOIN information_schema.views vv
--      ON vv.table_schema = v.schemaname AND vv.table_name = v.viewname
-- New column: coalesce(vv.check_option, 'NONE') AS check_option
```

Result column `check_option` is text: `'NONE'` | `'LOCAL'` | `'CASCADED'`.

- [ ] **Step 2: Update the views assembler**

In `crates/pgevolve-core/src/catalog/assemble/views.rs` (or wherever views are decoded), read the new column:

```rust
let check_option = match row.get_text("check_option")?.as_str() {
    "NONE" => None,
    "LOCAL" => Some(CheckOption::Local),
    "CASCADED" => Some(CheckOption::Cascaded),
    other => {
        return Err(CatalogError::DecodeError(format!(
            "unknown information_schema.views.check_option value: {other:?}"
        )));
    }
};
```

Set the field on the assembled `View`.

- [ ] **Step 3: Add a tier-3 integration test**

In `crates/pgevolve-core/tests/catalog_round_trip.rs` (or wherever existing view round-trips live), add (or extend) a test:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn read_view_with_local_check_option() -> Result<()> {
    if !docker_available() { return Ok(()); }
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let client = pg.connect().await?;
    client.batch_execute(
        "CREATE SCHEMA app; \
         CREATE TABLE app.t (id bigint PRIMARY KEY, deleted boolean); \
         CREATE VIEW app.live AS SELECT * FROM app.t WHERE NOT deleted \
             WITH LOCAL CHECK OPTION;"
    ).await?;
    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(vec![Identifier::from_unquoted("app").unwrap()], vec![])?;
    let (catalog, _) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter)).await??;
    let view = catalog.views.iter().find(|v| v.qname.name.as_str() == "live").unwrap();
    assert_eq!(view.check_option, Some(CheckOption::Local));
    Ok(())
}
```

Plus a parallel `read_view_with_cascaded_check_option` and `read_view_without_check_option_is_none`.

- [ ] **Step 2: Verify + commit**

```bash
cargo test -p pgevolve-core --lib catalog
cargo test -p pgevolve-core --test catalog_round_trip
git add crates/pgevolve-core/src/catalog/
git commit -m "$(cat <<'EOF'
feat(catalog): read information_schema.views.check_option

Extends the views catalog query with a coalesce join on
information_schema.views.check_option. Decodes 'NONE'/'LOCAL'/'CASCADED'
to Option<CheckOption>.

Stage 1.2 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Parser extension

- [ ] **Step 1: Extend `crates/pgevolve-core/src/parse/builder/view_stmt.rs`**

The CREATE VIEW AST has two forms for check option:

1. **SQL-clause form** — `CREATE VIEW v AS SELECT ... WITH LOCAL CHECK OPTION`. pg_query encodes this in `ViewStmt.withCheckOption` field (an enum: `NoCheckOption=0` / `LocalCheckOption=1` / `CascadedCheckOption=2`).

2. **WITH-options form** — `CREATE VIEW v WITH (check_option = 'cascaded') AS SELECT ...`. pg_query encodes this in `ViewStmt.options` (a list of `DefElem` nodes with `defname = "check_option"`).

The parser must handle both and fold to one canonical `Option<CheckOption>`. If BOTH are present in the same statement (PG itself accepts this and they must agree, else PG errors), trust PG's error path and decode from whichever surface set it.

Implementation sketch:

```rust
fn extract_check_option(stmt: &ViewStmt) -> Result<Option<CheckOption>, ParseError> {
    // 1. SQL-clause form: stmt.withCheckOption is an i32 enum.
    let sql_clause = match stmt.with_check_option {
        0 => None,                          // NoCheckOption
        1 => Some(CheckOption::Local),       // LocalCheckOption
        2 => Some(CheckOption::Cascaded),    // CascadedCheckOption
        other => return Err(ParseError::UnknownCheckOptionVariant(other)),
    };

    // 2. WITH-options form: scan stmt.options for check_option DefElem.
    let with_opt = stmt.options.iter().find_map(|opt| {
        let de = opt.node.as_ref().and_then(|n| match n {
            node::Node::DefElem(d) => Some(d.as_ref()),
            _ => None,
        })?;
        if de.defname.as_str() == "check_option" {
            // Value is a String node containing 'local' or 'cascaded'.
            extract_def_elem_text(de).ok().map(|v| v.to_ascii_lowercase())
        } else { None }
    });
    let with_opt = match with_opt.as_deref() {
        None => None,
        Some("local") => Some(CheckOption::Local),
        Some("cascaded") => Some(CheckOption::Cascaded),
        Some(other) => return Err(ParseError::UnknownCheckOptionValue(other.to_string())),
    };

    // Both present: they must agree (PG enforces). Prefer sql_clause.
    Ok(sql_clause.or(with_opt))
}
```

Add two `ParseError` variants:

```rust
    UnknownCheckOptionVariant(i32),
    UnknownCheckOptionValue(String),
```

Wire `extract_check_option` into the `parse_create_view` (and `parse_create_or_replace_view` if separate) function, assigning the result to the new field.

- [ ] **Step 2: Unit tests in `view_stmt.rs::tests`**

```rust
#[test]
fn create_view_with_local_check_option_parses() {
    let cat = parse_sql("CREATE SCHEMA app; CREATE VIEW app.v AS SELECT 1 AS x WITH LOCAL CHECK OPTION;").unwrap();
    assert_eq!(cat.views[0].check_option, Some(CheckOption::Local));
}

#[test]
fn create_view_with_cascaded_check_option_parses() {
    let cat = parse_sql("CREATE SCHEMA app; CREATE VIEW app.v AS SELECT 1 AS x WITH CASCADED CHECK OPTION;").unwrap();
    assert_eq!(cat.views[0].check_option, Some(CheckOption::Cascaded));
}

#[test]
fn create_view_bare_with_check_option_defaults_to_cascaded() {
    let cat = parse_sql("CREATE SCHEMA app; CREATE VIEW app.v AS SELECT 1 AS x WITH CHECK OPTION;").unwrap();
    assert_eq!(cat.views[0].check_option, Some(CheckOption::Cascaded));
}

#[test]
fn create_view_with_options_form_parses() {
    let cat = parse_sql(
        "CREATE SCHEMA app; CREATE VIEW app.v WITH (check_option = 'local') AS SELECT 1 AS x;"
    ).unwrap();
    assert_eq!(cat.views[0].check_option, Some(CheckOption::Local));
}

#[test]
fn create_view_no_check_option_is_none() {
    let cat = parse_sql("CREATE SCHEMA app; CREATE VIEW app.v AS SELECT 1 AS x;").unwrap();
    assert_eq!(cat.views[0].check_option, None);
}
```

`parse_sql` is whatever helper the existing view-parser tests use — read `view_stmt.rs::tests` for the convention.

- [ ] **Step 3: Verify + commit**

```bash
cargo test -p pgevolve-core --lib parse::builder::view_stmt
git add crates/pgevolve-core/src/parse/
git commit -m "$(cat <<'EOF'
feat(parse): extract WITH CHECK OPTION from CREATE VIEW

Handles both syntactic forms: SQL clause (WITH LOCAL/CASCADED CHECK
OPTION) and WITH-options (WITH (check_option = 'local'|'cascaded')).
Folds both to one canonical Option<CheckOption>. Bare WITH CHECK
OPTION defaults to Cascaded per PG semantics.

Stage 1.3 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.4: Differ + render + StepKind

- [ ] **Step 1: Add Change variant in `crates/pgevolve-core/src/diff/change.rs`**

```rust
    /// `CREATE OR REPLACE VIEW … WITH [LOCAL|CASCADED] CHECK OPTION` or
    /// the inverse (set/unset check option on an existing view).
    AlterViewSetCheckOption {
        qname: crate::identifier::QualifiedName,
        new_value: Option<crate::ir::view::CheckOption>,
    },
```

- [ ] **Step 2: Add StepKind + kind_name + parse_kind_name**

In `crates/pgevolve-core/src/plan/raw_step.rs`:

```rust
    AlterViewSetCheckOption,
```

In `crates/pgevolve-core/src/plan/plan.rs::kind_name`:

```rust
        StepKind::AlterViewSetCheckOption => "alter_view_set_check_option",
```

Mirror in `parse_kind_name`. Extend the round-trip test array.

- [ ] **Step 3: Wire diff in `crates/pgevolve-core/src/diff/views.rs`**

Find the existing per-view diff path (after `diff_views` pairs and processes per-view field changes). Add:

```rust
if target.check_option != source.check_option {
    out.push(
        Change::AlterViewSetCheckOption {
            qname: source.qname.clone(),
            new_value: source.check_option,
        },
        Destructiveness::Safe,
    );
}
```

- [ ] **Step 4: Render helper in `crates/pgevolve-core/src/plan/rewrite/views.rs`**

Add a helper `alter_view_set_check_option`. Since PG has no direct `ALTER VIEW v SET CHECK OPTION`, this emits a full `CREATE OR REPLACE VIEW` carrying the new check_option clause. Reuse the existing `create_view` renderer with the new option set:

```rust
#[must_use]
pub fn alter_view_set_check_option(view: &View) -> String {
    // The `view` here is the SOURCE view (with the new check_option).
    // We emit CREATE OR REPLACE VIEW since PG has no ALTER ... SET CHECK OPTION.
    create_or_replace_view(view)
}
```

Also extend `create_view` (and `create_or_replace_view` if separate) to render the `WITH CHECK OPTION` clause when `view.check_option.is_some()`:

```rust
// At the end of the existing create_view body, before the semicolon:
if let Some(co) = view.check_option {
    s.push_str(" WITH ");
    s.push_str(match co {
        CheckOption::Local => "LOCAL",
        CheckOption::Cascaded => "CASCADED",
    });
    s.push_str(" CHECK OPTION");
}
```

- [ ] **Step 5: Emit arm in `crates/pgevolve-core/src/plan/rewrite/mod.rs`**

Find the existing emit dispatch. Add:

```rust
        Change::AlterViewSetCheckOption { qname, new_value: _ } => {
            // We need the full View IR (with the new check_option) to
            // emit CREATE OR REPLACE VIEW. Look it up in the SOURCE
            // catalog by qname. The dispatcher already has access to
            // the source catalog (mirror how other view-replace arms
            // resolve this in views.rs::emit).
            let view = ctx.source.views.iter()
                .find(|v| v.qname == *qname)
                .expect("source view present (differ wouldn't emit otherwise)");
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterViewSetCheckOption,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: views::alter_view_set_check_option(view),
                transactional: TransactionConstraint::InTransaction,
            });
        }
```

If the dispatcher doesn't have `ctx.source` available, the simpler alternative is to thread the full source `View` through the Change variant itself (changes the struct shape):

```rust
    AlterViewSetCheckOption {
        view: crate::ir::view::View,  // full view; renders as CREATE OR REPLACE
    },
```

Pick whichever approach matches the existing codebase pattern for view-replacement Change variants.

- [ ] **Step 6: CLI display in `crates/pgevolve/src/commands/diff.rs`**

In `print_human` and `change_kind_name`:

```rust
        Change::AlterViewSetCheckOption { qname, new_value } => {
            let label = match new_value {
                Some(CheckOption::Local) => "LOCAL",
                Some(CheckOption::Cascaded) => "CASCADED",
                None => "none",
            };
            format!("~ ALTER VIEW {qname} SET CHECK OPTION ({label})")
        }
```

`change_kind_name`: `"alter_view_set_check_option"`.

- [ ] **Step 7: Verify + commit**

```bash
cargo test -p pgevolve-core --lib diff::views
cargo test -p pgevolve-core --lib plan::rewrite::views
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/pgevolve-core/src/diff/ crates/pgevolve-core/src/plan/ crates/pgevolve/src/commands/diff.rs
git commit -m "$(cat <<'EOF'
feat(diff+plan): AlterViewSetCheckOption (CREATE OR REPLACE VIEW)

PG has no in-place ALTER for check option; the planner emits a full
CREATE OR REPLACE VIEW carrying the new check_option clause. One new
StepKind, non-destructive.

Stage 1.4 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 2 — STATISTICS IR foundation

Pure data types. No behavior beyond derives.

**Files created:** `crates/pgevolve-core/src/ir/statistic.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/mod.rs`.

**Spec ref:** "Sub-spec 2: CREATE STATISTICS — IR shape".

### Task 2.1: Create the module

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/statistic.rs`**

```rust
//! Statistic IR — declarative model for Postgres CREATE STATISTICS.
//!
//! pgevolve manages `pg_statistic_ext` objects with explicit names. Source
//! must declare the name (`CREATE STATISTICS app.s ON (...) FROM app.t`);
//! anonymous form `CREATE STATISTICS ON (...) FROM app.t` is rejected at
//! parse time, mirroring the no-anonymous-indexes policy.
//!
//! Spec: `docs/superpowers/specs/2026-05-27-statistics-and-check-option-design.md`.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;

/// Declarative model of a Postgres `CREATE STATISTICS` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statistic {
    /// Schema-qualified statistic name (explicit names required).
    pub qname: QualifiedName,
    /// The target table whose columns are correlated.
    pub target: QualifiedName,
    /// Which kinds are enabled. At least one must be true (canon enforces).
    pub kinds: StatisticKinds,
    /// Column / expression list. Sorted by canon; deduped.
    pub columns: Vec<StatisticColumn>,
    /// `ALTER STATISTICS s SET STATISTICS n` — analyze target.
    /// `None` = unmanaged / use PG default (-1).
    pub statistics_target: Option<i32>,
    /// Object owner. `None` = unmanaged (v0.3.1 lenient pattern).
    pub owner: Option<Identifier>,
    /// Optional `COMMENT ON STATISTICS`.
    pub comment: Option<String>,
}

/// Which `kinds` flags are enabled on a `CREATE STATISTICS` object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatisticKinds {
    /// `ndistinct` — multi-column n-distinct counts.
    pub ndistinct: bool,
    /// `dependencies` — functional dependencies between columns.
    pub dependencies: bool,
    /// `mcv` — most-common-value lists per column combination.
    pub mcv: bool,
}

impl StatisticKinds {
    /// PG's default when no kinds clause is given: all three enabled.
    #[must_use]
    pub const fn pg_default() -> Self {
        Self { ndistinct: true, dependencies: true, mcv: true }
    }

    /// True iff at least one kind is enabled. An empty bitset is illegal
    /// at the IR level (canon rejects).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.ndistinct && !self.dependencies && !self.mcv
    }
}

/// A single entry in the statistic's column list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatisticColumn {
    /// Plain `column_name` reference.
    Column(Identifier),
    /// Expression statistic (PG 14+): `(lower(name))`. Canonicalized via
    /// `NormalizedExpr` (same canon as CHECK / USING / WITH CHECK).
    Expression(NormalizedExpr),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_default_is_all_true() {
        let k = StatisticKinds::pg_default();
        assert!(k.ndistinct && k.dependencies && k.mcv);
        assert!(!k.is_empty());
    }

    #[test]
    fn kinds_empty_when_all_false() {
        let k = StatisticKinds {
            ndistinct: false,
            dependencies: false,
            mcv: false,
        };
        assert!(k.is_empty());
    }

    #[test]
    fn column_form_does_not_equal_expression_form() {
        let c = StatisticColumn::Column(Identifier::from_unquoted("a").unwrap());
        let e = StatisticColumn::Expression(
            NormalizedExpr { canonical_text: "a".into(), ast_hash: [0; 32] }
        );
        assert_ne!(c, e);
    }
}
```

(`NormalizedExpr` may have a different constructor shape — use whatever the existing tests in `default_expr.rs` use. The third test is illustrative; adapt to actually-constructable expressions if simpler.)

- [ ] **Step 2: Add to `crates/pgevolve-core/src/ir/mod.rs`**

```rust
pub mod statistic;
```

Alphabetical position (between `sequence` and `subscription`).

- [ ] **Step 3: Build + test + commit**

```bash
cargo build -p pgevolve-core
cargo test -p pgevolve-core --lib ir::statistic
git add crates/pgevolve-core/src/ir/statistic.rs crates/pgevolve-core/src/ir/mod.rs
git commit -m "$(cat <<'EOF'
feat(ir): Statistic, StatisticKinds, StatisticColumn

New top-level IR module for CREATE STATISTICS. Pure data types; no
behavior beyond derives. Three kinds (ndistinct, dependencies, mcv)
plus expression-form columns via NormalizedExpr.

Stage 2 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 3 — Add `statistics` field to Catalog

**Files modified:** `crates/pgevolve-core/src/ir/catalog.rs` (and any hand-rolled `Catalog { … }` struct literals).

### Task 3.1: Backfill the field

- [ ] **Step 1: Add the field to `Catalog`**

In `crates/pgevolve-core/src/ir/catalog.rs`, append to the struct (alphabetical / logical, after `sequences`, before `subscriptions`):

```rust
    /// Multi-column statistics objects (CREATE STATISTICS).
    pub statistics: Vec<crate::ir::statistic::Statistic>,
```

- [ ] **Step 2: Initialize in `Catalog::empty()`**

```rust
            statistics: Vec::new(),
```

- [ ] **Step 3: Find + backfill hand-rolled literals**

```bash
grep -rln "Catalog {" crates/ | xargs grep -l "schemas:" | head
```

Each literal not using `..Catalog::empty()` gets `statistics: Vec::new(),`. v0.3.4 + v0.3.5 each found one site in `pgevolve-testkit/src/ir_mutator.rs`; expect similar here.

- [ ] **Step 4: Build + test**

```bash
cargo build --workspace
cargo test --workspace --lib
```

- [ ] **Step 5: Commit**

```bash
git add crates/pgevolve-core/src/ir/catalog.rs crates/pgevolve-testkit/src/
git commit -m "$(cat <<'EOF'
feat(ir): add Catalog::statistics

Backfills hand-rolled Catalog struct literals with statistics: Vec::new().
Pure plumbing — no behavior change.

Stage 3 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 4 — STATISTICS canon pass

Validate + sort.

**Files created:** `crates/pgevolve-core/src/ir/canon/statistics.rs`.
**Files modified:** `crates/pgevolve-core/src/ir/canon/mod.rs`, `crates/pgevolve-core/src/ir/mod.rs` (add IrError variants).

### Task 4.1: Add IrError variants

- [ ] **Step 1: Extend `IrError` in `ir/mod.rs`**

```rust
    /// A `StatisticKinds` had all three flags false.
    #[error("statistic {0}: empty kinds bitset (must enable at least one of ndistinct, dependencies, mcv)")]
    EmptyStatisticKinds(crate::identifier::QualifiedName),
    /// A `Statistic.columns` was empty.
    #[error("statistic {0}: empty column list")]
    EmptyStatisticColumns(crate::identifier::QualifiedName),
```

Build: `cargo build -p pgevolve-core`.

### Task 4.2: Create canon pass

- [ ] **Step 1: Write `crates/pgevolve-core/src/ir/canon/statistics.rs`**

```rust
//! Canon pass for statistics. Validates and sorts.
//!
//! Invariants enforced:
//! - `Statistic.kinds` has at least one enabled.
//! - `Statistic.columns` is non-empty.
//!
//! Sorts:
//! - `Statistic.columns` with Columns first (sorted by Identifier), then
//!   Expressions (sorted by canonical_text). Duplicates silently deduped.
//! - The statistics collection itself is sorted by `sort_and_dedupe`,
//!   not here.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::statistic::{Statistic, StatisticColumn};

pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for s in &mut cat.statistics {
        validate_and_sort(s)?;
    }
    Ok(())
}

fn validate_and_sort(s: &mut Statistic) -> Result<(), IrError> {
    if s.kinds.is_empty() {
        return Err(IrError::EmptyStatisticKinds(s.qname.clone()));
    }
    if s.columns.is_empty() {
        return Err(IrError::EmptyStatisticColumns(s.qname.clone()));
    }
    s.columns.sort_by(|a, b| match (a, b) {
        (StatisticColumn::Column(a), StatisticColumn::Column(b)) => a.cmp(b),
        (StatisticColumn::Column(_), StatisticColumn::Expression(_)) => std::cmp::Ordering::Less,
        (StatisticColumn::Expression(_), StatisticColumn::Column(_)) => std::cmp::Ordering::Greater,
        (StatisticColumn::Expression(a), StatisticColumn::Expression(b)) => {
            a.canonical_text.cmp(&b.canonical_text)
        }
    });
    s.columns.dedup();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::statistic::StatisticKinds;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn stat(cols: Vec<StatisticColumn>, kinds: StatisticKinds) -> Statistic {
        Statistic {
            qname: qn("app", "s"),
            target: qn("app", "t"),
            kinds,
            columns: cols,
            statistics_target: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn rejects_empty_kinds() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(
            vec![StatisticColumn::Column(id("a"))],
            StatisticKinds { ndistinct: false, dependencies: false, mcv: false },
        ));
        assert!(matches!(run(&mut cat).unwrap_err(), IrError::EmptyStatisticKinds(_)));
    }

    #[test]
    fn rejects_empty_columns() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(vec![], StatisticKinds::pg_default()));
        assert!(matches!(run(&mut cat).unwrap_err(), IrError::EmptyStatisticColumns(_)));
    }

    #[test]
    fn sorts_columns_then_expressions() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(
            vec![
                StatisticColumn::Expression(NormalizedExpr {
                    canonical_text: "lower(name)".into(),
                    ast_hash: [0; 32],
                }),
                StatisticColumn::Column(id("b")),
                StatisticColumn::Column(id("a")),
                StatisticColumn::Expression(NormalizedExpr {
                    canonical_text: "abs(id)".into(),
                    ast_hash: [0; 32],
                }),
            ],
            StatisticKinds::pg_default(),
        ));
        run(&mut cat).unwrap();
        let cols = &cat.statistics[0].columns;
        assert_eq!(cols.len(), 4);
        // Columns first, then expressions.
        assert!(matches!(cols[0], StatisticColumn::Column(ref i) if i.as_str() == "a"));
        assert!(matches!(cols[1], StatisticColumn::Column(ref i) if i.as_str() == "b"));
        assert!(matches!(cols[2], StatisticColumn::Expression(ref e) if e.canonical_text == "abs(id)"));
        assert!(matches!(cols[3], StatisticColumn::Expression(ref e) if e.canonical_text == "lower(name)"));
    }

    #[test]
    fn dedupes_duplicate_columns() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(
            vec![
                StatisticColumn::Column(id("a")),
                StatisticColumn::Column(id("a")),
                StatisticColumn::Column(id("b")),
            ],
            StatisticKinds::pg_default(),
        ));
        run(&mut cat).unwrap();
        assert_eq!(cat.statistics[0].columns.len(), 2);
    }

    #[test]
    fn passes_through_valid_statistic() {
        let mut cat = Catalog::empty();
        cat.statistics.push(stat(
            vec![StatisticColumn::Column(id("a"))],
            StatisticKinds::pg_default(),
        ));
        assert!(run(&mut cat).is_ok());
    }
}
```

- [ ] **Step 2: Wire into orchestrator**

In `crates/pgevolve-core/src/ir/canon/mod.rs`, add `pub mod statistics;` and call `statistics::run(cat)?;` after the existing per-object-kind passes (alphabetical / pipeline order — after `subscriptions::run` from v0.3.5).

- [ ] **Step 3: Build + test + commit**

```bash
cargo test -p pgevolve-core --lib ir::canon::statistics
git add crates/pgevolve-core/src/ir/canon/ crates/pgevolve-core/src/ir/mod.rs
git commit -m "$(cat <<'EOF'
feat(ir): canon pass for statistics

Validates non-empty kinds + non-empty column list. Sorts columns
(Column form first by Identifier, Expression form second by
canonical_text). Dedupes.

Stage 4 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 5 — STATISTICS catalog reader

Read `pg_statistic_ext` per-PG (stable across 14–18; no version variants needed).

**Files created:** `crates/pgevolve-core/src/catalog/statistics.rs`, `crates/pgevolve-core/src/catalog/assemble/statistics.rs`.
**Files modified:** `crates/pgevolve-core/src/catalog/queries/shared.rs`, `crates/pgevolve-core/src/catalog/mod.rs`.

### Task 5.1: SQL query

- [ ] **Step 1: Add to `crates/pgevolve-core/src/catalog/queries/shared.rs`**

```rust
/// Multi-column statistics objects (CREATE STATISTICS). Stable across
/// PG 14–18 — no version variants. Expression statistics are decoded via
/// `pg_get_expr` per stxexprs entry.
pub const STATISTICS_QUERY: &str = "\
    SELECT \
        s.oid::bigint AS oid, \
        sn.nspname::text AS schema, \
        s.stxname::text AS name, \
        tn.nspname::text AS target_schema, \
        t.relname::text AS target_name, \
        s.stxkind::text[] AS kinds, \
        s.stxkeys::int2[] AS keys, \
        t.oid::bigint AS target_oid, \
        coalesce(s.stxstattarget, -1) AS stat_target, \
        coalesce(a.rolname, '') AS owner, \
        coalesce(d.description, '') AS comment, \
        s.stxexprs IS NOT NULL AS has_expressions \
    FROM pg_statistic_ext s \
    JOIN pg_namespace sn ON sn.oid = s.stxnamespace \
    JOIN pg_class t ON t.oid = s.stxrelid \
    JOIN pg_namespace tn ON tn.oid = t.relnamespace \
    JOIN pg_authid a ON a.oid = s.stxowner \
    LEFT JOIN pg_description d \
        ON d.classoid = 'pg_statistic_ext'::regclass AND d.objoid = s.oid AND d.objsubid = 0 \
    ORDER BY sn.nspname, s.stxname";
```

Add a per-row follow-up query for expression entries (decoded via `pg_get_expr`):

```rust
/// Per-statistic expression decode. Returns one row per expression entry,
/// in stxexprs array order. Invoked per row when STATISTICS_QUERY indicates
/// has_expressions = true.
pub const STATISTIC_EXPRESSIONS_QUERY: &str = "\
    SELECT \
        idx AS expr_index, \
        pg_get_expr(expr, $2) AS expr_sql \
    FROM ( \
        SELECT \
            row_number() OVER () - 1 AS idx, \
            unnest(stxexprs::pg_node_tree[]) AS expr \
        FROM pg_statistic_ext WHERE oid = $1 \
    ) x \
    ORDER BY idx";
```

(`stxexprs` is technically `pg_node_tree` not an array of trees — the unnest may need a different approach. Confirm by reading PG docs and / or testing against an ephemeral PG. The implementer adapts as needed; the principle is: decode each expression entry via `pg_get_expr(expr, stxrelid)` to get SQL text.)

Per-column-name resolution: `stxkeys` is `int2[]` of `pg_attribute.attnum` values. Use the existing attribute-resolution helper from publications (Stage 5 of v0.3.4 added a per-row attribute resolver), or write a new bulk-resolution query.

- [ ] **Step 2: Add `Statistics` variant to `CatalogQuery` enum + per-version dispatch**

In `crates/pgevolve-core/src/catalog/queries/mod.rs`:

```rust
    Statistics,
    StatisticExpressions,  // if used; could be a bare SQL string instead of a variant
```

Dispatch (all PG versions use the shared query):

```rust
            CatalogQuery::Statistics => shared::STATISTICS_QUERY,
            CatalogQuery::StatisticExpressions => shared::STATISTIC_EXPRESSIONS_QUERY,
```

### Task 5.2: Decoder

- [ ] **Step 1: Write `crates/pgevolve-core/src/catalog/statistics.rs`**

Skeleton (adapt to actual Row API — read `crates/pgevolve-core/src/catalog/publications.rs` for the v0.3.4 template):

```rust
//! Decode pg_statistic_ext rows into Statistic IR.

#![allow(clippy::result_large_err)]

use crate::catalog::error::CatalogError;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::statistic::{Statistic, StatisticColumn, StatisticKinds};

pub struct PartialStatistic {
    pub oid: i64,
    pub qname: QualifiedName,
    pub target: QualifiedName,
    pub target_oid: i64,
    pub kinds: StatisticKinds,
    pub keys: Vec<i16>,
    pub has_expressions: bool,
    pub statistics_target: Option<i32>,
    pub owner: Option<Identifier>,
    pub comment: Option<String>,
}

pub fn decode_statistic_row(row: &impl RowLike) -> Result<PartialStatistic, CatalogError> {
    let schema = row.get_text("schema")?;
    let name = row.get_text("name")?;
    let target_schema = row.get_text("target_schema")?;
    let target_name = row.get_text("target_name")?;
    let kinds_raw: Vec<String> = row.get_text_array("kinds")?;
    let keys: Vec<i16> = row.get_int2_array("keys")?;
    let stat_target = row.get_int("stat_target")?;
    let owner_str = row.get_text("owner")?;
    let comment_str = row.get_text("comment")?;
    let has_expressions = row.get_bool("has_expressions")?;

    let kinds = StatisticKinds {
        ndistinct: kinds_raw.iter().any(|k| k == "d"),
        dependencies: kinds_raw.iter().any(|k| k == "f"),
        mcv: kinds_raw.iter().any(|k| k == "m"),
        // 'e' is an internal marker for "has expressions"; ignored here
        // since has_expressions is decoded separately.
    };

    Ok(PartialStatistic {
        oid: row.get_i64("oid")?,
        qname: QualifiedName::new(
            Identifier::from_unquoted(&schema).map_err(|e| CatalogError::InvalidIdentifier(schema, e.to_string()))?,
            Identifier::from_unquoted(&name).map_err(|e| CatalogError::InvalidIdentifier(name, e.to_string()))?,
        ),
        target: QualifiedName::new(
            Identifier::from_unquoted(&target_schema).map_err(|e| CatalogError::InvalidIdentifier(target_schema, e.to_string()))?,
            Identifier::from_unquoted(&target_name).map_err(|e| CatalogError::InvalidIdentifier(target_name, e.to_string()))?,
        ),
        target_oid: row.get_i64("target_oid")?,
        kinds,
        keys,
        has_expressions,
        statistics_target: if stat_target == -1 { None } else { Some(i32::try_from(stat_target).unwrap_or(-1)) },
        owner: if owner_str.is_empty() { None } else { Some(Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::InvalidIdentifier(owner_str, e.to_string()))?) },
        comment: if comment_str.is_empty() { None } else { Some(comment_str) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_decode_ndistinct() {
        let raw: Vec<String> = vec!["d".into()];
        let kinds = StatisticKinds {
            ndistinct: raw.iter().any(|k| k == "d"),
            dependencies: raw.iter().any(|k| k == "f"),
            mcv: raw.iter().any(|k| k == "m"),
        };
        assert!(kinds.ndistinct && !kinds.dependencies && !kinds.mcv);
    }

    #[test]
    fn kinds_decode_all_three() {
        let raw: Vec<String> = vec!["d".into(), "f".into(), "m".into()];
        let kinds = StatisticKinds {
            ndistinct: raw.iter().any(|k| k == "d"),
            dependencies: raw.iter().any(|k| k == "f"),
            mcv: raw.iter().any(|k| k == "m"),
        };
        assert!(kinds.ndistinct && kinds.dependencies && kinds.mcv);
    }

    #[test]
    fn stat_target_minus_one_is_none() {
        // Smoke: -1 from PG → None in IR.
        let stat_target = -1_i32;
        let result: Option<i32> = if stat_target == -1 { None } else { Some(stat_target) };
        assert!(result.is_none());
    }
}
```

### Task 5.3: Assembler

- [ ] **Step 1: Write `crates/pgevolve-core/src/catalog/assemble/statistics.rs`**

```rust
//! Orchestrate pg_statistic_ext read into Vec<Statistic>.

#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use crate::catalog::error::CatalogError;
use crate::catalog::statistics::{PartialStatistic, decode_statistic_row};
use crate::identifier::Identifier;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::statistic::{Statistic, StatisticColumn};

pub fn assemble_statistics(
    rows: &[impl RowLike],
    columns_by_rel: &BTreeMap<i64, BTreeMap<i16, Identifier>>,  // rel_oid → (attnum → name)
    expressions_by_oid: &BTreeMap<i64, Vec<NormalizedExpr>>,    // statistic oid → ordered exprs
) -> Result<Vec<Statistic>, CatalogError> {
    rows.iter()
        .map(|row| {
            let p = decode_statistic_row(row)?;
            let mut columns = Vec::with_capacity(p.keys.len() + expressions_by_oid.get(&p.oid).map_or(0, Vec::len));
            // Resolve column attnums.
            let attmap = columns_by_rel.get(&p.target_oid).ok_or_else(|| {
                CatalogError::DecodeError(format!(
                    "no attribute map for target table oid {} (statistic {})",
                    p.target_oid, p.qname,
                ))
            })?;
            for attnum in &p.keys {
                let name = attmap.get(attnum).ok_or_else(|| {
                    CatalogError::DecodeError(format!(
                        "attnum {attnum} not in pg_attribute for target table {} (statistic {})",
                        p.target, p.qname,
                    ))
                })?;
                columns.push(StatisticColumn::Column(name.clone()));
            }
            // Append expression entries.
            if let Some(exprs) = expressions_by_oid.get(&p.oid) {
                for e in exprs {
                    columns.push(StatisticColumn::Expression(e.clone()));
                }
            }
            Ok(Statistic {
                qname: p.qname,
                target: p.target,
                kinds: p.kinds,
                columns,
                statistics_target: p.statistics_target,
                owner: p.owner,
                comment: p.comment,
            })
        })
        .collect()
}
```

The expressions-by-oid map is built by the caller from the per-row `STATISTIC_EXPRESSIONS_QUERY` follow-up queries (only invoked when `has_expressions = true`). The column-attnum map is built from a bulk `pg_attribute` query like the v0.3.4 publications assembler.

### Task 5.4: Wire into `read_catalog`

- [ ] **Step 1: Add call in `crates/pgevolve-core/src/catalog/mod.rs`**

After other object-kind assemblers, before canonicalize:

```rust
// Statistics — needs column attmap + expression decoder.
let stat_rows = querier.run(CatalogQuery::Statistics)?;
let stat_attmap = build_statistic_attribute_map(querier, &stat_rows)?;
let stat_exprs = build_statistic_expressions(querier, &stat_rows)?;
catalog.statistics = crate::catalog::assemble::statistics::assemble_statistics(
    &stat_rows, &stat_attmap, &stat_exprs,
)?;
```

Implement `build_statistic_attribute_map` and `build_statistic_expressions` as local helpers in `catalog/mod.rs` (or as private helpers in `catalog/statistics.rs`).

### Task 5.5: Docker integration test

- [ ] **Step 1: Create `crates/pgevolve-core/tests/statistic_round_trip.rs`**

```rust
//! Round-trip: CREATE STATISTICS → read back → assert equal IR.
//! Requires Docker; skips when unavailable.

#![cfg(all(test, feature = "testkit"))]

use anyhow::Result;
use pgevolve_core::catalog::{CatalogFilter, read_catalog};
use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::statistic::{StatisticColumn, StatisticKinds};
use pgevolve_testkit::PgCatalogQuerier;
use pgevolve_testkit::ephemeral_pg::{EphemeralPostgres, default_pg_version, docker_available};

#[tokio::test(flavor = "multi_thread")]
async fn read_statistic_basic() -> Result<()> {
    if !docker_available() { return Ok(()); }
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let client = pg.connect().await?;
    client.batch_execute(
        "CREATE SCHEMA app; \
         CREATE TABLE app.t (id bigint PRIMARY KEY, a int, b int); \
         CREATE STATISTICS app.t_corr (ndistinct, dependencies) ON a, b FROM app.t;"
    ).await?;
    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(vec![Identifier::from_unquoted("app").unwrap()], vec![])?;
    let (catalog, _) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter)).await??;
    assert_eq!(catalog.statistics.len(), 1);
    let s = &catalog.statistics[0];
    assert_eq!(s.qname.name.as_str(), "t_corr");
    assert_eq!(s.target.name.as_str(), "t");
    assert_eq!(s.kinds, StatisticKinds { ndistinct: true, dependencies: true, mcv: false });
    assert_eq!(s.columns.len(), 2);
    assert!(matches!(s.columns[0], StatisticColumn::Column(ref i) if i.as_str() == "a"));
    assert!(matches!(s.columns[1], StatisticColumn::Column(ref i) if i.as_str() == "b"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn read_statistic_with_expression() -> Result<()> {
    if !docker_available() { return Ok(()); }
    let pg = EphemeralPostgres::start(default_pg_version()).await?;
    let client = pg.connect().await?;
    client.batch_execute(
        "CREATE SCHEMA app; \
         CREATE TABLE app.t (id bigint PRIMARY KEY, name text); \
         CREATE STATISTICS app.t_lower ON (lower(name)) FROM app.t;"
    ).await?;
    let querier = PgCatalogQuerier::new(client)?;
    let filter = CatalogFilter::new(vec![Identifier::from_unquoted("app").unwrap()], vec![])?;
    let (catalog, _) = tokio::task::spawn_blocking(move || read_catalog(&querier, &filter)).await??;
    assert_eq!(catalog.statistics.len(), 1);
    let s = &catalog.statistics[0];
    assert_eq!(s.columns.len(), 1);
    assert!(matches!(s.columns[0], StatisticColumn::Expression(_)));
    Ok(())
}
```

### Task 5.6: Verify + commit

```bash
cargo build -p pgevolve-core
cargo test -p pgevolve-core --lib catalog::statistics
cargo test -p pgevolve-core --test statistic_round_trip
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/pgevolve-core/src/catalog/ crates/pgevolve-core/tests/statistic_round_trip.rs
git commit -m "$(cat <<'EOF'
feat(catalog): read statistics from pg_statistic_ext

Reads pg_statistic_ext joined with pg_namespace, pg_class, pg_authid,
pg_description. stxkind char[] decoded into StatisticKinds (d/f/m;
'e' marker ignored — derived from has_expressions). Expression-form
columns decoded via pg_get_expr per stxexprs entry and canonicalized
through NormalizedExpr.

Stage 5 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 6 — STATISTICS source parser

Parse CREATE / ALTER / COMMENT STATISTICS into IR. Fold into one canonical record per qname. Reject anonymous CREATE, RENAME, INCLUDE.

**Files created:** `crates/pgevolve-core/src/parse/builder/statistic_stmt.rs`.
**Files modified:** `crates/pgevolve-core/src/parse/builder/mod.rs`, `crates/pgevolve-core/src/parse/error.rs`.

### Task 6.1: Add ParseError variants

- [ ] **Step 1: Extend `ParseError`**

```rust
    DuplicateStatistic(QualifiedName, SourceLocation),
    StatisticAnonymous(SourceLocation),
    StatisticEmptyKinds(QualifiedName, SourceLocation),
    StatisticEmptyColumns(QualifiedName, SourceLocation),
    UnknownStatisticKind(String, QualifiedName, SourceLocation),
    StatisticIncludeNotSupported(QualifiedName, SourceLocation),
    StatisticRenameNotSupported(QualifiedName, SourceLocation),
    AlterStatisticBeforeCreate(QualifiedName, SourceLocation),
    CommentOnStatisticBeforeCreate(QualifiedName, SourceLocation),
```

### Task 6.2: Write the parser module

- [ ] **Step 1: Create `crates/pgevolve-core/src/parse/builder/statistic_stmt.rs`**

The pg_query AST node is `CreateStatsStmt`. Fields:
- `defnames: Vec<Node>` — qualified name parts (require ≥1 — reject anonymous)
- `stat_types: Vec<Node>` — kinds (`String` nodes with `"ndistinct"`/`"dependencies"`/`"mcv"`). Empty means PG default = all three.
- `exprs: Vec<Node>` — column references (`ColumnRef`) or expressions
- `relations: Vec<Node>` — `RangeVar` of target table. Always 1 entry.
- `if_not_exists: bool` — accepted; pgevolve ignores

```rust
pub fn parse_create_statistics(
    stmt: &CreateStatsStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<QualifiedName, Statistic>,
) -> Result<(), ParseError> {
    // 1. Extract name. defnames must have ≥1 component.
    if stmt.defnames.is_empty() {
        return Err(ParseError::StatisticAnonymous(source_loc));
    }
    let qname = qualified_name_from_defnames(&stmt.defnames, source_loc)?;

    // 2. Reject duplicates.
    if existing.contains_key(&qname) {
        return Err(ParseError::DuplicateStatistic(qname, source_loc));
    }

    // 3. Parse kinds.
    let kinds = if stmt.stat_types.is_empty() {
        StatisticKinds::pg_default()  // PG default when omitted
    } else {
        parse_statistic_kinds(&stmt.stat_types, &qname, source_loc)?
    };

    // 4. Parse target.
    let target = qualified_name_from_range_var(stmt.relations.first(), source_loc)?;

    // 5. Parse columns / expressions.
    let columns = parse_statistic_columns(&stmt.exprs, &qname, source_loc)?;

    // Build and insert.
    existing.insert(qname.clone(), Statistic {
        qname,
        target,
        kinds,
        columns,
        statistics_target: None,
        owner: None,
        comment: None,
    });
    Ok(())
}

pub fn parse_alter_statistics(
    stmt: &AlterStatsStmt,
    source_loc: SourceLocation,
    existing: &mut BTreeMap<QualifiedName, Statistic>,
) -> Result<(), ParseError> {
    // AlterStatsStmt has: defnames (the statistic name), stxstattarget (the new value).
    let qname = qualified_name_from_defnames(&stmt.defnames, source_loc)?;
    let stat = existing.get_mut(&qname).ok_or_else(|| {
        ParseError::AlterStatisticBeforeCreate(qname.clone(), source_loc)
    })?;
    stat.statistics_target = Some(stmt.stxstattarget);  // adapt to actual field shape
    Ok(())
}
```

Plus helpers `parse_statistic_kinds`, `parse_statistic_columns`, `qualified_name_from_defnames`, `qualified_name_from_range_var`. Mirror the helper style from `parse/builder/publication_stmt.rs` (Stage 6 of v0.3.4) and `subscription_stmt.rs` (Stage 6 of v0.3.5).

If `qualified_name_from_range_var` already exists in `parse/builder/shared.rs` (it almost certainly does — every CREATE/ALTER stmt parser uses it), import + reuse rather than duplicate. If it doesn't, the implementation pulls `RangeVar.schemaname` + `RangeVar.relname` and calls `Identifier::from_unquoted` on each.

`parse_statistic_kinds` walks `stat_types: &[Node]`, where each entry is a `String` node with value `"ndistinct"` / `"dependencies"` / `"mcv"`. Anything else → `ParseError::UnknownStatisticKind`. Returns `StatisticKinds` with matching flags set.

`parse_statistic_columns` walks `exprs: &[Node]`. For each node:
- `ColumnRef` with a single-field `fields: [String { sval: "name" }]` → `StatisticColumn::Column(Identifier::from_unquoted(name))`.
- Any other expression node → wrap in a synthetic `SELECT (expr) FROM _t` and feed through `NormalizedExpr::from_sql` (or the codebase's equivalent — `subscription_password_in_source` lint and `publication_row_filter_references_unmanaged_column` lint both built tiny expression extractors; mirror).

**Column parsing**: each `exprs` entry is either a `ColumnRef` (plain column) or some other expression node. If `ColumnRef` with exactly one field → `StatisticColumn::Column(Identifier)`. Otherwise → run through `NormalizedExpr::from_node` (or equivalent) for `StatisticColumn::Expression`.

**Reject CREATE STATISTICS … INCLUDE**: pg_query encodes INCLUDE in a separate field on `CreateStatsStmt` (TBD — may be `include` or part of `stat_types`). Reject with `StatisticIncludeNotSupported` when present.

**Reject RENAME**: handled by `RenameStmt` dispatcher, not `AlterStatsStmt`. Add rejection arm.

### Task 6.3: Wire dispatch + state

- [ ] **Step 1: Add `pub mod statistic_stmt;` and dispatch arms**

```rust
            node::Node::CreateStatsStmt(s) => {
                statistic_stmt::parse_create_statistics(s, loc, &mut statistics)?;
            }
            node::Node::AlterStatsStmt(s) => {
                statistic_stmt::parse_alter_statistics(s, loc, &mut statistics)?;
            }
```

Thread `mut statistics: BTreeMap<QualifiedName, Statistic>` through the parser state. After all statements processed:

```rust
catalog.statistics = statistics.into_values().collect();
```

- [ ] **Step 2: Add RENAME rejection in the existing RenameStmt classifier**

Where ObjectPublication / ObjectSubscription rejection arms live, add:

```rust
ObjectType::ObjectStatisticExt => {
    return Err(ParseError::StatisticRenameNotSupported(
        qualified_name_from_object(&rename.object)?,
        loc,
    ));
}
```

- [ ] **Step 3: COMMENT ON STATISTICS**

The existing comment_stmt parser should already handle generic ObjectType variants. If `ObjectStatisticExt` isn't dispatched yet, add an arm:

```rust
ObjectType::ObjectStatisticExt => {
    let qname = qualified_name_from_object_address(...)?;
    let stat = statistics.get_mut(&qname).ok_or_else(|| {
        ParseError::CommentOnStatisticBeforeCreate(qname, loc)
    })?;
    stat.comment = comment_text;
}
```

### Task 6.4: Unit tests (10+)

Cover:
- CREATE STATISTICS app.s ON a, b FROM app.t — basic, default kinds
- CREATE STATISTICS app.s (ndistinct) ON a, b FROM app.t — explicit kind
- CREATE STATISTICS app.s (ndistinct, dependencies, mcv) ON a, b FROM app.t — all three
- CREATE STATISTICS app.s ON (lower(name)) FROM app.t — expression form
- CREATE STATISTICS app.s ON a, (lower(name)) FROM app.t — mixed
- ALTER STATISTICS app.s SET STATISTICS 1000 — folded with prior CREATE
- COMMENT ON STATISTICS app.s IS '...' — folded
- CREATE STATISTICS ON (a, b) FROM app.t (no name) → ParseError::StatisticAnonymous
- CREATE STATISTICS app.s ON a, b FROM app.t INCLUDE (c) → ParseError::StatisticIncludeNotSupported
- ALTER STATISTICS app.s RENAME TO new → ParseError::StatisticRenameNotSupported
- CREATE STATISTICS app.s (bogus) ON a FROM app.t → ParseError::UnknownStatisticKind
- COMMENT ON STATISTICS app.s IS '...' before CREATE → ParseError::CommentOnStatisticBeforeCreate

### Task 6.5: Verify + commit

```bash
cargo test -p pgevolve-core --lib parse::builder::statistic_stmt
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/pgevolve-core/src/parse/
git commit -m "$(cat <<'EOF'
feat(parse): CREATE / ALTER / COMMENT STATISTICS

Folds CREATE STATISTICS + ALTER STATISTICS SET STATISTICS +
COMMENT ON STATISTICS into one canonical Statistic per qname.
Default kinds (all three) when source omits the (kinds) clause.

Rejects: anonymous form (no name), INCLUDE clause (deferred to
v0.4.x), RENAME (no renames in pgevolve).

Stage 6 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 7 — STATISTICS differ

Granular per-statistic diff: structural change → `ReplaceStatistic`, `statistics_target` change → `AlterStatisticSetTarget`, owner / comment changes independent.

**Files created:** `crates/pgevolve-core/src/diff/statistics.rs`.
**Files modified:** `crates/pgevolve-core/src/diff/change.rs`, `crates/pgevolve-core/src/diff/mod.rs`, `crates/pgevolve-core/src/diff/owner_op.rs`.

### Task 7.1: Add 5 Change variants + OwnerObjectKind

In `change.rs`:

```rust
    /// `CREATE STATISTICS ...`
    CreateStatistic(crate::ir::statistic::Statistic),
    /// `DROP STATISTICS ...` — destructive.
    DropStatistic { qname: crate::identifier::QualifiedName },
    /// `DROP STATISTICS old; CREATE STATISTICS new;` — destructive; used
    /// when columns / kinds / target table differ (PG has no in-place ALTER
    /// for those fields).
    ReplaceStatistic {
        from: crate::ir::statistic::Statistic,
        to: crate::ir::statistic::Statistic,
    },
    /// `ALTER STATISTICS s SET STATISTICS n` — analyze target.
    AlterStatisticSetTarget {
        qname: crate::identifier::QualifiedName,
        value: i32,
    },
    /// `COMMENT ON STATISTICS s IS '...'`
    CommentOnStatistic {
        qname: crate::identifier::QualifiedName,
        comment: Option<String>,
    },
```

In `owner_op.rs`:

```rust
    Statistic,
```

Plus the Display arm: `"STATISTICS"`.

### Task 7.2: Implement differ

```rust
//! Differ for statistics. Per-statistic granular diff:
//! - Structural change (columns / kinds / target) → ReplaceStatistic (skip the rest).
//! - statistics_target differs → AlterStatisticSetTarget.
//! - owner differs (lenient) → AlterObjectOwner.
//! - comment differs → CommentOnStatistic.

use std::collections::BTreeMap;

use crate::diff::change::{Change, ChangeSet};
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, OwnerObjectKind};
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::statistic::Statistic;

pub fn diff_statistics(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&QualifiedName, &Statistic> =
        target.statistics.iter().map(|s| (&s.qname, s)).collect();
    let source_map: BTreeMap<&QualifiedName, &Statistic> =
        source.statistics.iter().map(|s| (&s.qname, s)).collect();

    // Creates.
    for (qname, src) in &source_map {
        if !target_map.contains_key(qname) {
            out.push(
                Change::CreateStatistic((*src).clone()),
                Destructiveness::Safe,
            );
        }
    }

    // Drops: lenient — no auto-drop. Surfaces via unmanaged-statistic lint.

    // Modifies.
    for (qname, src) in &source_map {
        let Some(tgt) = target_map.get(qname) else { continue; };
        diff_one(tgt, src, out);
    }
}

fn diff_one(target: &Statistic, source: &Statistic, out: &mut ChangeSet) {
    // Structural change → ReplaceStatistic; skip rest.
    if target.columns != source.columns
        || target.kinds != source.kinds
        || target.target != source.target
    {
        out.push(
            Change::ReplaceStatistic {
                from: target.clone(),
                to: source.clone(),
            },
            Destructiveness::RequiresApproval {
                reason: format!("structural change to statistic {} requires DROP + CREATE", source.qname),
            },
        );
        return;
    }

    // statistics_target diff.
    if let Some(s_target) = source.statistics_target
        && target.statistics_target != Some(s_target)
    {
        out.push(
            Change::AlterStatisticSetTarget {
                qname: source.qname.clone(),
                value: s_target,
            },
            Destructiveness::Safe,
        );
    }

    // Owner: v0.3.1 lenient.
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        let from = target.owner.clone().unwrap_or_else(|| {
            Identifier::from_unquoted("__unknown_owner__").expect("literal valid")
        });
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Statistic,
                qname: source.qname.clone(),
                signature: String::new(),
                from,
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment.
    if target.comment != source.comment {
        out.push(
            Change::CommentOnStatistic {
                qname: source.qname.clone(),
                comment: source.comment.clone(),
            },
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    // 10+ tests covering:
    // - identical statistics → empty diff
    // - source has it, target doesn't → CreateStatistic
    // - target has it, source doesn't → NO change (lenient)
    // - columns differ → ReplaceStatistic
    // - kinds differ → ReplaceStatistic
    // - target table differs → ReplaceStatistic
    // - structural change skips per-field downstream diffs
    // - only statistics_target differs → AlterStatisticSetTarget
    // - only owner differs → AlterObjectOwner
    // - source owner None, target owner Some → no diff (lenient)
    // - only comment differs → CommentOnStatistic
}
```

### Task 7.3: Wire into top-level diff + 5 stub arms in Change consumers

Mirror v0.3.5 Stage 7's stub approach. The 5 Change consumers are:
- `plan/rewrite/mod.rs` — combined no-op arm for the 5 variants
- `plan/ordering.rs::partition` — `CreateStatistic` → creates; `DropStatistic`/`ReplaceStatistic` → drops/modifies as appropriate; other 2 → modifies
- `plan/ordering.rs::change_node` — placeholder `NodeId::Table(target.clone())` for now; Stage 8 wires `NodeId::Statistic`
- `commands/diff.rs::print_human` + `change_kind_name` — placeholder strings

### Task 7.4: Verify + commit

```bash
cargo test -p pgevolve-core --lib diff::statistics
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/pgevolve-core/src/diff/ crates/pgevolve-core/src/plan/ordering.rs crates/pgevolve-core/src/plan/rewrite/mod.rs crates/pgevolve/src/commands/diff.rs
git commit -m "$(cat <<'EOF'
feat(diff): statistics — 5 granular Change variants

Pair by qname; per-statistic granular diff. Structural changes
(columns/kinds/target) → ReplaceStatistic (DROP + CREATE; PG has no
in-place ALTER). statistics_target → AlterStatisticSetTarget.
Owner/comment independent. Lenient — target-only statistics emit no
auto-drop (surfaces via unmanaged-statistic lint in Stage 9).

5 stub emit arms in 4 Change consumers let the workspace compile.

Stage 7 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 8 — Render + 5 StepKinds + NodeId + dep edges

Fill in SQL helpers, StepKind variants, real emit, NodeId, dep edges. Mirror v0.3.5 Stage 8.

**Files created:** `crates/pgevolve-core/src/plan/rewrite/statistics.rs`.
**Files modified:** `crates/pgevolve-core/src/plan/raw_step.rs`, `crates/pgevolve-core/src/plan/plan.rs`, `crates/pgevolve-core/src/plan/rewrite/mod.rs`, `crates/pgevolve-core/src/plan/edges.rs`, `crates/pgevolve/src/commands/diff.rs`.

### Task 8.1: 5 StepKind variants

```rust
    CreateStatistic,
    DropStatistic,
    ReplaceStatistic,
    AlterStatisticSetTarget,
    CommentOnStatistic,
```

Extend round-trip serialization test. Add `kind_name`/`parse_kind_name` mappings:

```
CreateStatistic         <-> "create_statistic"
DropStatistic           <-> "drop_statistic"
ReplaceStatistic        <-> "replace_statistic"
AlterStatisticSetTarget <-> "alter_statistic_set_target"
CommentOnStatistic      <-> "comment_on_statistic"
```

### Task 8.2: SQL helper module

Create `crates/pgevolve-core/src/plan/rewrite/statistics.rs`:

```rust
//! SQL rendering for STATISTICS operations.

use crate::identifier::QualifiedName;
use crate::ir::statistic::{Statistic, StatisticColumn, StatisticKinds};

#[must_use]
pub fn create_statistic(s: &Statistic) -> String {
    let mut out = format!("CREATE STATISTICS {} ", s.qname.render_sql());
    if !s.kinds.is_default_all() {
        out.push_str(&format!("({}) ", render_kinds(s.kinds)));
    }
    out.push_str("ON ");
    out.push_str(&render_columns(&s.columns));
    out.push_str(&format!(" FROM {};", s.target.render_sql()));
    out
}

#[must_use]
pub fn drop_statistic(qname: &QualifiedName) -> String {
    format!("DROP STATISTICS {};", qname.render_sql())
}

#[must_use]
pub fn replace_statistic(from: &Statistic, to: &Statistic) -> [String; 2] {
    [drop_statistic(&from.qname), create_statistic(to)]
}

#[must_use]
pub fn alter_statistic_set_target(qname: &QualifiedName, value: i32) -> String {
    format!("ALTER STATISTICS {} SET STATISTICS {};", qname.render_sql(), value)
}

#[must_use]
pub fn comment_on_statistic(qname: &QualifiedName, comment: Option<&str>) -> String {
    let body = comment.map_or_else(
        || "NULL".to_string(),
        |c| format!("'{}'", c.replace('\'', "''")),
    );
    format!("COMMENT ON STATISTICS {} IS {};", qname.render_sql(), body)
}

// ---- helpers ----

fn render_kinds(k: StatisticKinds) -> String {
    let mut parts = Vec::new();
    if k.ndistinct    { parts.push("ndistinct"); }
    if k.dependencies { parts.push("dependencies"); }
    if k.mcv          { parts.push("mcv"); }
    parts.join(", ")
}

fn render_columns(cols: &[StatisticColumn]) -> String {
    cols.iter()
        .map(|c| match c {
            StatisticColumn::Column(name) => name.render_sql(),
            StatisticColumn::Expression(e) => format!("({})", e.canonical_text),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl StatisticKinds {
    fn is_default_all(self) -> bool {
        self.ndistinct && self.dependencies && self.mcv
    }
}
```

(`is_default_all` could also live on `StatisticKinds` itself if cleaner — move to ir/statistic.rs in that case.)

8+ unit tests covering each helper.

### Task 8.3: Replace Stage 7 stubs

In `plan/rewrite/mod.rs`, replace the combined stub with 5 explicit arms (mirror v0.3.5 Stage 8's per-variant emit code):

```rust
        Change::CreateStatistic(s) => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateStatistic,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![s.qname.clone()],
                sql: statistics::create_statistic(s),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &s.comment {
                raw_steps.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnStatistic,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![s.qname.clone()],
                    sql: statistics::comment_on_statistic(&s.qname, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        Change::DropStatistic { qname } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::DropStatistic,
                destructive: true,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![qname.clone()],
                sql: statistics::drop_statistic(qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::ReplaceStatistic { from, to } => {
            let [drop_sql, create_sql] = statistics::replace_statistic(from, to);
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::ReplaceStatistic,
                destructive: true,
                destructive_reason: destructive_reason.clone(),
                intent_id: None,
                targets: vec![to.qname.clone()],
                sql: format!("{drop_sql}\n{create_sql}"),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::AlterStatisticSetTarget { qname, value } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterStatisticSetTarget,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: statistics::alter_statistic_set_target(qname, *value),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        Change::CommentOnStatistic { qname, comment } => {
            raw_steps.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnStatistic,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: statistics::comment_on_statistic(qname, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
```

Add `pub mod statistics;` to `plan/rewrite/mod.rs`.

### Task 8.4: NodeId::Statistic + dep edges

In `plan/edges.rs`:

```rust
    Statistic(QualifiedName),
```

Replace Stage 7's `NodeId::Table(target)` placeholder in `change_node` with `NodeId::Statistic(qname)`.

In the dep-graph builder (search for where Stage 8 of v0.3.4 publications added `Publication → Table` edges; the same builder is the target), add an edge from `Statistic → Table` (target table) so statistics create after their target table and drop before:

```rust
for s in &source.statistics {
    let stat_node = NodeId::Statistic(s.qname.clone());
    graph.add_edge(NodeId::Table(s.target.clone()), stat_node.clone(), DepSource::Structural);
}
```

The exact `graph.add_edge` API may differ — match what v0.3.4 publications used in commit `4f90f83` (Stage 8 of `docs/superpowers/plans/2026-05-26-publications.md`).

### Task 8.5: CLI display

In `commands/diff.rs::print_human`:

```rust
        Change::CreateStatistic(s) => format!("+ CREATE STATISTICS {}", s.qname),
        Change::DropStatistic { qname } => format!("- DROP STATISTICS {qname}"),
        Change::ReplaceStatistic { from, to: _ } => format!("~ REPLACE STATISTICS {} (structural change)", from.qname),
        Change::AlterStatisticSetTarget { qname, value } => format!("~ ALTER STATISTICS {qname} SET STATISTICS {value}"),
        Change::CommentOnStatistic { qname, .. } => format!("~ COMMENT ON STATISTICS {qname}"),
```

`change_kind_name` mirrors.

### Task 8.6: Verify + commit

```bash
cargo test -p pgevolve-core --lib plan::rewrite::statistics
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/pgevolve-core/src/plan/ crates/pgevolve/src/commands/
git commit -m "$(cat <<'EOF'
feat(plan): statistics render + emit + 5 new StepKinds + dep edges

plan::rewrite::statistics renders CREATE/DROP/ALTER STATISTICS SQL.
5 new StepKind variants. NodeId::Statistic added; Statistic → Table
dep edge so statistics create after their target table and drop
before.

Stage 8 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 9 — `unmanaged-statistic` lint

Single Warning lint mirroring other v0.3.x `unmanaged-*` rules.

**Files created:** `crates/pgevolve-core/src/lint/rules/unmanaged_statistic.rs`.
**Files modified:** `crates/pgevolve-core/src/lint/rules/mod.rs`, `crates/pgevolve-core/src/lint/universal.rs`.

### Task 9.1: Rule implementation

```rust
//! `unmanaged-statistic` (Warning) — catalog has a statistic source doesn't.

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-statistic";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let source_qnames: std::collections::BTreeSet<_> =
        source.statistics.iter().map(|s| &s.qname).collect();
    target
        .statistics
        .iter()
        .filter(|s| !source_qnames.contains(&s.qname))
        .map(|s| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!("catalog has statistic {} not declared in source", s.qname),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // 4 tests:
    // - empty + empty → silent
    // - source has s, target has s → silent
    // - source has s, target has s + t → fires for t
    // - source has s, target has u → fires for u
}
```

### Task 9.2: Register + wire

- Add `pub mod unmanaged_statistic;` to `lint/rules/mod.rs`.
- Wire into `run_drift_lints` (in `lint/universal.rs`) alongside other `unmanaged-*` rules.

### Task 9.3: Verify + commit

```bash
cargo test -p pgevolve-core --lib lint::rules::unmanaged_statistic
cargo test --workspace --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/pgevolve-core/src/lint/
git commit -m "$(cat <<'EOF'
feat(lint): unmanaged-statistic (Warning, waivable)

Catalog has a statistic source doesn't declare. Standard v0.3.x
lenient-drift pattern; mirrors unmanaged-publication /
unmanaged-subscription / unmanaged-policy / unmanaged-reloption.

Stage 9 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 10 — Conformance fixtures (3 view + 6 statistic = 9)

Standard fixture pattern. All `authoring = "objects"`.

### Task 10.1: VIEW WITH CHECK OPTION fixtures (3)

Under `crates/pgevolve-conformance/tests/cases/objects/views/`:

**1. `create-with-local-check-option/`** (PG 14+):

```sql
-- before.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint PRIMARY KEY, active boolean);

-- after.sql
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint PRIMARY KEY, active boolean);
CREATE VIEW app.live AS SELECT * FROM app.t WHERE active = true
    WITH LOCAL CHECK OPTION;
```

```toml
[meta]
title = "CREATE VIEW WITH LOCAL CHECK OPTION"
authoring = "objects"
spec_refs = ["objects.view.check_option"]
[pg]
min = 14
max = 18
[expect.plan]
steps = 1
```

**2. `create-with-cascaded-check-option/`** — same but `CASCADED`.

**3. `toggle-check-option/`**:
- before: view with `WITH LOCAL CHECK OPTION`
- after: same view with `WITH CASCADED CHECK OPTION`
- Expected: 1 `AlterViewSetCheckOption` step (emits `CREATE OR REPLACE VIEW`).

### Task 10.2: STATISTICS fixtures (6)

Under `crates/pgevolve-conformance/tests/cases/objects/statistics/`:

**1. `create-simple/`** — `(ndistinct, dependencies)` on two columns.

**2. `with-mcv/`** — all three kinds explicit.

**3. `expression-stats/`** — `ON (lower(name))`.

**4. `alter-set-target/`**:
- before: statistic with default target
- after: same statistic with `ALTER STATISTICS app.s SET STATISTICS 1000`
- Expected: 1 `AlterStatisticSetTarget` step.

**5. `replace-on-column-change/`**:
- before: statistic on `(a, b)`
- after: same statistic on `(a, b, c)`
- Expected: 1 `ReplaceStatistic` step (DROP + CREATE; destructive, intent required).

**6. `lint/unmanaged-statistic/`**:
- before.sql seeds catalog with a statistic
- after.sql doesn't declare it
- Expected: 0 plan steps + advisory `unmanaged-statistic`. Mirror v0.3.4's `objects/publications/lint/unmanaged-publication/` for exact fixture.toml shape.

### Task 10.3: Bless + verify

```bash
cargo xtask bless --conformance
cargo test -p pgevolve-conformance
```

Spot-check 3-4 blessed plan.sql files. Especially:
- `create-with-cascaded-check-option/expected/plan.sql` ends with `WITH CASCADED CHECK OPTION`.
- `replace-on-column-change/expected/plan.sql` contains both `DROP STATISTICS` and `CREATE STATISTICS`.
- `expression-stats/expected/plan.sql` preserves the expression text.

### Task 10.4: Commit

```bash
git add crates/pgevolve-conformance/tests/cases/objects/
git commit -m "$(cat <<'EOF'
test(conformance): 3 view-check-option + 6 statistic fixtures

cases/objects/views/{create-with-local,create-with-cascaded,toggle}-check-option/
plus cases/objects/statistics/{create-simple,with-mcv,expression-stats,
alter-set-target,replace-on-column-change,lint/unmanaged-statistic}/.

Stage 10 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage 11 — Proptest + docs + v0.3.7 release

### Task 11.1: Proptest extensions

- [ ] **Step 1: Extend `crates/pgevolve-testkit/src/ir_generator.rs`**

`arb_view` (existing) extended to optionally produce `check_option`:

```rust
fn arb_check_option() -> impl Strategy<Value = Option<CheckOption>> {
    prop_oneof![
        Just(None),
        Just(Some(CheckOption::Local)),
        Just(Some(CheckOption::Cascaded)),
    ]
}
```

Plumb into the existing view-generation strategy.

`arb_statistic` (new):

```rust
fn arb_statistic_kinds() -> impl Strategy<Value = StatisticKinds> {
    (any::<bool>(), any::<bool>(), any::<bool>())
        .prop_filter("at least one kind", |(d, f, m)| *d || *f || *m)
        .prop_map(|(ndistinct, dependencies, mcv)| StatisticKinds { ndistinct, dependencies, mcv })
}

pub fn arb_statistic(target: QualifiedName, col_pool: Vec<Identifier>) -> impl Strategy<Value = Statistic> {
    if col_pool.is_empty() {
        // Defensive: shouldn't happen in arbitrary_catalog wiring.
        return Just(Statistic {
            qname: QualifiedName::new(
                Identifier::from_unquoted("app").unwrap(),
                Identifier::from_unquoted("placeholder").unwrap(),
            ),
            target,
            kinds: StatisticKinds::pg_default(),
            columns: vec![StatisticColumn::Column(Identifier::from_unquoted("id").unwrap())],
            statistics_target: None,
            owner: None,
            comment: None,
        }).boxed();
    }
    (
        identifier_strategy("stat"),
        arb_statistic_kinds(),
        proptest::sample::subsequence(col_pool, 1..=col_pool.len().min(4))
            .prop_map(|cols| cols.into_iter().map(StatisticColumn::Column).collect::<Vec<_>>()),
    )
        .prop_map(move |(name, kinds, columns)| Statistic {
            qname: QualifiedName::new(target.schema.clone(), name),
            target: target.clone(),
            kinds,
            columns,
            statistics_target: None,
            owner: None,
            comment: None,
        })
        .boxed()
}
```

Plumb into `arbitrary_catalog` — generate 0–1 statistics per table.

- [ ] **Step 2: Run 10× per constitution §9**

```bash
for i in 1 2 3 4 5 6 7 8 9 10; do
    echo "=== Run $i ==="
    PROPTEST_CASES=512 cargo test --workspace --release 2>&1 | tail -3
done
```

All 10 green.

- [ ] **Step 3: Commit**

```
test(proptest): statistics + view check_option in arbitrary_catalog

arb_statistic / arb_statistic_kinds draw columns from the target
table's actual columns so generated statistics always reference
real columns. arb_check_option extends arb_view with the new field.

10× per §9; all green.

Stage 11.1 of docs/superpowers/plans/2026-05-27-statistics-and-check-option.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Task 11.2: Docs

- [ ] **Step 1: Update `docs/spec/objects.md`** — flip both rows to ✅:

```markdown
| `STATISTICS` (CREATE STATISTICS) | ✅ Supported | Multi-column statistics objects (ndistinct, dependencies, mcv) + PG14+ expression statistics. Explicit names required (no anonymous form). Granular differ — ALTER SET STATISTICS for target, ReplaceStatistic for any other change. unmanaged-statistic lint. change_kinds: [create, drop, replace, alter_set_target, comment_on] |
| CREATE VIEW … WITH CHECK OPTION | ✅ Supported | Per-view `check_option: Option<CheckOption>` (Local/Cascaded). Both source forms parsed (SQL clause + WITH-options). Diff emits CREATE OR REPLACE VIEW. change_kinds: [alter_set_check_option] |
```

- [ ] **Step 2: Create `docs/spec/statistics.md`** modeled on `publications.md`:

- Source surface (4 forms with examples)
- Lenient semantics — whole-statistic grain
- Granular differ (Replace on structural; SetTarget for the cheap path)
- Lints (unmanaged-statistic)
- Out of scope (anonymous form, INCLUDE clause PG18+, RENAME)
- Catalog reader (pg_statistic_ext + expression-decode via pg_get_expr)

- [ ] **Step 3: Index `docs/spec/statistics.md` in `docs/spec/README.md`**

- [ ] **Step 4: Add cookbook recipe** at `docs/user/cookbook.md` — "Multi-column statistics for correlated columns" with example showing CREATE STATISTICS + ALTER SET STATISTICS.

- [ ] **Step 5: CHANGELOG** — `[0.3.7]` section above `[0.3.6]`:

```markdown
## [0.3.7] — 2026-05-27

### Added

- **`CREATE STATISTICS`** — multi-column statistics objects (ndistinct,
  dependencies, mcv) with PG 14+ expression statistics. Explicit names
  required (anonymous form rejected, mirroring index-naming policy).
  Granular differ: `AlterStatisticSetTarget` for the cheap `SET STATISTICS n`
  path; `ReplaceStatistic` (DROP + CREATE) for any other change since PG
  has no in-place ALTER for column lists or kinds.
- **`CREATE VIEW … WITH CHECK OPTION`** — per-view `check_option:
  Option<CheckOption>` (`Local` | `Cascaded`). Parser folds both SQL-clause
  and WITH-options forms; differ emits `CREATE OR REPLACE VIEW`.
- **5 new StepKind variants for STATISTICS** + **1 for views**:
  `CreateStatistic`, `DropStatistic`, `ReplaceStatistic`,
  `AlterStatisticSetTarget`, `CommentOnStatistic`,
  `AlterViewSetCheckOption`.
- **`unmanaged-statistic` lint** (Warning, waivable) — standard v0.3.x
  lenient-drift surface.
- **9 conformance fixtures** (3 views + 6 statistics).

### Closes

Third and fourth items from the post-v0.3.3 agreed roadmap (`STATISTICS`
was 📋 Planned in `objects.md`; `CREATE VIEW … WITH CHECK OPTION` was 🔮 Future).
```

### Task 11.3: Version bump

```bash
# Root Cargo.toml [workspace.package].version = "0.3.7"
cargo build --workspace
v=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
echo "version: $v"
grep -q "^## \[$v\] — " CHANGELOG.md && echo OK || echo MISMATCH
```

### Task 11.4: §9 verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
cargo doc --workspace --no-deps 2>&1 | grep -cE "^warning"  # expect 0
```

### Task 11.5 — CRITICAL: re-bless conformance + tier-3

v0.3.4/v0.3.5/v0.3.6 each required a re-bless after version bump. Don't skip:

```bash
docker info 2>&1 | head -3

cargo xtask bless --conformance   # plan.sql goldens (version-hash includes pgevolve_version)
cargo test -p pgevolve-conformance

cargo xtask bless                 # tier-3 catalog snapshots (Catalog gains `statistics`; View gains `check_option`)
cargo test -p pgevolve-core --test catalog_round_trip
```

### Task 11.6: Release commit

```bash
git add docs/spec/objects.md docs/spec/statistics.md docs/spec/README.md docs/user/cookbook.md CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pgevolve-conformance/tests/cases/ crates/pgevolve-core/tests/fixtures/catalog/
git commit -m "$(cat <<'EOF'
release: v0.3.7 — STATISTICS + VIEW WITH CHECK OPTION

CREATE STATISTICS as a first-class IR object: ndistinct/dependencies/mcv
kinds plus PG 14+ expression statistics. Explicit names required.
Granular differ — AlterStatisticSetTarget for the cheap path,
ReplaceStatistic for structural changes (PG has no in-place ALTER
for column lists or kinds).

CREATE VIEW … WITH CHECK OPTION as a drop-in field on View. Both
source forms parsed; CREATE OR REPLACE VIEW handles the change.

5 new StepKinds for statistics + 1 for views. 1 new lint
(unmanaged-statistic). 9 conformance fixtures. Re-blessed plan IDs
+ tier-3 catalog snapshots for the v0.3.7 version bump and the
new IR fields.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 11.7: STOP

NO `git tag`, NO `git push`, NO `gh issue close`. Report DONE.

---

## Done.

After Stage 11, v0.3.7 is committed locally and ready for tagging.

Next plan target per `docs/spec/roadmap.md`: **v0.3.8 — `CREATE COLLATION` + `RANGE TYPE`**.
