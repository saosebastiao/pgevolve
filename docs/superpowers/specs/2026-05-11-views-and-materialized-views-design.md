# pgevolve v0.2 — Views and Materialized Views

- Status: draft, awaiting review
- Date: 2026-05-11
- Authors: Daniel Toone
- Scope: v0.2 sub-spec — `VIEW`, `MATERIALIZED VIEW`, and adjacent surface

## 1. Overview

This sub-spec adds `VIEW` and `MATERIALIZED VIEW` (MV) management to pgevolve. v0.1 ships with full table / column / index / constraint / sequence support; views and MVs are the next user-visible object family on the roadmap.

The design leans on three v0.1 invariants:

1. **The IR is what both sides canonicalize to.** Both source and catalog sides produce identical `View` / `MaterializedView` IR records by running their respective body text through the same `NormalizedBody::from_sql` canonicalizer. Source reads the original `CREATE VIEW` body; the catalog reader reads `pg_get_viewdef(oid, true)` from the live DB. Both pass through the same canonicalizer and produce byte-equal `canonical_text`.
2. **No data loss without explicit approval.** `DROP VIEW` is destructive (it removes a queryable surface); `DROP MATERIALIZED VIEW` is *not* destructive (MV rows are derived from base tables).
3. **Every plan step is reviewable.** Dependent-view recreations are emitted as explicit `DROP VIEW` + `CREATE VIEW` sequences rather than `DROP ... CASCADE`, so every view that gets recreated appears by name in `plan.sql`.

## 2. Scope

### In scope

| Feature | Notes |
|---|---|
| `CREATE VIEW` (basic SELECT body) | |
| Aliased column lists (`CREATE VIEW v (a, b) AS ...`) | |
| `CREATE OR REPLACE VIEW` | Used by the planner when the new column list is a superset of the old (see §6.2). |
| `DROP VIEW` | Destructive — requires `intent.toml` approval. |
| `ALTER VIEW ... SET (security_barrier = ...)` | Boolean reloption. |
| `ALTER VIEW ... SET (security_invoker = ...)` | Boolean reloption; PG 15+ only. |
| `COMMENT ON VIEW` / `COMMENT ON COLUMN` for view columns | Extends the existing v0.1 COMMENT machinery to view subjects. |
| `CREATE MATERIALIZED VIEW` | Always emitted as `WITH NO DATA` + a separate `REFRESH MATERIALIZED VIEW` step (see §6.3). |
| `DROP MATERIALIZED VIEW` | *Not* destructive. |
| `REFRESH MATERIALIZED VIEW [CONCURRENTLY]` | New planner step kind. CONCURRENTLY auto-chosen under `strategy = "online"` when a unique index exists on the MV. |
| `CREATE INDEX` on a materialized view | The existing index machinery is reused; `Index::on` gains an `ObjectRef::Mv(_)` variant. |
| `COMMENT ON MATERIALIZED VIEW` | |
| Renames (`ALTER VIEW ... RENAME TO`, `ALTER MATERIALIZED VIEW ... RENAME TO`) | Modeled as drop+create, consistent with the v0.1 rename policy. |
| Schema moves (`ALTER VIEW ... SET SCHEMA`, `ALTER MATERIALIZED VIEW ... SET SCHEMA`) | Modeled as drop+create. |
| Auto-updatable views | Free — a property of the view body, no IR change. |

### Deferred (future sub-specs)

| Feature | Reason |
|---|---|
| `WITH CHECK OPTION` (`LOCAL` / `CASCADED`) | Pre-existing 🔮 classification in `docs/spec/objects.md`; not blocking. |
| Recursive views (`CREATE RECURSIVE VIEW`) | Body parses as `WITH RECURSIVE ...` anyway; uncommon. |
| MV reloptions (`fillfactor`, autovacuum overrides) | Belongs with the broader "table reloptions" v0.2 sub-spec. |
| `INSTEAD OF` triggers for non-auto-updatable views | Waits for trigger support in the v0.2 trigger sub-spec. |
| `GRANT` / `REVOKE` on views | v0.3 ACL work. |

## 3. Key design decisions

| Decision | Choice | Rationale |
|---|---|---|
| **Body canonicalization** | `NormalizedBody::from_sql` on both source and catalog sides. Source parses the raw `CREATE VIEW` body; catalog re-parses `pg_get_viewdef(oid, true)`. Both produce byte-equal `canonical_text`. | Canonicalization is pgevolve-side, not PG-side. No shadow round-trip required for normal `diff`/`plan` runs. Plans are reproducible without Docker. |
| **OR-REPLACE strategy** | Emit `CREATE OR REPLACE VIEW` when the new column list is a superset of the old; else drop+create. | Eliminates unnecessary churn (and permission churn under future GRANT support) for the common "add a column to a view" case without doubling the planner's test matrix. |
| **MV data is derived** | `DROP MATERIALIZED VIEW` is not destructive; recreations don't require `intent.toml` approval. | Matches how every team treats MVs in practice and avoids friction during routine MV iteration. |
| **MV refresh** | Always emit a `REFRESH MATERIALIZED VIEW` step after `CREATE MATERIALIZED VIEW`. Auto-prefer CONCURRENTLY under `strategy = "online"` when a unique index exists; otherwise emit plain `REFRESH` plus a lint warning. | Predictable plans; matches the opportunistic online-rewrite pattern used by other v0.1 rewrites. |
| **Dependency tracking source** | Source side walks the parsed view body AST. `FromClause` nodes give relation refs; `FuncCall` nodes give function refs; `TypeCast`/`ColumnRef` give type/column refs. Each produces a `DepEdge { source: DepSource::AstExtracted }`. | Compiler-like diagnostics (missing column errors point at source lines), reproducible without containers, no bootstrap circularity. Aligns with arch readiness Decisions 9 and 10. |
| **Dependency tracking granularity** | Column-level (via `ColumnRef` AST nodes). | Avoids recreating unrelated dependent views when a sibling column changes. |
| **Dependent recreation in plan** | Emitted as explicit `DROP VIEW` + `CREATE VIEW` for each affected view, in topo order — never `DROP ... CASCADE`. | Every recreated view is visible by name in `plan.sql` for review. |
| **Shadow validation** | `--shadow-validate` flag boots a shadow PG, applies, reads `pg_depend`, and cross-checks against AST-derived edges. Mismatches are warnings (or errors under `--shadow-strict`). Shadow is opt-in; the normal `diff`/`plan` path requires no container. | Opt-in cross-check for canonicalizer correctness and coverage of edge cases, without making Docker mandatory. Aligns with arch readiness Decision 12. |

## 4. IR additions

In `crates/pgevolve-core/src/ir/`:

```rust
pub struct View {
    pub schema: SchemaName,
    pub name: ObjectName,
    pub columns: Vec<ViewColumn>,            // ordered; reflects CREATE VIEW v (a, b) ... if aliased
    pub body_canonical: NormalizedBody,      // produced by NormalizedBody::from_sql on both source and catalog sides
    pub body_dependencies: Vec<DepEdge>,     // AST-extracted; column-level edges
    pub security_barrier: Option<bool>,      // None = not set; PG default applies
    pub security_invoker: Option<bool>,      // None = not set; PG 15+ only
    pub comment: Option<String>,
}

pub struct MaterializedView {
    pub schema: SchemaName,
    pub name: ObjectName,
    pub columns: Vec<ViewColumn>,
    pub body_canonical: NormalizedBody,
    pub body_dependencies: Vec<DepEdge>,
    pub comment: Option<String>,
}

pub struct ViewColumn {
    pub name: ColumnName,
    pub comment: Option<String>,
}

pub struct DepEdge {
    pub kind: DepKind,                       // Column | Function | Type | Sequence
    pub target: ObjectRef,
    pub subobject: Option<ColumnName>,       // populated when kind = Column
    pub source: DepSource,                   // AstExtracted | AstDeclared (via @pgevolve dep: directive)
}
```

`NormalizedBody` is the type from `crates/pgevolve-core/src/parse/normalize_body.rs` (landed in v0.2-readiness). It carries `canonical_text: String` and `canonical_hash: [u8; 32]` (BLAKE3). The `NormalizedBody::from_sql` constructor walks the `pg_query` AST and emits the canonical form. Both the source pipeline and the catalog pipeline call `NormalizedBody::from_sql`; they never construct `NormalizedBody` from raw strings.

`DepEdge` uses the type from `crates/pgevolve-core/src/plan/edges` (landed in v0.2-readiness), which already carries `DepSource`. There is no restricted constructor: both source and catalog paths call `DepEdge { source: DepSource::AstExtracted, .. }` directly.

The existing `Index::on` field gains an `ObjectRef::Mv(_)` variant alongside `ObjectRef::Table(_)`. No other index changes are required — partial, expression, INCLUDE, opclass, collation, etc., all work identically on MV indexes.

There is no `populated: bool` field on `MaterializedView`. Whether an MV holds rows at a given moment is a runtime catalog fact, not part of the desired-state IR.

## 5. Source and catalog pipelines

### 5.1 Source pipeline (AST-first)

1. The existing source loader parses every `.sql` file with `pg_query`. For each `CreateViewStmt` and `CreateTableAsStmt` (MV) it produces a *provisional* IR record with empty `body_canonical` and empty `body_dependencies`.
2. After all non-view IR (schemas, types, tables, indexes, constraints, sequences, functions) is in the source IR, the loader runs an **AST canonicalization pass** over each view and MV:
   1. For each view/MV, take the raw SELECT body text from the parsed `CreateViewStmt` / `CreateTableAsStmt`.
   2. Re-parse the body with `pg_query` (it was already parsed in step 1; this uses the cached AST).
   3. Call `NormalizedBody::from_sql` on the body AST → fills `body_canonical` with a `NormalizedBody` carrying `canonical_text` and `canonical_hash`.
   4. Walk the body AST: `FromClause` nodes → relation `DepEdge`s; `FuncCall` nodes → function `DepEdge`s; `TypeCast` nodes → type `DepEdge`s; `ColumnRef` nodes → column-level `DepEdge`s. Each edge carries `source: DepSource::AstExtracted`. Fills `body_dependencies`.
   5. Resolve each dep edge target against the provisional IR. An unresolved reference (e.g., view body references a column that doesn't exist) produces an **AST resolution error** with source location, before any DB touch.
3. No shadow round-trip is performed during normal `diff`/`plan` runs. The AST canonicalization pass is fast enough that caching is not required.
4. If a view or MV body contains `-- @pgevolve dep:` directives (recognized by `crates/pgevolve-core/src/parse/directives.rs`, landed in v0.2-readiness), those directives contribute additional `DepEdge { source: DepSource::AstDeclared }` entries alongside the AST-extracted ones.

### 5.2 Catalog pipeline

Two new readers are added to `crates/pgevolve-core/src/catalog/`:

- `read_views`: pulls `pg_class` rows with `relkind = 'v'`, joins `pg_get_viewdef(oid, true)`, `pg_attribute`, `pg_class.reloptions`, and `pg_depend`. For each row, calls `NormalizedBody::from_sql` on the `pg_get_viewdef` output. Produces `Vec<View>`.
- `read_materialized_views`: same shape, `relkind = 'm'`. Produces `Vec<MaterializedView>`.

The catalog reader also walks the parsed `pg_get_viewdef` AST to produce dep edges (same walk as §5.1 step 2.4), producing `DepEdge { source: DepSource::AstExtracted }`. These are used for the dep graph; the raw `pg_depend` rows are not consumed for canonicalization.

The existing index reader is extended to accept `relkind IN ('r', 'm')` as valid index parents, so indexes on MVs flow through the same machinery.

### 5.3 Why the two sides are byte-equal

Both sides parse their input with `pg_query` and pass the resulting AST through `NormalizedBody::from_sql`. Equality is therefore byte-equality on `canonical_text` — the diff predicate is `source.body_canonical.canonical_hash == catalog.body_canonical.canonical_hash`. Because both sides use the same canonicalizer, a source body like `SELECT id, email FROM app.users` and a catalog body like ` SELECT users.id, users.email\n   FROM app.users;` converge to the same canonical form.

The invariant to uphold: `NormalizedBody::from_sql(body).canonical_text == NormalizedBody::from_sql(pg_get_viewdef(apply(body))).canonical_text` for any well-formed view body. This is validated by the fuzz property test in §10.3.

## 6. Diff and planner

### 6.1 Diff change kinds

| Change kind | Trigger | Destructive? |
|---|---|---|
| `View::Create` | source-only view | no |
| `View::Drop` | catalog-only view | **yes** — requires `intent.toml` approval |
| `View::ReplaceBody { compatible: true }` | `body_canonical` differs; new column list is a superset of old | no |
| `View::ReplaceBody { compatible: false }` | `body_canonical` differs; column list narrows or reorders | no (but may cascade dep recreations) |
| `View::SetReloption` | `security_barrier` or `security_invoker` toggled | no |
| `View::SetComment` | view or column comment changes | no |
| `Mv::Create` | source-only MV | no |
| `Mv::Drop` | catalog-only MV | **no** (MV data is derived) |
| `Mv::ReplaceBody` | `body_canonical` differs (PG has no OR-REPLACE for MVs) | no |
| `Mv::SetComment` | comment changes | no |

### 6.2 OR-REPLACE compatibility predicate

`compatible = true` iff every column in the *old* column list appears in the *new* column list at the same index, with the same resolved type and the same name. New columns may be appended at the end. Computed from the IR column lists, not the canonical bodies.

If `compatible = true`: planner emits a single `CREATE OR REPLACE VIEW` step. No dependents are disturbed.

If `compatible = false`: planner emits `DROP VIEW` + `CREATE VIEW`, and recursively recreates every dependent view in topological order (see §6.4).

### 6.3 Step kinds

The plan format's step-kind vocabulary (`plan::raw_step::StepKind`) gains the following variants. Each maps to exactly one SQL statement, consistent with v0.1.

| Step kind | SQL emitted | Notes |
|---|---|---|
| `create_view` | `CREATE VIEW ...` or `CREATE OR REPLACE VIEW ...` | The `or_replace` flag in the step payload distinguishes the two; golden tests assert on it. Used both for fresh creation (`View::Create`) and for compatible body replacement (`View::ReplaceBody { compatible: true }`). |
| `drop_view` | `DROP VIEW <qname>` | No `CASCADE`. Destructive — requires an `[[intent]]` row. Used for `View::Drop` and as the first half of `View::ReplaceBody { compatible: false }`. |
| `create_materialized_view` | `CREATE MATERIALIZED VIEW ... WITH NO DATA` | Always `WITH NO DATA`; the subsequent `refresh_materialized_view` step populates it. |
| `drop_materialized_view` | `DROP MATERIALIZED VIEW <qname>` | No `CASCADE`. Non-destructive. |
| `refresh_materialized_view` | `REFRESH MATERIALIZED VIEW [CONCURRENTLY] <qname>` | `concurrently` flag in the step payload. Emitted after every `create_materialized_view` and `Mv::ReplaceBody` unless suppressed via a `[[step_override]]` row in `intent.toml` (see §8). |
| `alter_view_set_reloption` | `ALTER VIEW <qname> SET ( <option> = <value> )` | Used for `security_barrier` / `security_invoker` flips. |
| `comment_on_view` | `COMMENT ON VIEW ...` / `COMMENT ON COLUMN ...` / `COMMENT ON MATERIALIZED VIEW ...` | Reuses the existing comment-step machinery; the new step kind exists so goldens can disambiguate view comments from table comments. |

A `View::ReplaceBody { compatible: false }` therefore emits two steps: `drop_view` then `create_view` (with `or_replace = false`). Each transitively-recreated dependent view contributes another `drop_view` + `create_view` pair. Existing `create_index` / `drop_index` step kinds cover indexes on MVs without change.

### 6.4 Dependent-view recreation

When a diff produces `View::ReplaceBody { compatible: false }` or `View::Drop`, OR when any upstream change in the diff (column drop, column rename, column type change, table drop, function signature change, type definition change) affects an object that a view's `body_dependencies` points at, the planner walks the dep graph and emits explicit `DROP VIEW` + `CREATE VIEW` steps for every transitively-affected view, in topological order.

`DROP ... CASCADE` is never used. Every recreated view appears by name in `plan.sql`.

Dependent recreations inherit the destructiveness of the *underlying* change. A column drop that forces three views to recreate still requires exactly one `intent.toml` entry (for the column drop); the three view recreations are mechanical follow-on.

### 6.5 Online-rewrite policy

Two new entries in the `[planner.online_rewrites]` section of `pgevolve.toml`:

```toml
[planner.online_rewrites]
refresh_mv_concurrently      = true   # default true; only takes effect when MV has a unique index
view_drop_create_dependents  = true   # default true; if false, planner errors on incompatible view changes
```

`refresh_mv_concurrently = true`: under `strategy = "online"`, if the MV has at least one unique index, the planner emits `REFRESH MATERIALIZED VIEW CONCURRENTLY`. Otherwise it emits plain `REFRESH MATERIALIZED VIEW` and surfaces a **lint warning** (`refresh-concurrently-needs-unique-index`) at plan time pointing at the MV. Under `strategy = "atomic"`, plain `REFRESH` is always emitted.

`view_drop_create_dependents = false`: planner refuses incompatible view changes and surfaces an error naming each affected dependent. Useful for teams that want to gate cascading recreations behind manual review.

## 7. Lints

Three new lint rules ship with this work:

| Lint ID | Severity | Trigger | Fix hint |
|---|---|---|---|
| `view-shadows-table` | error | A view's qname collides with a table that also exists in source. | Rename the view; PG itself would reject this at apply time. |
| `mv-no-unique-index` | warning | MV exists but has no unique index. Under `strategy = "online"` this means refreshes will block reads. | Add a unique index, or switch to `strategy = "atomic"` for this environment. |
| `view-body-references-unmanaged-schema` | warning | View body's `body_dependencies` includes an object in a schema not listed in `[managed].schemas`. | Add the schema to `[managed].schemas`, or accept that pgevolve cannot prove dependency safety for this reference. |

## 8. `intent.toml` impact

The destructive-approval shape is unchanged from v0.1. `DROP VIEW` is the only new destructive change kind this sub-spec introduces; it follows the existing `[[intent]]` schema:

```toml
plan_id = "abc1234567890123"

[[intent]]
id       = 1
step     = 7
kind     = "drop_view"
target   = "app.users_summary"
reason   = "drops view app.users_summary"
approved = false
```

`DROP MATERIALIZED VIEW`, `CREATE OR REPLACE VIEW`, drop-and-recreate dependent recreations, comment updates, and reloption flips are all non-destructive — no `[[intent]]` row required.

**New table: `[[step_override]]`.** Adds a non-destructive per-step modifier so users can suppress the auto-emitted post-create `REFRESH` for specific MVs (the `WITH NO DATA` use case). It is a sibling of `[[intent]]`, not a replacement:

```toml
[[step_override]]
kind     = "refresh_materialized_view"
target   = "app.daily_revenue"
suppress = true
```

The executor honors `[[step_override]]` rows the same way it honors `[[intent]]` rows — read at apply time, recorded in the audit log. Unlike `[[intent]]`, missing `[[step_override]]` rows are not a hard failure: the default behavior (emit the REFRESH) just runs.

This handles `WITH NO DATA` without expanding the destructive surface. The `[[step_override]]` table is reserved for additional non-destructive per-step modifiers in future sub-specs; for now `refresh_materialized_view` is its only consumer.

## 9. File layout

In the default `schema-mirror` layout profile:

```
schema/<schema>/views/<name>.sql                # both VIEW and MATERIALIZED VIEW
schema/<schema>/indexes/<name>.sql              # MV indexes go here, same as table indexes
```

`kind-grouped`, `feature-grouped`, and `free-form` profiles already accept arbitrary `CREATE` statements per their existing rules — no profile-specific changes are required.

## 10. Testing

### 10.1 Conformance fixtures

New fixture groups under `crates/pgevolve-conformance/tests/cases/`:

- `views/create-simple` — single view over one table
- `views/create-with-aliases` — `CREATE VIEW v (a, b) AS ...`
- `views/replace-body-compatible` — column-additive body change; expects `CREATE OR REPLACE VIEW`
- `views/replace-body-incompatible` — column-narrowing body change; expects drop+create
- `views/drop` — view in catalog, missing from source; expects `DROP VIEW` with intent approval
- `views/dependent-recreation` — `v2` selects from `v1`; underlying-table column drop forces both `v1` and `v2` to recreate in topo order
- `views/comment-on-view` — view + column comments
- `views/security-barrier-toggle` — reloption flip via `ALTER VIEW`
- `views/security-invoker-toggle` — PG 15+ only; reloption flip
- `matviews/create-simple` — MV + auto-emitted REFRESH step
- `matviews/create-no-unique-index-online` — under online strategy, expects plain REFRESH plus `mv-no-unique-index` lint warning
- `matviews/refresh-concurrently` — MV with unique index under online strategy
- `matviews/replace-body` — body change forces drop+create+refresh
- `matviews/index-on-mv` — index creation on an MV; exercises `Index::on = ObjectRef::Mv(_)`
- `matviews/with-no-data-override` — `step_override` in `intent.toml` suppresses the post-create REFRESH

Each fixture follows the existing `fixture.toml` + `before.sql` + `after.sql` (+ optional `plan.sql.golden`) layout. Goldens regenerable via `cargo xtask bless --conformance`.

### 10.2 Tier-3 catalog goldens

Catalog read-back fixtures for views and MVs added for PG 14, 15, 16, 17 (security_invoker fixtures only for PG 15+). Regenerable via `cargo xtask bless`.

### 10.3 Property tests (nightly)

Two new generators in `pgevolve-testkit`:

- `arb_view_body` — generates random SELECT bodies over a generated table corpus. Used to fuzz canonicalization: parse body → `NormalizedBody::from_sql` → assert the round-trip invariant holds: `NormalizedBody::from_sql(body).canonical_text == NormalizedBody::from_sql(pg_get_viewdef(apply(body))).canonical_text`. This confirms `NormalizedBody::from_sql` is closed under the PG rewrite: applying a view body to PG and reading it back through the same canonicalizer produces the same `canonical_text` as canonicalizing the original source. Runs against a real PG via the `docker-tests` feature gate.
- `arb_view_dependency_graph` — generates view dependency DAGs (up to N levels of nesting) and asserts that an arbitrary column rename on a leaf table produces a plan that recreates exactly the transitively-affected views, in valid topo order, with no spurious recreations.

Property tests stay `#[ignore]` and run nightly per the policy established in [`2026-05-11-conformance-test-suite-design.md`](./2026-05-11-conformance-test-suite-design.md).

## 11. Edge cases

- **View body references an extension function.** Out of scope for this design — extensions are a separate v0.2 sub-spec. For now, the planner emits a hard error if a view's `body_dependencies` includes a function whose schema is neither in `[managed].schemas` nor a built-in PG schema (`pg_catalog`, `information_schema`). User remedies: add the schema to `[managed].schemas`, or wait for extension support.
- **View references a sequence's `nextval`.** Already supported. The dep edge points at the sequence; no new logic.
- **MV with `WITH NO DATA` in source.** Modeled via a per-step `intent.toml` override (`step_override` with `kind = "refresh_materialized_view"`, `suppress = true`). Default behavior is always to emit the REFRESH.
- **Catalog view body has been hand-edited via `CREATE OR REPLACE VIEW` outside pgevolve.** Detected as a normal diff; the next plan re-applies the source-defined body. No special handling.
- **Source view depends on a column being added in the same plan.** Existing topo sort handles this — the column-add step precedes the view-create step.
- **A view exists in the live DB on a schema *not* in `[managed].schemas`.** Ignored, matching v0.1's schema-scoping model.
- **Two views with the same name in different schemas.** Each is identified by `(schema, name)`; no conflict.
- **A view that selects `*` from a table.** PG expands `*` to an explicit column list at parse time; `pg_get_viewdef` reflects the expanded form. When the catalog reader calls `NormalizedBody::from_sql` on the `pg_get_viewdef` output, the expanded form becomes canonical. The source body's `*` also expands via AST resolution against the source IR. Adding a column to the underlying table changes the canonical body on both sides and produces a `View::ReplaceBody` diff — which is the correct user-facing behavior.

## 12. Documentation updates

When this work ships:

- `docs/spec/objects.md`: flip `VIEW` and `MATERIALIZED VIEW` from 📋 to ✅. Add rows for `security_barrier` and `security_invoker` reloptions. Confirm `WITH CHECK OPTION` and recursive views remain 🔮.
- `docs/spec/lint-and-layout.md`: add the three new lint rules.
- `docs/spec/cli.md`: document the two new `[planner.online_rewrites]` toggles.
- `docs/user/plan-format.md`: document the `refresh_materialized_view` and `replace_view` step kinds plus the `step_override` `intent.toml` shape.
- `docs/user/cookbook.md`: add a "Managing views" entry.
- `docs/system/planner.md`: document the OR-REPLACE compatibility predicate and the dependent-recreation walk.
- `docs/system/ir.md`: document the new `View` / `MaterializedView` IR types and the `DepEdge` shape.

## 13. Rollout

Phasing is left to the implementation plan. The natural ordering is: IR types → source parser → AST canonicalization pass with dep extraction → catalog readers → differ change kinds → planner step kinds and OR-REPLACE predicate → online-rewrite policy → lints → conformance fixtures → documentation. Each forms a reviewable PR.

## 14. Open questions

None blocking. Three implementation-time decisions that don't need answering now:

- How aggressively to deepen `NormalizedBody::canonicalize` for view-specific shapes: `RECURSIVE` views (body is `WITH RECURSIVE ...`; does the canonicalizer handle CTE name normalization?), `LATERAL` joins (do we normalize join order within LATERAL?), set-returning functions in `SELECT` (e.g., `generate_series` — do we strip redundant casts on arguments?). These can be added incrementally; the canonicalizer's test suite is the right venue.
- Whether MV bodies need any special canonicalization beyond what views need. An MV body is structurally identical to a view body; the same `NormalizedBody::from_sql` path handles both. The question is whether there are any MV-specific AST shapes (e.g., `WITH NO DATA` at the statement level) that need explicit canonicalization rules. Likely not — the `WITH NO DATA` clause is a statement-level option, not part of the body SELECT; `pg_get_viewdef` doesn't include it.
- Whether `--shadow-validate` should emit a machine-readable diff of AST-extracted edges vs `pg_depend` edges, to aid canonicalizer development. Can be added later if feedback from early users suggests it.
