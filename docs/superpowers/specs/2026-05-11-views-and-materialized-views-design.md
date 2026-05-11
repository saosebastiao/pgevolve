# pgevolve v0.2 — Views and Materialized Views

- Status: draft, awaiting review
- Date: 2026-05-11
- Authors: Daniel Toone
- Scope: v0.2 sub-spec — `VIEW`, `MATERIALIZED VIEW`, and adjacent surface

## 1. Overview

This sub-spec adds `VIEW` and `MATERIALIZED VIEW` (MV) management to pgevolve. v0.1 ships with full table / column / index / constraint / sequence support; views and MVs are the next user-visible object family on the roadmap.

The design leans on three v0.1 invariants:

1. **The IR is what Postgres says it is.** Both source and catalog sides produce identical `View` / `MaterializedView` IR records by routing the source side through an ephemeral Postgres ("shadow PG") and reading back the same catalog functions the catalog reader uses (`pg_get_viewdef`, `pg_depend`, `pg_class.reloptions`).
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
| **Body canonicalization** | Round-trip through shadow PG; both sides read `pg_get_viewdef(oid, true)`. | PG is the authoritative rewriter; equivalence becomes byte-equality on the canonical form. Avoids reimplementing PG's rewriter. |
| **OR-REPLACE strategy** | Emit `CREATE OR REPLACE VIEW` when the new column list is a superset of the old; else drop+create. | Eliminates unnecessary churn (and permission churn under future GRANT support) for the common "add a column to a view" case without doubling the planner's test matrix. |
| **MV data is derived** | `DROP MATERIALIZED VIEW` is not destructive; recreations don't require `intent.toml` approval. | Matches how every team treats MVs in practice and avoids friction during routine MV iteration. |
| **MV refresh** | Always emit a `REFRESH MATERIALIZED VIEW` step after `CREATE MATERIALIZED VIEW`. Auto-prefer CONCURRENTLY under `strategy = "online"` when a unique index exists; otherwise emit plain `REFRESH` plus a lint warning. | Predictable plans; matches the opportunistic online-rewrite pattern used by other v0.1 rewrites. |
| **Dependency tracking source** | Source side uses shadow `pg_depend`, same as the catalog reader. | Aligns with the body-canonicalization decision; no in-process name resolution. |
| **Dependency tracking granularity** | Column-level (uses `pg_depend.refobjsubid`). | Avoids recreating unrelated dependent views when a sibling column changes. |
| **Dependent recreation in plan** | Emitted as explicit `DROP VIEW` + `CREATE VIEW` for each affected view, in topo order — never `DROP ... CASCADE`. | Every recreated view is visible by name in `plan.sql` for review. |
| **Shadow cost gating** | Shadow pass runs only when the source tree contains at least one view or MV; result is cached keyed by `(source_ir_hash, pg_major)`. | Projects without views pay nothing; projects with views amortize the shadow boot across repeated `diff` / `plan` runs on unchanged source. |

## 4. IR additions

In `crates/pgevolve-core/src/ir/`:

```rust
pub struct View {
    pub schema: SchemaName,
    pub name: ObjectName,
    pub columns: Vec<ViewColumn>,            // ordered; reflects CREATE VIEW v (a, b) ... if aliased
    pub body_canonical: CanonicalViewBody,   // output of pg_get_viewdef(oid, true) from shadow or catalog
    pub body_dependencies: Vec<DepEdge>,     // column-level edges
    pub security_barrier: Option<bool>,      // None = not set; PG default applies
    pub security_invoker: Option<bool>,      // None = not set; PG 15+ only
    pub comment: Option<String>,
}

pub struct MaterializedView {
    pub schema: SchemaName,
    pub name: ObjectName,
    pub columns: Vec<ViewColumn>,
    pub body_canonical: CanonicalViewBody,
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
}

pub struct CanonicalViewBody(String);        // newtype; never constructed except by the shadow-load or catalog-read paths
```

The existing `Index::on` field gains an `ObjectRef::Mv(_)` variant alongside `ObjectRef::Table(_)`. No other index changes are required — partial, expression, INCLUDE, opclass, collation, etc., all work identically on MV indexes.

There is no `populated: bool` field on `MaterializedView`. Whether an MV holds rows at a given moment is a runtime catalog fact, not part of the desired-state IR.

## 5. Source and catalog pipelines

### 5.1 Source pipeline (with shadow round-trip)

1. The existing source loader parses every `.sql` file with `pg_query`. For each `CreateViewStmt` and `CreateTableAsStmt` (MV) it produces a *provisional* IR record with empty `body_canonical` and empty `body_dependencies`.
2. After all non-view IR (schemas, types, tables, indexes, constraints, sequences, functions) is in the source IR, the loader runs a **shadow-load pass**:
   1. Boot an ephemeral PG matching the configured target version (reuse `pgevolve-testkit::EphemeralPostgres`).
   2. Apply the source IR in dependency-correct order (schemas → types → tables/indexes/constraints/sequences → functions → views/MVs → MV indexes). This path already exists for `validate --shadow`; this design promotes it from an optional check to the *normal* path whenever views or MVs are present.
   3. For each view/MV, query `pg_get_viewdef(oid, true)` → fill `body_canonical`; query `pg_depend` filtered to `refclassid IN (pg_class, pg_proc, pg_type)` → fill `body_dependencies` (column-level via `refobjsubid`).
   4. Tear down the shadow container.
3. The shadow result is cached keyed by `(source_ir_hash, pg_major)`. Cache directory: `.pgevolve/cache/shadow/`. The directory is added to the default `init`-generated `.gitignore`. Cache invalidates automatically when any source file changes (the hash includes every parsed source IR record).
4. If the source tree contains zero views and zero MVs, the shadow pass is skipped entirely.

### 5.2 Catalog pipeline

Two new readers are added to `crates/pgevolve-core/src/catalog/`:

- `read_views`: pulls `pg_class` rows with `relkind = 'v'`, joins `pg_get_viewdef`, `pg_attribute`, `pg_class.reloptions`, and `pg_depend`. Produces `Vec<View>`.
- `read_materialized_views`: same shape, `relkind = 'm'`. Produces `Vec<MaterializedView>`.

The existing index reader is extended to accept `relkind IN ('r', 'm')` as valid index parents, so indexes on MVs flow through the same machinery.

### 5.3 Why the two sides are byte-equal

Both `body_canonical` strings come from the same Postgres function (`pg_get_viewdef`) executed against an actual PG instance. Both `body_dependencies` sets come from the same catalog table (`pg_depend`) read the same way. Equality is therefore byte-equality on canonical bodies and set-equality on dep edges — no second canonicalizer to maintain.

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

Two new `-- @pgevolve step` kinds are added to the plan format:

| Step kind | Payload fields | Notes |
|---|---|---|
| `refresh_materialized_view` | `target: <qname>`, `concurrently: bool` | Emitted after every `CREATE MATERIALIZED VIEW` and `Mv::ReplaceBody`. Can be suppressed per-step via `intent.toml` (`refresh_after_create = false`). |
| `replace_view` | `target: <qname>`, `mode: or_replace \| drop_create` | Mode is informational; lets golden tests assert on which path was chosen. |

Existing `create`, `drop`, `alter`, `comment` step kinds cover everything else.

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

`intent.toml` schema is unchanged. The new destructive entry shape is:

```toml
[[approve]]
kind   = "drop_view"
target = "app.users_summary"
reason = "Replaced by app.users_summary_v2 — coordinated with billing team."
```

`DROP VIEW` is the only new destructive change kind. `DROP MATERIALIZED VIEW`, `CREATE OR REPLACE VIEW`, drop-and-recreate dependent recreations, comment updates, and reloption flips are all non-destructive.

A new non-destructive override entry shape allows suppressing the auto-emitted post-create refresh on a per-MV basis:

```toml
[[step_override]]
kind     = "refresh_materialized_view"
target   = "app.daily_revenue"
suppress = true
```

This handles the `WITH NO DATA` use case without expanding the destructive surface.

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

- `arb_view_body` — generates random SELECT bodies over a generated table corpus. Used to fuzz canonicalization: parse → shadow load → `pg_get_viewdef` → parse → assert canonical form is idempotent under a second round-trip.
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
- **A view that selects `*` from a table.** PG expands `*` to an explicit column list at parse time; `pg_get_viewdef` reflects the expanded form. Adding a column to the underlying table changes the canonical body and produces a `View::ReplaceBody` diff — which is the correct user-facing behavior (the view's column set just grew).

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

Phasing is left to the implementation plan. The natural ordering is: IR types → source parser → shadow-load pass with dep extraction → catalog readers → differ change kinds → planner step kinds and OR-REPLACE predicate → online-rewrite policy → lints → conformance fixtures → documentation. Each forms a reviewable PR.

## 14. Open questions

None blocking. Two implementation-time decisions that don't need answering now:

- The exact cache-eviction policy for `.pgevolve/cache/shadow/` (LRU on size? TTL? "never evict, user clears manually"?). Defaults to "never evict" until evidence shows it matters.
- Whether to expose a `--no-shadow` flag on `pgevolve diff` that errors when views are present (rather than spawning a container). Can be added later if user feedback demands it.
