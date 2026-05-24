# Pipeline

The path from source SQL to applied DDL, phase by phase. Each phase has
its own implementation crate / module, status, and the design doc /
plan that drove it.

See [`../README.md`](./README.md) for the status legend.

## Phase summary

```mermaid
flowchart TD
    SQL["schema/*.sql"] -- parse --> SourceIR["Source IR"]
    DB[("live Postgres")] -- introspect --> CatalogIR["Catalog IR"]
    SourceIR -- canonicalize --> Source["Catalog (source)"]
    CatalogIR --> Target["Catalog (target)"]
    Source --> Diff{{diff}}
    Target --> Diff
    Diff --> CS["ChangeSet"]
    CS -- order --> OCS["OrderedChangeSet"]
    OCS -- rewrite --> Steps["Vec&lt;RawStep&gt;"]
    Steps -- group_steps --> Groups["Vec&lt;TransactionGroup&gt;"]
    Groups -- "Plan::from_grouped" --> Plan["Plan"]
    Plan --> PlanSql["plan.sql"]
    Plan --> Intent["intent.toml"]
    Plan --> Manifest["manifest.toml"]
    Plan -- "apply()" --> DB
```

## Parsing (`pgevolve_core::parse`)

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/parse/mod.rs::tests`, `parse/statement.rs::tests`, `parse/directives.rs::tests`; tier-2: `crates/pgevolve-core/tests/parser_corpus.rs`, `parse_directory.rs`.

| Aspect | Status | Notes |
|---|---|---|
| `pg_query`-based statement classification | ✅ Implemented | Postgres's own parser, exposed via the `pg_query` crate. |
| Whitelist of source-side DDL kinds | ✅ Implemented | `CREATE SCHEMA/TABLE/INDEX/SEQUENCE`, `ALTER TABLE` (limited to FK whitelist), `COMMENT ON`. Everything else rejects with `UnsupportedObjectKind`. |
| Per-file `-- @pgevolve schema=<name>` directive | ✅ Implemented | Allows unqualified objects in a file to default to a named schema. |
| Per-object source location tracking | ✅ Implemented | Powers lint findings and round-trip cross-checks. |
| `parse_directory` and `parse_directory_with_locations` | ✅ Implemented | The first returns a `Catalog`; the second adds the `qname → SourceLocation` map for the linter. |
| Deterministic file order | ✅ Implemented | Walks paths in sort order so identical inputs produce identical output.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/determinism.rs` |
| Multi-file project layout enforced by the layout profile | ✅ Implemented | See [`lint-and-layout.md`](./lint-and-layout.md). |

## AST canonicalization pass (`pgevolve_core::parse::ast_canon`)

Runs immediately after source parse, before the AST resolution pass. For each view and materialized view in the provisional catalog, the pass:

1. Calls `NormalizedBody::from_sql` (see `parse/normalize_body.rs`) on `raw_body` to fill `body_canonical`.
2. Walks the body AST to extract `DepEdge` records with `DepSource::AstExtracted` provenance, filling `body_dependencies`.
3. Resolves each referenced relation against the provisional catalog. Unresolved references surface as `AstCanonError::UnresolvedReference`.
4. Fills `columns` from the SELECT target list when no explicit alias list was provided (using Postgres's column-naming algorithm: explicit alias → rightmost `ColumnRef` name → `"?column?"` fallback).

The same `NormalizedBody::from_sql` call is used on the catalog side (T5 reader queries `pg_get_viewdef`), so source-side and catalog-side canonical texts are directly comparable by the differ. The v0.2 property test (`view_canonicalization_closed_under_pg_rewrite`) verifies this closure invariant.

| Aspect | Status | Notes |
|---|---|---|
| `NormalizedBody::from_sql` canonicalization | ✅ Implemented | `pg_query` parse + deparse + redundant-qualifier strip (`SELECT users.id FROM app.users` → `SELECT id FROM app.users` for single-relation `FROM` clauses, so PG14's qualified `pg_get_viewdef` output matches PG17's unqualified form) + whitespace collapse. Source: `parse/normalize_body.rs`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/normalize_body.rs::tests`; tier-2: `crates/pgevolve-core/tests/normalize_body.rs`, `ast_canon.rs` |
| Dep-edge extraction from view body AST | ✅ Implemented | `DepEdge { from: NodeId::View, to: NodeId::Table/View/Mv, source: AstExtracted }`.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/ast_canon.rs`, `dep_edges.rs` |
| `body_dependencies` integration into planner ordering | ✅ Implemented | View → table and view → view edges are part of the dep-graph; creates and drops respect the topo order.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/ordering.rs::tests`, `plan/graph.rs::tests`; tier-C: `scenarios/dependency-chains/linear-3-layer-create`, `scenarios/view-uses-function` |

## AST resolution pass (`pgevolve_core::parse::resolve`)

Runs after the AST canonicalization pass. Validates structural references before diff.

| Aspect | Status | Notes |
|---|---|---|
| FK targets validated against declared tables | ✅ Implemented | Surfaces unresolved references as `ParseError::AstResolution` with source location.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/ast_resolution.rs`; tier-C: `failure/ast-resolution/fk-to-missing-table` |
| Default-using sequences validated against declared sequences | ✅ Implemented | Same error path.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/ast_resolution.rs` |
| View body cross-references validated against declared objects | ✅ Implemented | The AST canon pass (`ast_canon.rs`) surfaces unresolved view body references as `AstCanonError::UnresolvedReference`.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/ast_canon.rs` |

## Catalog reader (`pgevolve_core::catalog`)

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/catalog/mod.rs::tests`, `catalog/filter.rs::tests`, `catalog/version.rs::tests`, `catalog/rows.rs::tests`; tier-3: `crates/pgevolve-core/tests/catalog_round_trip.rs`, `functions_round_trip.rs`, `types_round_trip.rs`, `dump_round_trip.rs`.

| Aspect | Status | Notes |
|---|---|---|
| Version detection (`pg_control_system`, server_version_num) | ✅ Implemented | PG 14/15/16/17 tested per major. |
| Per-version SQL strings for each catalog query | ✅ Implemented | |
| Schemas, tables, columns, constraints, indexes, sequences | ✅ Implemented | Mirrors the v0.1 IR surface. |
| Dependencies (sequence `OWNED BY`, default → sequence) | ✅ Implemented | |
| `pg_catalog.default` collation normalized to "none" | ✅ Implemented | Avoids phantom drift on every text column. |
| Sequence / function / collation PG defaults normalized to `None` | ✅ Implemented | PG stores explicit values for things the user didn't declare (sequence min/max, function `procost=100`/`prorows=1000`, implicit `pg_catalog.default` collation on text columns). The catalog reader returns raw values; `ir::canon::filter_pg_defaults` strips them on both the source-built and catalog-read `Catalog`. One place for "next time PG returns a surprising default."<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/canon/filter_pg_defaults.rs::tests` |
| Views and materialized views | ✅ Implemented | `read_views` and `read_materialized_views` query `pg_views` / `pg_matviews`, call `pg_get_viewdef` for the body text, and feed it through `NormalizedBody::from_sql` so the catalog-side canonical text is directly comparable with the source-side canonical text. |
| Functions, procedures | ✅ Implemented | `pg_proc` joined with `pg_language`, `pg_type`, `pg_namespace`; body reconstructed via `pg_get_functiondef`. |
| User-defined types (enums, domains, composites) | ✅ Implemented | `pg_type` joined with `pg_enum`, `pg_attribute`, `pg_constraint`, `pg_attrdef`. |
| Extensions | ✅ Implemented | `pg_extension` joined with `pg_namespace` and `pg_description`. Extension-owned objects excluded via `pg_depend deptype='e'` filter. |
| Triggers | ✅ Implemented | `pg_trigger` joined with `pg_class`, `pg_namespace`, `pg_description`. Filtered: `NOT tgisinternal`; NOT extension-owned. |
| Partitioning (parents + children) | ✅ Implemented | `pg_class.relkind='p'` + `pg_get_partkeydef` for partitioned parents; `relispartition=true` + `pg_get_expr(relpartbound)` for partition children. Both queries: NOT extension-owned; scoped to managed schemas. |
| Cluster surface: roles, role attributes, role membership (`pg_authid`, `pg_auth_members`) | ✅ Implemented | Returned in `ClusterCatalog`, queried via the `pgevolve cluster …` subcommand family. v0.3.0. |
| Per-object owner + grants (object-level + column-level) + `ALTER DEFAULT PRIVILEGES` rules | ✅ Implemented | `pg_class.relowner`, `pg_class.relacl`, `pg_attribute.attacl`, `pg_default_acl` decoded into `owner` / `grants` / `Catalog::default_privileges`. v0.3.1. |
| Row-level security: per-table `rls_enabled` / `rls_forced` + `pg_policies` | ✅ Implemented | `pg_class.relrowsecurity` / `relforcerowsecurity` + a join on `pg_policies` for embedded `Vec<Policy>`. v0.3.2. |
| Storage parameters / reloptions (`pg_class.reloptions::text[]`) | ✅ Implemented | Decoded into typed `TableStorageOptions` / `IndexStorageOptions` with `extra: BTreeMap` for unknown keys. Materialized views share the table decoder. v0.3.3. |
| `CREATE STATISTICS`, publications, subscriptions, FDWs | 📋 Planned / 🔮 Future | See `docs/spec/objects.md`. |
| Catalog filtering by `[managed]` schemas + `[managed].ignore_objects` globs | ✅ Implemented | Unmanaged schemas don't appear in the IR at all. |
| Catalog drift detection — returns `(Catalog, DriftReport)` | ✅ Implemented | See "Catalog drift detection" section below.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/catalog_drift.rs` |

## Catalog drift detection

The catalog reader returns a `DriftReport` alongside the `Catalog`. The differ and planner consume it to automatically recover from partial-apply states.

| Drift kind | Detection | Diff emit | Planner emit | Status | Tests |
|---|---|---|---|---|---|
| NOT VALID constraints (`pg_constraint.convalidated = false`) | Catalog reader | `Change::ValidateConstraint` | `ALTER TABLE ... VALIDATE CONSTRAINT` | ✅ Implemented | tier-2: `crates/pgevolve-core/tests/catalog_drift.rs` |
| INVALID indexes (`pg_index.indisvalid = false`) | Catalog reader | `Change::RecreateIndex` | `DROP INDEX + CREATE INDEX` | ✅ Implemented | tier-2: `crates/pgevolve-core/tests/catalog_drift.rs` |

Both drift kinds are auto-recovery paths — the user doesn't author NOT VALID or INVALID states; pgevolve detects and resolves them. This covers recovery from crashed FK NOT VALID + VALIDATE rewrites and failed `CREATE INDEX CONCURRENTLY`.

## Differ (`pgevolve_core::diff`)

**Tests (whole section):** tier-1: every `crates/pgevolve-core/src/diff/*.rs::tests` module (tables, columns, constraints, indexes, sequences, views, triggers, types, routines, policies, grants, reloptions, owner_op, schemas, extensions, cluster, default_privileges); tier-1: `crates/pgevolve-core/src/diff/changeset.rs::tests`, `change.rs::tests`, `destructiveness.rs::tests`.

| Aspect | Status | Notes |
|---|---|---|
| `Catalog::diff` produces a structured `Vec<Difference>` for assertions | ✅ Implemented | **Tests:** tier-1: `crates/pgevolve-core/src/diff/mod.rs::tests`, `ir/difference.rs` |
| `diff(target, source) → ChangeSet` produces the unordered planner input | ✅ Implemented | Each `ChangeEntry` carries a `Destructiveness` classification.<br>**Tests:** tier-1: `crates/pgevolve-core/src/diff/changeset.rs::tests` |
| Pair-by-qname semantics | ✅ Implemented | Tables / indexes / sequences pair by qualified name; columns / constraints pair by bare name within their parent table.<br>**Tests:** tier-1: `crates/pgevolve-core/src/diff/tables.rs::tests`, `columns.rs::tests` |
| Column reorder detection | 🟡 Partial | Detected as `columns.<order>` difference but the planner does not yet emit a reorder step (Postgres has no `ALTER COLUMN ... POSITION`, so this would require table rewrite).<br>**Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/column_position_drift.rs::tests`; tier-2: `crates/pgevolve-core/tests/lint_position_drift.rs` |
| Index option change detection | ✅ Implemented | Triggers `ReplaceIndex` (drop + create).<br>**Tests:** tier-1: `crates/pgevolve-core/src/diff/indexes.rs::tests` |
| Constraint rename detection | ⛔ Not planned | Diffs as drop+add. |
| Destructiveness classification | ✅ Implemented | Three levels: `Safe`, `RequiresApproval`, `RequiresApprovalAndDataLossWarning`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/diff/destructiveness.rs::tests` |

## Planner (`pgevolve_core::plan`)

### Ordering

**Tests (whole subsection):** tier-1: `crates/pgevolve-core/src/plan/ordering.rs::tests`, `plan/ordered.rs::tests`, `plan/graph.rs::tests`, `plan/edges.rs::tests`; tier-2: `crates/pgevolve-core/tests/dep_edges.rs`, `determinism.rs`; tier-5: property `plan_id_is_deterministic`, `create_graph_topo_sorts_or_only_fk_cycles` (`crates/pgevolve-core/tests/property_tests.rs`).

| Aspect | Status | Notes |
|---|---|---|
| Three-phase ordering: creates → modifies → drops | ✅ Implemented | Each phase topologically sorted by the appropriate graph. |
| Source-side dependency graph for creates / modifies | ✅ Implemented | Schema ← table, table ← index, FK ← both endpoints, sequence ← owning table, table ← default-using sequence. |
| Target-side dependency graph for drops | ✅ Implemented | Same edges; drop order is the reverse topo sort. |
| FK forward-reference cycle handling | ✅ Implemented | Cycles are broken by extracting offending FKs into a post-pass `DeferredFkAdd` list.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/emit/deferred_fk.rs`; tier-C: `failure/cycle` |
| Deterministic tie-break | ✅ Implemented | Topological sort uses a `BTreeSet`/min-heap by `Ord`; identical inputs produce byte-identical plans.<br>**Tests:** tier-2: `crates/pgevolve-core/tests/determinism.rs` |

### Rewrites

**Tests (whole subsection):** tier-1: `crates/pgevolve-core/src/plan/rewrite/tests.rs` (covers concurrent index, FK NOT VALID, CHECK NOT VALID, set-not-null pattern, atomic-vs-online gating).

| Rule | Status | Notes |
|---|---|---|
| Concurrent index create (`CREATE INDEX CONCURRENTLY`) on existing tables | ✅ Implemented | Non-unique only. Excluded for new tables, unique indexes, and atomic policy.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/tests::create_index_on_existing_table_rewrites_to_concurrent` |
| Concurrent index drop (`DROP INDEX CONCURRENTLY`) on existing non-unique indexes | ✅ Implemented | Same gating.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/concurrent_index.rs` |
| FK `NOT VALID` + `VALIDATE CONSTRAINT` for adds on existing tables | ✅ Implemented | Splits across two transaction groups so step A (cheap) commits before step B (table scan).<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/fk_not_valid_validate.rs` |
| CHECK `NOT VALID` + `VALIDATE CONSTRAINT` for adds on existing tables | ✅ Implemented | Same pattern as FK.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/check_not_valid_validate.rs` |
| `SET NOT NULL` on populated columns via the CHECK pattern (4 steps) | ✅ Implemented | `ADD CHECK NOT VALID` → `VALIDATE` → `SET NOT NULL` (cheap once validated) → `DROP CONSTRAINT`.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/set_not_null_check_pattern.rs`, `plan/rewrite/tests::set_not_null_on_existing_column_emits_four_steps` |
| Per-environment policy override (`[environments.<env>].strategy = "atomic"`) | ✅ Implemented | Atomic mode disables every online rewrite.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/policy.rs::tests`, `plan/rewrite/tests::atomic_policy_disables_concurrent_index_rewrite` |
| `REFRESH MATERIALIZED VIEW CONCURRENTLY` upgrade | ✅ Implemented | When `refresh_mv_concurrently = true` (default) and the MV has a unique index, the planner emits `REFRESH MATERIALIZED VIEW CONCURRENTLY` instead of the locking variant. Gated on strategy = online.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/refresh_mv_concurrently.rs::tests`; tier-C: `objects/materialized_views/refresh-concurrently` |
| Dependent-view recreation cascade (`recreate_views::extend_with_dependent_recreations`) | ✅ Implemented | When a table drop, column change, or incompatible view-body replace is detected, the planner walks `body_dependencies` transitively and emits explicit `DROP + CREATE` steps for every affected view. Controlled by `view_drop_create_dependents` switch (default `true`).<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/recreate_views.rs::tests`, `plan/rewrite/views.rs::tests`; tier-C: `scenarios/dependency-chains/view-on-view-column-drop` |
| `ALTER TYPE ... ADD VALUE` (enum value add) online rewrite | 📋 Planned, v0.2 | Lands with enum support.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/types.rs::tests`; tier-C: `objects/enums/add-value-at-end`, `add-value-before-existing` |
| `ALTER COLUMN ... TYPE` online rewrite (e.g., int → bigint) | 🔮 Future | Currently emits a single `ALTER COLUMN ... TYPE` step, which can rewrite the entire table. The "USING expr + new column + rename" pattern is a candidate v0.3 rewrite.<br>**Tests:** tier-C: `objects/columns/alter-column-type-widening`, `alter-column-type-narrowing` |
| `REINDEX CONCURRENTLY` for bloated indexes | 🔮 Future | Not currently emitted by the planner; users invoke manually. |

### Plan-time lint gate

| Aspect | Status | Notes |
|---|---|---|
| `run_drift_lints` called after diff, before writing plan | ✅ Implemented | Any `LintAtPlan` finding without a matching `[[lint_waiver]]` in `intent.toml` causes `pgevolve plan` to exit with code 2.<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_waiver_e2e.rs::plan_refuses_unwaived_column_position_drift`, `plan_proceeds_with_matching_lint_waiver`; tier-C: `failure/lint-at-plan/column-position-drift-no-waiver` |
| `column-position-drift` as a `LintAtPlan` finding | ✅ Implemented | See [`lint-and-layout.md`](./lint-and-layout.md).<br>**Tests:** tier-2: `crates/pgevolve-core/tests/lint_position_drift.rs` |

### Step grouping

**Tests (whole subsection):** tier-1: `crates/pgevolve-core/src/plan/grouping.rs::tests`.

| Aspect | Status | Notes |
|---|---|---|
| Adjacent steps with the same `TransactionConstraint` coalesce into one group | ✅ Implemented | |
| Transactional groups run inside one `BEGIN; … COMMIT;` | ✅ Implemented | |
| Non-transactional groups run as autocommit singletons | ✅ Implemented | Each `CONCURRENTLY` step is its own atomic unit. |

## Plan format (`pgevolve_core::plan::{serialize, deserialize}`)

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/plan/serialize.rs::tests`, `plan/deserialize.rs::tests`, `plan/plan.rs::tests`.

| File | Status | Notes |
|---|---|---|
| `plan.sql` | ✅ Implemented | Canonical artifact: directive header + per-group BEGIN/COMMIT + per-step `-- @pgevolve step=…` directive lines + SQL bodies. Runs cleanly under `psql -f` even without pgevolve's executor. |
| `intent.toml` | ✅ Implemented | One `[[intent]]` row per destructive step; user must flip `approved = true` before applying. |
| `manifest.toml` | ✅ Implemented | Plan id (full hex), version metadata, target identity, embedded pre-image catalog as JSON. |
| Round-trip property: `read_plan_dir(write_plan_dir(p)) == p` | ✅ Implemented | Property-tested.<br>**Tests:** tier-5: property in `crates/pgevolve-core/tests/property_tests.rs` |
| Cross-file plan-id mismatch detection | ✅ Implemented | All three files must agree on `plan_id`. |
| Deterministic `PlanId` (BLAKE3 over bincode-encoded `(source, target, version, ruleset)`) | ✅ Implemented | Identical inputs always produce the same id.<br>**Tests:** tier-5: `plan_id_is_deterministic` (`crates/pgevolve-core/tests/property_tests.rs`) |

## Executor (`pgevolve::executor`)

**Tests (whole section):** tier-4: `crates/pgevolve/tests/executor_smoke.rs`, `chaos_apply.rs`; tier-5: `drift_recovery_property` in `crates/pgevolve/tests/pg_property_tests.rs`.

| Stage | Status | Notes |
|---|---|---|
| Bootstrap `pgevolve.bootstrap_version` / `apply_log` / `plan_steps` / `lock` tables | ✅ Implemented | Idempotent; append-only migration list.<br>**Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::bootstrap_is_idempotent` |
| Singleton advisory lock (`pg_try_advisory_lock`) | ✅ Implemented | Lock key derived from ASCII `PGEVOLVE`. Session-scoped; released on disconnect or via `release_lock`.<br>**Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::advisory_lock_contention` |
| Target-identity computation (BLAKE3 of `(db, host, port, cluster_name, system_identifier)`) | ✅ Implemented | **Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::target_identity_is_stable_across_reconnects`, `target_identity_differs_between_distinct_databases` |
| Preflight: identity match | ✅ Implemented | Bypassed only with `--allow-different-target`.<br>**Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::apply_rejects_target_identity_mismatch` |
| Preflight: drift recheck | 🟡 Partial | The plan slot exists but the executor's drift check is stubbed; the CLI's `apply` currently forces `allow_drift = true`. Phase-9 follow-up. <!-- TODO: no test located 2026-05-24 --> |
| Preflight: intent approval enforcement | 🟡 Partial | The plan's `intents` field is loaded but the executor doesn't re-check `approved = true` from disk. Phase-9 follow-up. <!-- TODO: no test located 2026-05-24 --> |
| Preflight: `[[lint_waiver]]` structural validation | ✅ Implemented | Preflight validates that every `[[lint_waiver]]` row has non-empty `rule` and `target`. Documented limitation: does not re-run drift lints at apply time (source not available); the live-catalog recheck stub will land in a future task.<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_waiver_e2e.rs::lint_waiver_survives_intent_toml_round_trip` |
| Audit row writes (`open_apply_log`, `mark_step_*`, `close_apply_log`) | ✅ Implemented | **Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::apply_succeeds_end_to_end_and_persists_audit_rows`, `status_queries_return_recent_apply_with_steps` |
| Transactional group execution (single `BEGIN…COMMIT`) | ✅ Implemented | A step failure rolls back the group; every step in the group ends up `failed` (the offender) or `rolled_back` (the rest).<br>**Tests:** tier-4: `crates/pgevolve/tests/executor_smoke.rs::apply_rolls_back_transactional_group_on_failure` |
| Autocommit group execution | ✅ Implemented | Stops on first failure; earlier steps stay `succeeded`.<br>**Tests:** tier-4: `crates/pgevolve/tests/chaos_apply.rs` |
| `abort_after_step` testkit hook (chaos harness) | ✅ Implemented | Cleanly aborts after a named step; the apply_log row goes to `aborted`.<br>**Tests:** tier-4: `crates/pgevolve/tests/chaos_apply.rs` |
| Real `SIGKILL`-mid-apply chaos | 🔮 Future | The clean-abort path covers recovery semantics; literal SIGKILL is more invasive and reserved for v0.2's chaos coverage. |

## Shadow validation (`pgevolve validate --shadow`)

**Tests (whole section):** tier-4: `crates/pgevolve/tests/shadow_validate.rs`, `shadow_validate_flag.rs`, `shadow_validate_views.rs`, `shadow_backend.rs`.

| Aspect | Status | Notes |
|---|---|---|
| Ephemeral Postgres per configured major version | ✅ Implemented | testcontainers-backed; the IR is applied via the same planner + executor pipeline.<br>**Tests:** tier-4: `crates/pgevolve/tests/shadow_validate.rs::shadow_round_trip_succeeds_on_clean_source` |
| Round-trip introspection + diff | ✅ Implemented | Mismatches are reported as line-by-line `Finding`s on stderr.<br>**Tests:** tier-4: `crates/pgevolve/tests/shadow_validate_views.rs`, `shadow_validate_flag.rs` |
| `--shadow` without Docker | ✅ Implemented | Exits with a clear error rather than crashing inside testcontainers.<br>**Tests:** tier-4: `crates/pgevolve/tests/shadow_validate.rs::shadow_without_section_errors_cleanly` |
