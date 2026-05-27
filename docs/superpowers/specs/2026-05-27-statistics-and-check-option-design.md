# v0.3.7 sub-spec — `CREATE STATISTICS` + `CREATE VIEW … WITH CHECK OPTION`

**Status:** approved 2026-05-27. Bundles two independent small features per `docs/spec/roadmap.md`.

**Goal:** Ship v0.3.7 — add first-class IR for Postgres `CREATE STATISTICS` (multi-column statistics objects) and a per-view `check_option: Option<CheckOption>` field on existing `View` IR.

**Non-goals:**
- Anonymous `CREATE STATISTICS ON (...) FROM t` (no explicit name) — rejected at parse time, mirrors the no-anonymous-indexes policy in `docs/spec/objects.md`.
- `ALTER STATISTICS s RENAME TO …` — no renames in pgevolve, ever.
- `CREATE STATISTICS … INCLUDE (col)` (PG 18+) — deferred to v0.4.x.
- `WITH CHECK OPTION` on materialized views — PG does not support it there.
- Index-level statistics (`CREATE INDEX … WHERE` predicate stats are PG-internal, not user-declarable).

Both features ship in a single release because each is small and they're operationally unrelated (no cross-coupling in the implementation). Treating them as one release minimizes ceremony overhead.

---

## Sub-spec 1: VIEW WITH CHECK OPTION

### Mental model

`WITH [LOCAL | CASCADED] CHECK OPTION` is a per-view declarative constraint enforcing the view's `WHERE` predicate on `INSERT` / `UPDATE` through the view. PG models it as a property of the view; pgevolve models it as an `Option<CheckOption>` field on `View` for v0.3.x lenient drift consistency.

### IR shape

Add to `crates/pgevolve-core/src/ir/view.rs::View`:

```rust
/// `WITH [LOCAL | CASCADED] CHECK OPTION`, when set in source.
/// `None` = unmanaged (lenient — operator may have set it out-of-band;
/// pgevolve neither sets nor resets unless source declares).
pub check_option: Option<CheckOption>,
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckOption {
    /// `WITH LOCAL CHECK OPTION` — applies only to this view's predicate.
    Local,
    /// `WITH CASCADED CHECK OPTION` — applies through chained updatable views.
    Cascaded,
}
```

`MaterializedView` does NOT get this field (PG does not support CHECK OPTION on MVs).

### Source surface

Two equivalent forms in source SQL, both parsed and folded:

```sql
-- SQL-clause form
CREATE VIEW app.active_users AS
    SELECT * FROM app.users WHERE deleted_at IS NULL
WITH LOCAL CHECK OPTION;

-- WITH-options form
CREATE VIEW app.active_users WITH (check_option = 'cascaded') AS
    SELECT * FROM app.users WHERE deleted_at IS NULL;
```

Parser folds both into `CheckOption::{Local, Cascaded}`. Source-side bare `WITH CHECK OPTION` (no `LOCAL` / `CASCADED` keyword) defaults to `Cascaded` per PG semantics.

### Catalog reader

`information_schema.views.check_option` returns `'NONE'` / `'LOCAL'` / `'CASCADED'`. Decode directly:

```sql
SELECT v.viewname, v.schemaname, vv.check_option, ...
FROM pg_views v
JOIN information_schema.views vv
    ON vv.table_schema = v.schemaname AND vv.table_name = v.viewname
WHERE ...
```

`'NONE'` → `None` ; `'LOCAL'` → `Some(CheckOption::Local)`; `'CASCADED'` → `Some(CheckOption::Cascaded)`.

Add to the existing views-read query in `catalog/queries/shared.rs`.

### Differ

Pair by qname (existing). When `check_option` differs:

```rust
Change::AlterViewSetCheckOption {
    qname: QualifiedName,
    new_value: Option<CheckOption>,
}
```

Renders as `CREATE OR REPLACE VIEW` (PG supports check-option change in place; no DROP needed). One new `StepKind::AlterViewSetCheckOption`. Non-destructive.

No new lints — `Option<CheckOption>` with the lenient semantics self-documents.

### Conformance

3 new fixtures under `objects/views/`:
- `create-with-local-check-option/` — verifies the SQL-clause form
- `create-with-cascaded-check-option/` — verifies the `WITH (check_option = 'cascaded')` form parses to the same IR
- `toggle-check-option/` — Local → Cascaded → unset, exercising the differ's `AlterViewSetCheckOption` emit

---

## Sub-spec 2: CREATE STATISTICS

### Mental model

`CREATE STATISTICS` declares multi-column statistics objects that PG's planner uses for correlated columns. pgevolve manages them as first-class IR with explicit names (no anonymous form). Three kinds (`ndistinct`, `dependencies`, `mcv`) plus expression statistics. The only in-place ALTER is `SET STATISTICS n` (target); any other change requires DROP + CREATE (PG has no in-place ALTER for column lists or kinds).

### IR shape

New module `crates/pgevolve-core/src/ir/statistic.rs`:

```rust
/// Declarative model of a Postgres `CREATE STATISTICS` object.
pub struct Statistic {
    /// Schema-qualified statistic name. Explicit names required;
    /// anonymous `CREATE STATISTICS ON (...) FROM t` is rejected.
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
    /// Object owner. `None` = unmanaged (v0.3.1 pattern).
    pub owner: Option<Identifier>,
    /// Optional `COMMENT ON STATISTICS`.
    pub comment: Option<String>,
}

/// Which `kinds` flags are enabled on a `CREATE STATISTICS` object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatisticKinds {
    /// `ndistinct` — multi-column n-distinct counts.
    pub ndistinct: bool,
    /// `dependencies` — functional dependencies between columns.
    pub dependencies: bool,
    /// `mcv` — most-common-value lists per column combination.
    pub mcv: bool,
}

impl StatisticKinds {
    /// True iff at least one kind is enabled. Canon rejects all-false.
    pub const fn is_empty(&self) -> bool {
        !self.ndistinct && !self.dependencies && !self.mcv
    }
}

/// A single entry in the statistic's column list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatisticColumn {
    /// Plain `column_name` reference.
    Column(Identifier),
    /// Expression statistic (PG 14+): `(lower(name))`.
    /// Canonicalized via `NormalizedExpr`.
    Expression(NormalizedExpr),
}
```

`Catalog::statistics: Vec<Statistic>` — sorted by `qname` in `sort_and_dedupe`.

### Canon validation

In `crates/pgevolve-core/src/ir/canon/statistics.rs`:

- `StatisticKinds::is_empty()` → `IrError::EmptyStatisticKinds(qname)` (PG requires ≥1 kind).
- `Statistic.columns` is empty → `IrError::EmptyStatisticColumns(qname)`.
- Sort + dedupe `columns` (column-form by Identifier; expression-form by `canonical_text`).
- Mixed-form columns (Column + Expression in the same list) are allowed and sorted with Columns first, Expressions second.

### Source surface

```sql
-- Explicit kinds + multi-column.
CREATE STATISTICS app.orders_corr (ndistinct, dependencies)
    ON customer_id, status, placed_at
    FROM app.orders;

-- Default kinds (all three — PG default when kinds clause omitted).
CREATE STATISTICS app.orders_full
    ON customer_id, status
    FROM app.orders;

-- Expression statistic (PG 14+).
CREATE STATISTICS app.orders_lower_email
    ON (lower(email))
    FROM app.orders;

-- Mixed column + expression.
CREATE STATISTICS app.orders_mixed
    ON status, (date_trunc('day', placed_at))
    FROM app.orders;

-- @pgevolve owner: stats_admin
ALTER STATISTICS app.orders_corr SET STATISTICS 1000;
COMMENT ON STATISTICS app.orders_corr IS 'multi-col stat for query planner';
```

The parser folds inline kinds + later `ALTER STATISTICS … SET STATISTICS …` + `COMMENT ON STATISTICS …` into one canonical `Statistic` per qname (mirrors v0.3.4 PUBLICATION fold pattern).

**Source-side rejections (parse-time):**
- `CREATE STATISTICS ON (...) FROM t` (no name) → `ParseError::StatisticAnonymous` with a suggested rewrite using an explicit name.
- `ALTER STATISTICS s RENAME TO …` → `ParseError::StatisticRenameNotSupported`.
- `CREATE STATISTICS … INCLUDE (col)` (PG 18+) → `ParseError::StatisticIncludeNotSupported` (deferred to v0.4.x per roadmap).

### Catalog reader

Primary table: `pg_statistic_ext` joined with:
- `pg_namespace` (schema for the statistic itself)
- `pg_class` (the target table; from `stxrelid`)
- `pg_attribute` (column names; via `stxkeys::int2[]`)
- `pg_authid` (owner from `stxowner`)
- `pg_description` (comment; `classoid = 'pg_statistic_ext'::regclass`, `objsubid = 0`)

Per-PG SQL constants in `catalog/queries/shared.rs`. No PG version variants expected (the surface is stable across 14–18).

**Decoders:**
- `stxkind` is `char[]` with values: `d`=ndistinct, `f`=dependencies, `m`=mcv, `e`=expression-only (internal, implies `Expression` entries exist). Decode to `StatisticKinds` ignoring the `e` marker (it's derived, not user-toggleable).
- `stxstattarget` (or `-1` default) → `statistics_target: Option<i32>` (`-1` → `None`).
- `stxexprs` is a `pg_node_tree`-typed text column. For each expression entry, call `pg_get_expr(stxexprs, stxrelid)` to get SQL text, then feed through `NormalizedExpr::from_sql` for canon (same canon as CHECK / USING / RLS).
- `stxkeys::int2[]` → resolve each `attnum` to `Identifier` via `pg_attribute` join.

### Differ

Pair by `qname`. Per-statistic cases:

| Source | Target | Emits |
|---|---|---|
| present | absent | `Change::CreateStatistic(Statistic)` (Safe) |
| absent | present | no auto-drop (lenient); `unmanaged-statistic` warning |
| both | both | granular (see below) |

**Granular diff when both present:**

The differ checks fields in this order, each independently emitting a Change:

1. **Structural change** — `columns`, `kinds`, or `target` differ → `Change::ReplaceStatistic { from, to }` (DROP + CREATE; PG has no in-place ALTER for these fields). When emitted, the rest of the per-field checks below are SKIPPED for that statistic — the recreate re-establishes everything.
2. **`statistics_target` differs** (and no structural change above) → `Change::AlterStatisticSetTarget { qname, value }` (cheap; uses PG's `ALTER STATISTICS s SET STATISTICS n` form).
3. **Owner differs** (lenient — only when source's owner is `Some`) → standard `Change::AlterObjectOwner` with `OwnerObjectKind::Statistic`.
4. **Comment differs** → `Change::CommentOnStatistic { qname, comment }`.

Comment is independent so a comment-only edit doesn't trigger a full statistic recreate. Owner is independent so an owner change on a structurally-unchanged statistic emits just the ALTER OWNER.

### Planner step kinds

```rust
CreateStatistic,
DropStatistic,                // destructive (intent required)
ReplaceStatistic,             // destructive — DROP + CREATE
AlterStatisticSetTarget,
CommentOnStatistic,
```

5 new step kinds. All transactional. `Drop` and `Replace` are destructive (data-loss-free but irreversible decision; intent required).

### Lint rules

- **`unmanaged-statistic`** (Warning, waivable) — catalog has statistic source doesn't declare. Mirrors `unmanaged-publication` / `unmanaged-subscription` / `unmanaged-reloption`.

### Dependency graph

`NodeId::Statistic(QualifiedName)` joins the existing enum.

Edges:
- `Statistic → Table` (target table). Statistics create after the table they reference; drop before.

Expression-statistic columns don't add edges — the expression's body dependencies (e.g., functions called inside `lower(email)`) are *not* parsed for v0.3.7. Adding statistic-expression dep edges is a future enhancement; for now, the implicit `Statistic → Table` edge plus the planner's tier ordering is sufficient for correctness.

### Conformance

6 fixtures under `crates/pgevolve-conformance/tests/cases/objects/statistics/`:

| # | Fixture | Verifies |
|---|---|---|
| 1 | `create-simple/` | ndistinct + dependencies on two columns |
| 2 | `with-mcv/` | all three kinds explicit |
| 3 | `expression-stats/` | `ON (lower(name))` parses + round-trips through `NormalizedExpr` |
| 4 | `alter-set-target/` | only `SET STATISTICS` path — single `AlterStatisticSetTarget` step |
| 5 | `replace-on-column-change/` | adding a column triggers `ReplaceStatistic` (DROP + CREATE) |
| 6 | `lint/unmanaged-statistic/` | catalog has statistic source doesn't → warning advisory |

### Property tests

Extend `crates/pgevolve-testkit/src/ir_generator.rs` with `arb_statistic`:

```rust
fn arb_statistic_kinds() -> impl Strategy<Value = StatisticKinds>;
fn arb_statistic_columns(pool: Vec<Identifier>) -> BoxedStrategy<Vec<StatisticColumn>>;
pub fn arb_statistic(table_pool: Vec<QualifiedName>, col_pool: Vec<Identifier>) -> BoxedStrategy<Statistic>;
```

Generate 0–1 statistics per catalog, drawing the target table from the catalog's actual tables and column names from that table's actual columns. Expression-form columns use a fixed-pool strategy (`Just(NormalizedExpr::from_sql("(lower(name))").unwrap())`) to keep canon expectations stable.

---

## Release shape

v0.3.7 ships both. Single release commit; CHANGELOG calls out both features in separate sub-bullets.

Same release ceremony as v0.3.4–v0.3.6:
1. Signed tag (`git tag -s v0.3.7 -m "…"`).
2. Push main + tag.
3. `cargo publish -p pgevolve-core` then `cargo publish -p pgevolve`.
4. Re-bless conformance plan IDs (version bump shifts every hash) AND tier-3 catalog snapshots (Catalog gains the `statistics` field; the View struct gains the `check_option` field).

## File / module additions

```
crates/pgevolve-core/src/
├── ir/
│   ├── view.rs                          MODIFY — add check_option: Option<CheckOption>
│   ├── statistic.rs                     NEW — Statistic, StatisticKinds, StatisticColumn
│   ├── catalog.rs                       MODIFY — add statistics field
│   ├── mod.rs                           MODIFY — re-export statistic
│   └── canon/
│       ├── mod.rs                       MODIFY — wire statistics pass
│       └── statistics.rs                NEW — validate + sort
├── catalog/
│   ├── statistics.rs                    NEW — decoder
│   ├── queries/shared.rs                MODIFY — STATISTICS_QUERY + views query gets check_option
│   ├── assemble/
│   │   └── statistics.rs                NEW — assembler
│   └── mod.rs                           MODIFY — wire into read_catalog
├── parse/
│   └── builder/
│       ├── view_stmt.rs                 MODIFY — extract check_option from CREATE/REPLACE VIEW
│       ├── statistic_stmt.rs            NEW — CREATE/ALTER/COMMENT STATISTICS
│       └── mod.rs                       MODIFY — dispatch
├── diff/
│   ├── views.rs                         MODIFY — emit AlterViewSetCheckOption
│   ├── statistics.rs                    NEW — per-statistic granular diff
│   ├── change.rs                        MODIFY — 6 new variants (1 view + 5 statistic)
│   ├── mod.rs                           MODIFY — call diff_statistics
│   └── owner_op.rs                      MODIFY — OwnerObjectKind::Statistic
├── plan/
│   ├── raw_step.rs                      MODIFY — 6 new StepKind variants
│   ├── plan.rs                          MODIFY — extend kind_name / parse_kind_name
│   ├── edges.rs                         MODIFY — add NodeId::Statistic + dep edges
│   └── rewrite/
│       ├── views.rs                     MODIFY — render WITH CHECK OPTION in create_view + new alter helper
│       ├── statistics.rs                NEW — SQL helpers
│       └── mod.rs                       MODIFY — dispatch new variants
└── lint/
    ├── rules/
    │   ├── unmanaged_statistic.rs       NEW
    │   └── mod.rs                       MODIFY
    └── universal.rs                     MODIFY — wire unmanaged-statistic into run_drift_lints

crates/pgevolve-conformance/tests/cases/objects/
├── views/                               MODIFY — 3 new check-option fixtures
└── statistics/                          NEW — 6 fixtures

crates/pgevolve-testkit/src/
└── ir_generator.rs                      MODIFY — arb_statistic strategies + plumb into arbitrary_catalog

docs/spec/
├── objects.md                           MODIFY — flip STATISTICS + VIEW CHECK OPTION rows to ✅
├── statistics.md                        NEW — capability page
└── README.md                            MODIFY — index statistics.md

CHANGELOG.md                              MODIFY — [0.3.7] section
Cargo.toml                                MODIFY — version 0.3.6 → 0.3.7
```
