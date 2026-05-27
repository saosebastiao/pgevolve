# Changelog

All notable changes to pgevolve are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.5] — 2026-05-26

### Added

- **SUBSCRIPTION as a first-class IR object.** Per-field lenient
  `SubscriptionOptions` (enabled, binary, streaming Off/On/Parallel,
  two_phase, disable_on_error PG15+, password_required + run_as_owner
  + origin PG16+, failover PG17+). Opaque CONNECTION string with
  `${VAR}` env-var interpolation.
- **Apply-time `${VAR}` resolution.** Source IR and plan.sql store
  unresolved `${VAR}` placeholders. Preflight scans every step's SQL,
  resolves against process env, fails before any DB connection if a
  reference is unset. Secrets never persist to disk.
- **8 new StepKind variants** for subscription operations.
- **4 lint rules**: `unmanaged-subscription` (Warning),
  `subscription-references-undeclared-publication` (Warning),
  `subscription-feature-requires-pg-version` (Error, not waivable),
  `subscription-password-in-source` (Error, not waivable) —
  catches plaintext `password=` at parse time.
- **`[fixture] apply` flag** in the conformance harness so fixtures
  with cross-cluster side-effects (subscriptions) can validate
  parse/diff/plan/lint without applying.
- **12 conformance fixtures** under `objects/subscriptions/`.

### Closes

Second item from the post-v0.3.3 agreed roadmap (next:
CREATE VIEW WITH CHECK OPTION).

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

## [0.3.3] — 2026-05-23

### Added

- **Storage parameters / reloptions on tables, indexes, materialized views.** Typed `Option<T>` fields for the well-known keys (fillfactor, autovacuum_*, parallel_workers, fastupdate, buffering, pages_per_range, etc.) plus `extra: BTreeMap<String, String>` for extension-registered or unknown keys. Tables and MVs share the autovacuum substruct since PG documents identical key sets.
- **Per-AM fillfactor validation** at parse time: B-tree 50..=100, GiST 10..=100, SP-GiST 90..=100, BRIN/GIN reject fillfactor.
- **Lenient drift policy**: source `None` always means "unmanaged" — never triggers `RESET`. `unmanaged-reloption` lint surfaces catalog reloptions not in source.
- **3 new StepKind variants**: `SetTableStorage`, `SetIndexStorage`, `SetMaterializedViewStorage`. One ALTER step per relkind per diff (batches multiple keys into one SET).
- **`unmanaged-reloption` lint** (warning, waivable).
- **Source parser** for `WITH (...)` on CREATE TABLE/INDEX/MATERIALIZED VIEW and `ALTER ... SET (...)`. `RESET (...)` rejected in source.
- **Catalog reader** decodes `pg_class.reloptions::text[]` into typed structs.
- **11 conformance fixtures.**

### Known limitations

- `CREATE TABLE/INDEX/MATERIALIZED VIEW … WITH (…)` against a brand-new object emits the CREATE without the inline `WITH`. Convergent on the next plan run via `ALTER … SET`. Same gap as owner/grants/policies on new objects in v0.3.x; will be closed uniformly in a follow-up.

### Closes

Slipped v0.2 commitment from `docs/spec/objects.md` (table reloptions row marked 🟡 Partial). Per-partition storage parameters also satisfied (partitions inherit since they're `Table` in IR).

## [0.3.2] — 2026-05-22

### Added

- **Row-level security policies** — `Table` gains `rls_enabled`, `rls_forced`, and `policies: Vec<Policy>`. Policies carry `permissive`, `command`, `roles`, `using`, `with_check`. USING / WITH CHECK reuse `NormalizedExpr` canonicalization shared with check constraints.
- **Source parser:** `CREATE POLICY` + four `ALTER TABLE ... { ENABLE | DISABLE | FORCE | NO FORCE } ROW LEVEL SECURITY` subcommands. `ALTER POLICY` and `DROP POLICY` rejected in source (diff-driven).
- **Differ:** 5 new Change variants (`CreatePolicy`, `DropPolicy`, `AlterPolicy`, `SetTableRowSecurity`, `SetTableForceRowSecurity`). Command-kind changes recreate (DROP + CREATE) because PG doesn't allow `ALTER POLICY` to change the command.
- **Catalog reader:** new `pg_policies` query + `relrowsecurity` / `relforcerowsecurity` on the tables query.
- **Two lint additions:**
  - `grant-references-unknown-role` (existing) now also walks policy `TO` clauses.
  - `force-rls-without-policies` (new, Warning) — fires when a table has FORCE RLS enabled but no policies defined (PG would deny all rows).
- **Conformance:** 11 new fixtures under `objects/policies/`.

### Closes

v0.3 security/permissions trilogy: roles (v0.3.0) → grants (v0.3.1) → policies (v0.3.2).

## [0.3.1] — 2026-05-22

### Added

- **Object grants + ownership** — all 8 grantable IR types (Schema, Sequence, Table, View, MaterializedView, Function, Procedure, UserType) gain `owner: Option<Identifier>` + `grants: Vec<Grant>`. Column-level grants on tables/views/MVs.
- **Default privileges** — `Catalog.default_privileges: Vec<DefaultPrivilegeRule>` mirroring `pg_default_acl`. Supports `FOR ROLE x IN SCHEMA y GRANT/REVOKE ... ON {TABLES, SEQUENCES, FUNCTIONS, TYPES, SCHEMAS}`.
- **Lenient drift policy** — catalog grants to roles outside source surface as `grants-to-unmanaged-role` warning, never silently revoked.
- **Optional cluster-link** — `[cluster].project` in pgevolve.toml validates grantee role names against the linked cluster project's `roles/*.sql` via the `grant-references-unknown-role` lint (Error severity).
- **Three new lint rules**:
  - `grant-references-unknown-role` (Error, cluster-aware) — grantee not in linked cluster source.
  - `grants-to-unmanaged-role` (Warning) — catalog grants to roles not declared in source.
  - `revoke-from-owner` (Error) — REVOKE would target object's owner.
- **Six new StepKind variants:** `AlterObjectOwner`, `GrantObjectPrivilege`, `RevokeObjectPrivilege`, `GrantColumnPrivilege`, `RevokeColumnPrivilege`, `AlterDefaultPrivileges`.

### Catalog reader

- New `catalog::grants` module decodes PG aclitem text format. All 6 family queries gain `<obj>owner` + `<obj>acl::text[]`. Tables/views/MVs also decode `pg_attribute.attacl` for column-level grants. Owner self-grants stripped (PG materializes them when any explicit grant exists; they're implicit by ownership).

### Source parser

- Three new builder modules: object-level `GRANT`, `ALTER ... OWNER TO`, `ALTER DEFAULT PRIVILEGES`. `GRANT ALL` expands per object type. Column-level grants extracted from `AccessPriv.cols`. REVOKE rejected in source.

### Shadow validate

- `validate --shadow` now respects "unmanaged owner" semantics: when source declares `owner = None` (no `OWNER TO`), shadow-validate ignores any catalog-side owner. Similarly for grants — only managed grantees compared.

### Conformance

- 13 new fixtures under `objects/grants/` covering table/schema/function/sequence/owner/default-privs/lint sub-roots. Two cluster-link fixtures deferred (harness extension out of scope for v0.3.1).

## [0.3.0] — 2026-05-22

### Added

- **Cluster-level surface** — new project type (`pgevolve-cluster.toml + roles/`), new command family (`pgevolve cluster init/diff/plan/apply/status`), new executor running against a superuser DSN. Per Decision 23 of the v0.2 architecture review.
- **`ROLE` / `CREATE USER` fully managed** — `ClusterCatalog.roles` with full PG attribute matrix (superuser, createdb, createrole, inherit, login, replication, bypass_rls, connection_limit, valid_until), plus role membership via inline `IN ROLE` or `GRANT role TO target`. Passwords intentionally not modeled.
- **Two new universal lint rules**:
  - `role-loses-superuser` (warning) — fires on `ALTER ROLE … NOSUPERUSER` when the role had superuser.
  - `role-membership-cycle` (error) — detects cycles in the projected post-apply membership graph; pre-empts PG's apply-time rejection.
- **Conformance harness** — new `authoring = "cluster"` mode + seven fixtures under `cases/cluster/roles/`.
- **Property tests** — `arbitrary_role_attributes`, `arbitrary_cluster_catalog` generators with cycle-free membership; diff round-trip invariant: `diff_cluster(A, B)` applied to `A` yields `B` modulo canonicalization.

### Catalog reader

- New `read_cluster_catalog(querier, bootstrap_roles)` querying `pg_authid` + `pg_auth_members`. Filters `pg_*` predefined roles and caller-supplied bootstrap roles.

### Known v0.3.0 gaps (closing in 0.3.x)

- Cluster apply: reads `plan.sql` and runs each statement in a transaction. `intent.toml` destructive-step gates and apply-log tracking are deferred.
- No advisory lock during cluster apply; concurrent applies are not protected.
- Object-level GRANT/REVOKE (per-DB) lands in v0.3.1.
- RLS policies (per-DB) land in v0.3.2.

## [0.2.1] — 2026-05-21

### Added

- **Per-column TOAST storage** — `STORAGE { PLAIN | EXTERNAL | EXTENDED | MAIN }` is now a managed `Column` attribute. Source parser accepts both inline (`col text STORAGE EXTERNAL`, PG 16+ syntax) and `ALTER COLUMN SET STORAGE` forms. Differ emits non-destructive `SET STORAGE` steps; canon strips type-default values so explicit and implicit defaults are equivalent.
- **Per-column TOAST compression** — `COMPRESSION { pglz | lz4 }` is now a managed attribute. `None` preserves the cluster `default_toast_compression` GUC; explicit `pglz` or `lz4` overrides it. `SET COMPRESSION DEFAULT` round-trips through the parser as `None`.
- **Two new lint rules** (surfaced as `Plan.advisory_findings` and printed by `pgevolve plan` to stderr):
  - `storage-downgrade-not-retroactive` — warns when a SET STORAGE change reduces toastability (e.g. `EXTERNAL → MAIN`), since existing TOASTed values aren't rewritten until UPDATE or VACUUM FULL.
  - `compression-change-not-retroactive` — warns on any compression change for the same reason.

### Catalog reader

- `COLUMNS_QUERY` now selects `attstorage` and `attcompression` from `pg_attribute`. No per-version split; both columns are present in PG 14+ (the project MSRV).

### Conformance

- Five new fixtures under `objects/columns/`: `set-storage-external`, `set-storage-plain-warning`, `set-compression-lz4`, `create-table-with-storage`, `set-storage-type-default-noop`.

## [0.2.0] — 2026-05-21

Extends the v0.1 surface with **views, materialized views, user-defined types, functions/procedures, extensions, triggers, and declarative partitioning** as fully-managed objects. The differ, planner, linter, conformance suite, and property tests all cover the new object kinds. Ships alongside a project constitution (`docs/CONSTITUTION.md`), `cargo-deny`-enforced license + advisory policy (`deny.toml`), `CLAUDE.md` agent guidance, and shadow validation for view bodies (T13).

### Added — internal architecture (2026-05-19)

- **`pgevolve-core-macros` crate** — internal proc-macro crate exposing `#[derive(DiffMacro)]`. Most IR structs (`Schema`, `Sequence`, `Column`, `Constraint`, `ForeignKey`, `Procedure`, `Index`) now derive their `Diff` impl with `#[diff(skip)]` / `#[diff(via_debug)]` / `#[diff(nested)]` field attributes. Hand-written impls retained where they have non-trivial logic (`Catalog`, `Function`, `Table`, `View`, `MaterializedView`, `UserType`, and the enum impls). Removes ~250 lines of mechanical boilerplate.
- **`ir::canon` pipeline** — every IR-value normalization rule moved into a single ordered pipeline. Four named passes: `filter_pg_defaults` (sequence min/max, function cost/rows, `pg_catalog.default` collation → `None`); `sentinel_view_columns` (view/MV column types → shared sentinel); `renumber_enum_sort_orders` (enum sort_order → `1.0, 2.0, 3.0, …`); `sort_and_dedupe` (canonical-key sort + duplicate detection). `Catalog::canonicalize` is now a thin wrapper. Catalog reader and source builders are kept "raw" — they no longer filter PG defaults. The rule for the next PG-default surprise lives in one place.
- **`pgevolve::api::build_plan`** — library entry point that runs the full parse→introspect→diff→order→rewrite→group→assemble pipeline and returns a `Plan` value. No `println!`, no waiver-prompt UX, no `--shadow-validate`, no on-disk plan directory.
- **`pgevolve::executor::apply_plan(&Plan, …)`** — sibling to `apply(plan_dir, …)` that takes an in-memory `Plan`. The disk-based `apply` is now a thin shim that calls `read_plan_dir` then delegates. CLI `plan`/`apply` commands are thin wrappers over `api::build_plan` / `executor::apply_plan` plus CLI UX.
- **`Plan::approve_all_intents`** — helper on `pgevolve_core::plan::Plan` for test harnesses building plans programmatically. Production apply still requires explicit `intent.toml` approval.

### Changed — conformance suite (2026-05-19)

- Conformance Layer 4 (apply roundtrip) now runs **in-process** via `pgevolve::api::build_plan` + `pgevolve::executor::apply_plan`. The subprocess scaffolding (`cargo_bin`, `run_pgevolve`, `plan_and_locate`, `patch_intent_toml_approve_all`, `write_project`) is gone — ~150 fewer lines in `crates/pgevolve-conformance/src/assertions/apply.rs`. Faster (no per-fixture binary rebuild + spawn) and easier to debug (assertions can inspect the `Plan` value rather than its on-disk rendering).

### Added — shadow validation for view bodies (T13)

- **`--shadow-validate` now cross-checks view + materialized view bodies** against an ephemeral Postgres. `render_catalog` emits views/MVs (after sequences); `cross_check` queries `pg_get_viewdef` for each declared view/MV, re-parses through the source canonicalizer, and compares against the source IR's body canonical text + body_dependencies (the latter walked via `pg_rewrite → pg_depend → pg_class`). Defaults to warnings; `--shadow-strict` promotes mismatches to errors.
- New `crates/pgevolve-core/src/render/view.rs` (`render_view`, `render_materialized_view`).
- Docker-gated integration tests in `crates/pgevolve/tests/shadow_validate_views.rs`.
- Closes the deferred T13 plan from sub-spec #1 (views/MVs).

### Added — IR (functions and procedures)

- `Function { qname, args, arg_types_normalized, return_type, language, body, body_dependencies, volatility, strict, security, parallel, leakproof, cost, rows, comment }` flat IR type in `pgevolve-core::ir::function`.
- `Procedure { qname, args, language, body, body_dependencies, security, commits_in_body, comment }` flat IR type in `pgevolve-core::ir::procedure`.
- `FunctionArg { name, mode: ArgMode, ty, default }` — argument declaration with IN/OUT/INOUT/VARIADIC modes.
- `NormalizedArgTypes { types, canonical_hash }` — BLAKE3 hash over comma-joined IN/INOUT/VARIADIC type strings; the function identity disambiguator for overloads.
- `ReturnType` — `Scalar`, `SetOf`, `Table { columns }`, `Trigger`, `EventTrigger`, `Void`.
- `FunctionLanguage` — `Sql` | `PlPgSql`.
- `Catalog::functions: Vec<Function>` and `Catalog::procedures: Vec<Procedure>` — flat collections, sorted by `(qname, arg_types_normalized)` / `qname` after `canonicalize()`.

### Added — pipeline (functions and procedures)

- **Source parser** — `CREATE FUNCTION` and `CREATE PROCEDURE` parse into the `Function` / `Procedure` IR. Full attribute matrix (volatility, strict, security, parallel, leakproof, cost, rows). Dollar-quote body extraction for both SQL and PL/pgSQL languages.
- **PL/pgSQL body parser** (`parse/builder/plpgsql.rs`) — wraps the body in a synthetic `CREATE FUNCTION` and calls `pg_query::parse_plpgsql`. Extracts static embedded SQL dep edges (`PLpgSQL_stmt_execsql`), detects `COMMIT`/`ROLLBACK` nodes for `commits_in_body`, and scans `-- @pgevolve dep:` directives for dynamic SQL.
- **AST resolution** — validates routine body dep edges against the catalog; unresolved managed-schema references surface as warnings.
- **Catalog reader** — queries `pg_proc` (with `pg_language`, `pg_type`, `pg_namespace`) to reconstruct `Function` and `Procedure` from a live database. Handles multi-arg functions, OUT args, overloads, and all attribute columns.
- **Differ** — `FunctionChange` variants: `Create`, `Drop`, `OrReplace`, `ReplaceWithCascade`, `CommentOn`. `ProcedureChange` variants: `Create`, `Drop`, `OrReplace`, `CommentOn`.
- **OR-REPLACE compatibility predicate** (`function_can_or_replace`) — returns `true` when language, return type, and OUT/INOUT parameters are all unchanged; falls back to `ReplaceWithCascade` otherwise.
- **Planner** — 6 new step kinds: `CreateOrReplaceFunction`, `DropFunction`, `CommentOnFunction`, `CreateOrReplaceProcedure`, `DropProcedure`, `CommentOnProcedure`. Procedures with `commits_in_body = true` are placed in non-transactional steps.
- **`NodeId::Function` / `NodeId::Procedure`** — added to the dep graph; body dep edges drive correct creation/drop ordering relative to their referenced tables, views, and types.

### Added — lint rules (functions and procedures)

- `plpgsql-dynamic-sql` (Error) — PL/pgSQL body uses `EXECUTE` without a `-- @pgevolve dep:` directive.
- `procedure-contains-commit` (Warning) — procedure body contains `COMMIT` or `ROLLBACK`; runs with `transactional=OutsideTransaction`.
- `function-references-unmanaged-schema` (Warning) — routine body dep edge targets an unmanaged schema.

### Added — tests (functions and procedures)

- **~22 conformance fixtures** (Tier C): `objects/functions/` and `objects/procedures/` covering SQL functions, PL/pgSQL functions, procedures, overloads, dep-edge extraction, `ReplaceWithCascade`, and all three lint rules.
- **Property test** `plpgsql_canonicalization_is_idempotent` (`#[ignore]`, pure, no Docker) — for each body in the representative `PLPGSQL_BODIES` corpus, `parse_routine_body → canonical_text → re-parse → canonical_text` produces byte-identical output. Closes the round-trip invariant the differ relies on.

### Added — IR (triggers)

- `Trigger { qname, table_name, function_name, timing: TriggerTiming, events: Vec<TriggerEvent>, for_each: ForEach, when_clause: Option<NormalizedExpr>, update_columns: Vec<Identifier>, referencing: Option<TransitionTables>, constraint: bool, deferrable: bool, initially_deferred: bool, comment: Option<String> }` flat IR type in `pgevolve-core::ir::trigger`.
- `TriggerTiming` — `Before` | `After` | `InsteadOf`.
- `TriggerEvent` — `Insert` | `Update` | `Delete` | `Truncate`.
- `ForEach` — `Row` | `Statement`.
- `TransitionTables { old_table: Option<Identifier>, new_table: Option<Identifier> }` — REFERENCING clause for transition tables.
- `Catalog::triggers: Vec<Trigger>` flat collection, sorted by `(table_name, qname)` after `canonicalize()`.

### Added — pipeline (triggers)

- **Source parser** — `CREATE [CONSTRAINT] TRIGGER` parses into the `Trigger` IR. BEFORE/AFTER/INSTEAD OF timing, INSERT/UPDATE/DELETE/TRUNCATE events, FOR EACH ROW/STATEMENT, WHEN clause (as `NormalizedExpr`), UPDATE OF column list, and REFERENCING transition tables all modeled. `ALTER TRIGGER` in source files rejected at statement classification.
- **`COMMENT ON TRIGGER` parser arm** — recognized alongside `COMMENT ON FUNCTION` and `COMMENT ON EXTENSION` in the comment-statement path.
- **Catalog reader** — queries `pg_trigger` joined with `pg_class`, `pg_namespace`, and `pg_description`. Filters: `NOT tgisinternal` (system-generated triggers excluded); `NOT EXISTS (pg_depend deptype='e')` (extension-owned triggers excluded). Reconstructs all modeled fields including constraint, deferrable, and initially-deferred flags.
- **Differ** — `TriggerChange` variants: `Create`, `Drop`, `CommentOn`. Any structural difference (timing, events, for-each, when-clause, function, columns, transition tables, constraint flags) emits `Drop` + `Create` — there is no `ALTER TRIGGER` for body-level changes. `CommentOn` is comment-only.
- **Planner** — 3 new step kinds: `CreateTrigger`, `DropTrigger` (destructive; intent required), `CommentOnTrigger`. `DropTrigger` is placed in the same destructive bucket as `DropTable` and `DropFunction`.
- **`NodeId::Trigger`** — added to the dep graph; `Trigger → Table/View/MV` edges ensure the target relation exists before the trigger is created; `Trigger → Function` edges ensure the trigger function exists before the trigger fires.

### Added — lint rules (triggers)

- `trigger-references-unmanaged-table` (Warning) — trigger's target relation is not in any managed schema.
- `trigger-references-unmanaged-function` (Warning) — trigger function is not in any managed schema.

### Added — tests (triggers)

- **Conformance fixtures** (Tier C): `objects/triggers/` covering create/drop/comment, BEFORE/AFTER/INSTEAD OF variants, ROW vs STATEMENT, WHEN clause, UPDATE OF columns, REFERENCING transition tables, CONSTRAINT TRIGGER with DEFERRABLE, and both lint rules.

### Added — IR (partitioning)

- `partition_by: Option<PartitionBy>` and `partition_of: Option<PartitionOf>` fields added to `Table` in `pgevolve-core::ir::table`.
- New `ir/partition.rs` module:
  - `PartitionBy { strategy: PartitionStrategy, columns: Vec<PartitionColumn> }` — the `PARTITION BY` clause on a partitioned parent.
  - `PartitionStrategy` — `Range` | `List` | `Hash`.
  - `PartitionColumn { kind: PartitionColumnKind, collation: Option<QualifiedName>, opclass: Option<QualifiedName> }` — a single partition key element with optional collation and opclass overrides.
  - `PartitionColumnKind` — `Column(Identifier)` | `Expr(NormalizedExpr)`.
  - `PartitionOf { parent: QualifiedName, bounds: PartitionBounds }` — the `PARTITION OF parent FOR VALUES …` clause on a partition child.
  - `PartitionBounds` — `Range { from, to }` | `List { values }` | `Hash { modulus, remainder }` | `Default`.
  - `BoundDatum` — `Literal(NormalizedExpr)` | `MinValue` | `MaxValue`.

### Added — pipeline (partitioning)

- **Source Form 1** — `CREATE TABLE child PARTITION OF parent FOR VALUES …` parsed directly into `Table { partition_of: Some(…) }`. The parent's key is inferred from its `partition_by`.
- **Source Form 2** — standalone `CREATE TABLE child PARTITION OF parent FOR VALUES …` in a separate file. Identical IR as Form 1.
- **Source Form 3** — `ALTER TABLE parent ATTACH PARTITION child FOR VALUES …` combined with a standalone child `CREATE TABLE` (no inline `PARTITION OF`). The parser merges the attach statement into the child's `partition_of`, producing the same IR as Form 2. Equivalence of Form 2 and Form 3 is verified by a conformance fixture.
- **Sub-partitioning** — a `Table` may have both `partition_by` (it is itself a partitioned parent) and `partition_of` (it is a partition of another parent).
- **Catalog reader** — two new queries: `SELECT_PARTITIONED_TABLES` (`pg_class.relkind='p'` + `pg_get_partkeydef`) reads partitioned-parent keys; `SELECT_PARTITIONS` (`relispartition=true` + `pg_get_expr(relpartbound, oid)`) reads partition children and re-parses the bounds text. Both filters apply `NOT EXISTS (pg_depend deptype='e')`.
- **Differ** — `TableChange::AttachPartition { parent, child, bounds }` and `TableChange::DetachPartition { parent, child }` variants. Bounds change on a stable parent → DetachPartition + AttachPartition. Parent `partition_by` rekey → `UnsupportedDiff` (no safe in-place path in Postgres). Column and constraint diff is suppressed when either the source or target side is a partition (columns are inherited from the parent).
- **Planner** — 2 new step kinds: `AttachPartition` (non-destructive) and `DetachPartition` (destructive; intent required). `AttachPartition` is placed in the same post-create ordering bucket as `CreateIndex`; `DetachPartition` is placed in the same destructive bucket as `DropTable`.
- **`NodeId` dep edge** — child partition → parent table (`DepSource::Structural`). Ensures the parent exists (and has its `partition_by` applied) before the child is attached.

### Added — lint rules (partitioning)

- `partition-references-unmanaged-parent` (Error) — a partition child's `partition_of.parent` schema is not in `[managed].schemas`. Prevents silent attach failures when the parent table is outside pgevolve's control.

### Added — tests (partitioning)

- **14 conformance fixtures** (Tier C): `objects/partitions/` covering:
  - `create-range-parent-and-two-partitions` — RANGE parent with `FOR VALUES FROM … TO …`.
  - `create-list-parent` — LIST parent.
  - `create-hash-parent-and-partitions` — HASH parent with `FOR VALUES WITH (MODULUS m, REMAINDER r)`.
  - `create-default-partition` — `DEFAULT` partition.
  - `add-partition` — attaching a new partition to an existing parent.
  - `drop-partition` — detach + destructive intent path.
  - `replace-bounds` — bounds change → detach + reattach.
  - `attach-existing-standalone` — Form 3 attach of a pre-existing standalone table.
  - `attach-form-vs-declarative-form-equivalent` — Form 2 vs Form 3 produce identical plans.
  - `detach-to-standalone` — detach leaves the child as a standalone table.
  - `subpartitioned` — sub-partitioned child (both `partition_by` and `partition_of` set).
  - `lint-unmanaged-parent` — `partition-references-unmanaged-parent` fires when parent is in an unmanaged schema.
  - Reject path fixtures for invalid bounds expressions.

### Changed — differ (partitioning)

- `diff_tables` now skips column and constraint diffing when either the source or the target side of a table pair is a partition (`partition_of.is_some()`). Partition children inherit their column list from the parent; diffing inherited columns produces spurious changes. The partition bounds themselves are diffed via `AttachPartition`/`DetachPartition` instead.

### Added — IR (extensions)

- `Extension { name, schema: Option<Identifier>, version: Option<String>, comment: Option<String> }` flat IR type in `pgevolve-core::ir::extension`.
- `Catalog::extensions: Vec<Extension>` flat collection. `canon::sort_and_dedupe` rejects duplicate extension names.

### Added — pipeline (extensions)

- **Source parser** — `CREATE EXTENSION [IF NOT EXISTS] name [WITH SCHEMA s] [VERSION 'v']` parses into the `Extension` IR. `CASCADE`, `FROM old_version`, and unknown options rejected with structural errors. `ALTER EXTENSION` in source files rejected at statement classification. `COMMENT ON EXTENSION` parsing added.
- **Catalog reader** — queries `pg_extension` joined with `pg_namespace` and `pg_description`. The reader for every other object kind (tables, indexes, sequences, functions, types, views, MVs) gains a `NOT EXISTS (pg_depend deptype='e')` filter so extension-owned objects never appear as drift.
- **Differ** — `ExtensionChange` variants: `Create`, `Drop`, `AlterUpdate`, `ReplaceWithCascade`, `CommentOn`. Source-`None` for schema, version, or comment means "any catalog value", so unpinned source declarations don't diff against any installed version.
- **Planner** — 4 new step kinds: `CreateExtension`, `DropExtension` (destructive), `AlterExtensionUpdate`, `CommentOnExtension`. Schema changes go through `DropExtension` + `CreateExtension` with linked intent.
- **`NodeId::Extension`** — added to the dep graph; `Extension → Schema` edges force the schema to exist before the extension is created.

### Added — lint rules (extensions)

- `extension-version-unpinned` (Warning) — `CREATE EXTENSION foo;` without a `VERSION` clause.
- `extension-references-unmanaged-schema` (Error) — `WITH SCHEMA gis` but `gis` isn't in the source catalog.

### Added — tests (extensions)

- **11 conformance fixtures** (Tier C): `objects/extensions/` covering create/drop/replace/comment paths plus version-pin and version-unpinned no-op cases. `scenarios/extension-owned-objects-ignored` exercises the `pg_depend deptype='e'` filter. `scenarios/create-order-schema-first` verifies the `Extension → Schema` dep ordering.

### Changed — conformance suite (extensions)

- Apply-layer post-check (`assertions::apply`) switched from strict `canonical_eq` to a differ-based convergence check. Source IR with `None` for schema/version/comment now correctly converges against any catalog reading that has concrete values for those fields.

### Added — IR (user-defined types)

- `UserType { qname, kind: UserTypeKind, comment }` flat IR type in `pgevolve-core::ir::user_type`.
- `UserTypeKind::Enum { values: Vec<EnumValue> }` — ordered label list with `sort_order: f32` mirroring `pg_enum.enumsortorder`.
- `UserTypeKind::Domain { base, nullable, default, check_constraints, collation }` — domain defaults and CHECK expressions use `NormalizedExpr` for canonical comparison.
- `UserTypeKind::Composite { attributes: Vec<CompositeAttribute> }` — each attribute carries name, type, and optional collation.
- `Catalog::types: Vec<UserType>` — flat collection, sorted by `qname` after `canonicalize()`.

### Added — pipeline (user-defined types)

- **Source parser** — `CREATE TYPE … AS ENUM`, `CREATE DOMAIN`, `CREATE TYPE … AS (…)` all parse into the `UserType` IR. Duplicate labels / attributes rejected at parse time.
- **AST resolution** — `UserDefined(QualifiedName)` column type references resolved against `Catalog::types` after the source parse pass.
- **Catalog reader** — queries `pg_type`, `pg_enum`, `pg_attribute` (for composites), and `pg_constraint` / `pg_attrdef` (for domains) to reconstruct `UserType` from a live database.
- **Differ** — `UserTypeChange` variants: `Create`, `Drop`, `EnumAddValue`, `EnumRenameValue`, `DomainAddCheck`, `DomainDropCheck`, `DomainSetDefault`, `DomainSetNotNull`, `CompositeAddAttribute`, `CompositeDropAttribute`, `CompositeAlterAttributeType`, `CommentOn`, `ReplaceWithCascade`.
- **Compatibility predicates** — `enum_can_alter_in_place` (preserved labels maintain relative order; renames position-paired) and `composite_can_alter_in_place` (preserved attributes maintain relative order). Both fall back to `ReplaceWithCascade` when the predicate returns `false`.
- **Planner** — 12 new step kinds: `CreateType`, `DropType`, `AlterTypeAddValue`, `AlterTypeRenameValue`, `AlterDomainAddConstraint`, `AlterDomainDropConstraint`, `AlterDomainSetDefault`, `AlterDomainSetNotNull`, `AlterTypeAddAttribute`, `AlterTypeDropAttribute`, `AlterTypeAlterAttributeType`, `CommentOnType`.
- **`NodeId::Type`** — added to the dep graph; edges from type → column (column's `ColumnType::UserDefined`) and type → type (domain base type) drive correct creation/drop ordering.

### Added — lint rules (user-defined types)

- `type-shadows-table` (Error) — a user-defined type shares a qualified name with a table, view, or MV.
- `enum-value-collision` (Error) — an enum type declares duplicate value labels.
- `composite-attribute-collision` (Error) — a composite type declares duplicate attribute names.
- `domain-check-references-unmanaged-type` (Warning) — a domain's CHECK expression references a schema outside `[managed].schemas`.

### Added — tests (user-defined types)

- **20 conformance fixtures** (Tier C): `objects/enums/` (8), `objects/domains/` (6), `objects/composites/` (4), `objects/user_type_lints/` (2).
- **Property test** `enum_add_value_preserves_existing_values` (`#[ignore]`, pure, no Docker) — for any random initial label list and a new distinct label, `diff_user_types` emits exactly one `EnumAddValue` change.

### Added — IR (views and materialized views)

- `View` and `MaterializedView` flat IR types in `pgevolve-core::ir::view`.
- `ViewColumn` — named column with resolved type and optional comment; used by both views and MVs.
- `body_canonical: NormalizedBody` — parsed-and-deparsed SELECT body in canonical form. Enables cosmetically-different but semantically-identical view bodies to diff equal.
- `body_dependencies: Vec<DepEdge>` — dependency edges extracted from the body AST with `DepSource::AstExtracted` provenance. Powers the dependent-recreation walk and the `view-body-references-unmanaged-schema` lint.
- `security_barrier` and `security_invoker` reloptions on `View`.

### Added — pipeline (views and materialized views)

- **AST canonicalization pass** (`parse/ast_canon.rs`) — runs after source parse; calls `NormalizedBody::from_sql` on each view body, extracts `DepEdge`s, resolves references against the provisional catalog, and fills in column types.
- **Catalog reader** — `read_views` and `read_materialized_views` query `pg_views` / `pg_matviews`, call `pg_get_viewdef`, and feed the result through `NormalizedBody::from_sql`. Source-side and catalog-side canonical texts are directly comparable.
- **Differ** — `ViewChange` and `MvChange` variants. OR-REPLACE compatibility predicate (`body_is_or_replace_compatible`) determines whether a body change emits `CREATE OR REPLACE VIEW` (compatible) or `DROP + CREATE` (incompatible).
- **Planner** — 7 new step kinds: `CreateView`, `DropView`, `CreateMaterializedView`, `DropMaterializedView`, `RefreshMaterializedView`, `AlterViewSetReloption`, `CommentOnView`.
- **Online rewrites** — `REFRESH MATERIALIZED VIEW CONCURRENTLY` upgrade (when unique index present); dependent-view recreation cascade (`recreate_views::extend_with_dependent_recreations`).

### Added — configuration

- `[planner.online_rewrites].refresh_mv_concurrently` (default `true`) — upgrade `REFRESH` to `REFRESH CONCURRENTLY` when the MV has a unique index.
- `[planner.online_rewrites].view_drop_create_dependents` (default `true`) — cascade dependent-view recreations; set `false` to error instead of auto-cascading.
- `[[step_override]]` rows in `intent.toml` — suppress individual plan steps by kind + target.

### Added — lint rules (views and materialized views)

- `view-shadows-table` (Error) — a view or MV shares a qualified name with a managed table.
- `mv-no-unique-index` (Warning) — an MV has no unique index; `REFRESH CONCURRENTLY` unavailable.
- `view-body-references-unmanaged-schema` (Warning) — a view body dependency edge points to an unmanaged schema.

### Added — tests (views and materialized views)

- **15 conformance fixtures** (Tier C): `objects/views/` (8), `objects/materialized_views/` (6), `intent/drop-view-requires-intent` (1), `scenarios/dependency-chains/` (2).
- **Property test** `view_canonicalization_closed_under_pg_rewrite` (`#[ignore]`, Docker-gated) — verifies `NormalizedBody::from_sql` closure under the PG rewrite for a fixed set of representative view bodies.
- **Property test** `arb_view_dependency_graph` (`#[ignore]`, Docker-free) — generates random view DAGs over a generated table corpus, mutates a leaf-table column, and asserts the resulting plan recreates exactly the transitively-dependent views in valid topological order. Closes the deferred test from sub-spec #1 §12.2. New `arbitrary_view_catalog` generator in `pgevolve-testkit`.

## [0.1.0] — 2026-05-17

First tagged release. The v0.1 surface manages **schemas, tables (with
columns/constraints/comments), indexes, and sequences** against Postgres
14, 15, 16, and 17.

### Added — pipeline

- **Parser** (`pgevolve-core::parse`) — `*.sql` → IR via `pg_query`. Tracks
  source locations for every parsed object. Recognises `-- @pgevolve`
  directives (`schema=…`, `dep:…`).
- **AST resolution pass** — runs between parse and canonicalize.
  Validates structural references (FKs against declared tables;
  default-using sequences against declared sequences). Surfaces
  unresolved references with source-located errors before any DB
  touch.
- **Catalog reader** (`pgevolve-core::catalog`) — live PG → IR via
  per-PG-major SQL strings and a sync `CatalogQuerier` trait. Returns
  `(Catalog, DriftReport)`. The drift report captures NOT VALID
  constraints and INVALID indexes for auto-recovery.
- **Differ** (`pgevolve-core::diff`) — pair-by-qname, structural;
  `ChangeSet` plus higher-level `Change` enum. Drift entries fold into
  `Change::ValidateConstraint` / `Change::RecreateIndex`.
- **Planner** (`pgevolve-core::plan`) — order → rewrite → group → wrap.
  Deterministic topo sort (Kahn + min-heap tiebreak). FK cycle
  extraction via `DeferredFkAdd`. Four online rewrites:
  `CREATE INDEX CONCURRENTLY`, `FK NOT VALID + VALIDATE`, `CHECK NOT
  VALID + VALIDATE`, `SET NOT NULL via CHECK pattern`.
- **Plan format** — three-file directory (`plan.sql`, `intent.toml`,
  `manifest.toml`); deterministic `PlanId` (BLAKE3 over bincoded
  canonical IRs); `[[intent]]` rows with `approved: bool`;
  `[[lint_waiver]]` rows to acknowledge `LintAtPlan` findings;
  `RecordedFinding` rows in `manifest.toml` for apply-time waiver
  recheck.
- **Executor** (`pgevolve::executor`) — bootstrap, advisory lock,
  per-step audit, preflight (target identity, drift recheck, intent
  approval, lint-waiver recheck). Per-group transactional or
  autocommit execution.
- **Linter** (`pgevolve-core::lint`) — universal rules + four built-in
  layout profiles (`schema-mirror`, `kind-grouped`, `feature-grouped`,
  `free-form`) plus a regex+assertion custom-profile mechanism. New
  `Severity::LintAtPlan` tier (gates plan with exit code 2 unless
  waived) and a new `column-position-drift` rule.

### Added — IR

- Top-level types: `Catalog`, `Schema`, `Table`, `Column`,
  `Constraint`, `Index`, `Sequence`, plus `ColumnType`, `DefaultExpr`,
  `NormalizedExpr`, `NormalizedBody` (the statement-scope counterpart
  for v0.2 body-bearing objects).
- Dep-graph types: `DepEdge { from, to, source: DepSource }` with
  `Structural` (v0.1) + `AstExtracted` / `AstDeclared` (v0.2)
  provenance.

### Added — CLI

- `pgevolve init` — scaffold project files.
- `pgevolve lint [--format human|json]` — universal + layout-profile
  rules.
- `pgevolve validate [--shadow] [--shadow-validate] [--shadow-strict]`
  — source-tree validation.
- `pgevolve diff --db <env> [--format human|json|sql]
  [--shadow-validate]` — print the change set.
- `pgevolve plan --db <env> [-o <dir>] [--shadow-validate]` — write a
  plan directory. Refuses with exit 2 on unwaived `LintAtPlan`
  findings.
- `pgevolve apply <plan-dir> --db <env>` — execute a plan.
- `pgevolve status --db <env>` — recent applies and per-step state.
- `pgevolve dump --db <env> -o <dir>` — introspect a live DB and write
  a fully-populated `schema/` tree via the new IR → SQL emitter
  (`pgevolve-core::render`).
- `pgevolve bootstrap --db <env>` — install/upgrade the metadata
  schema.
- `pgevolve graph [--graph-format dot|mermaid] [-o <path>]` — render
  the dep graph.
- `pgevolve doctor --db <env>` — project health check (drift, dangling
  intents, recent apply failures).
- `pgevolve rewrite-table <qname> --db <env> --confirm-rewrite` —
  skeleton; full implementation lands with a v0.2 sub-spec.

### Added — config

- `pgevolve.toml` with `[project]`, `[managed]`, `[planner]`,
  `[planner.online_rewrites]`, `[environments.<env>]`, and a new
  `[shadow]` block (`backend = auto | testcontainers | dsn`; per-backend
  `url`, `url_env`, `reset`, `extensions`, `postgres_version`).

### Added — test infrastructure

- `pgevolve-testkit` — `EphemeralPostgres`, `PgCatalogQuerier`,
  `MigrationFixture`, IR generator + mutator, `TestPgBackend`
  pluggable backend trait with testcontainers / compose / dsn impls
  (selected via `PGEVOLVE_TEST_PG_MODE`).
- `pgevolve-conformance` — Tier C suite with five fixture authoring
  subtrees (`objects/`, `scenarios/`, `intent/`, `failure/`,
  `regressions/`) and nine assertion layers (L1 diff, L2 plan
  structural, L3 plan-SQL golden, L4 apply roundtrip, L5 minimality,
  L6 no-collateral-damage, L7 intent shape, L8 dep-graph golden, L9
  topological order). Runtime budgets enforced per-fixture and
  suite-total.
- `dev/docker-compose.pg.yml` — PG 14/15/16/17 on stable ports for
  fast local test iteration in compose mode.
- `cargo xtask` subcommands: `bless --conformance`,
  `coverage --check|--gaps`, `fixture-cost`, `capture-regression`,
  `verify-regression`, `property-status`, `diagnose-pg-version`.

### Added — workflows

- `ci.yml` — fmt, clippy, unit + tier-2 tests, conformance matrix
  across PG 14/15/16/17, property-status compliance gate.
- `property-tests.yml` — nightly property test runs with
  auto-capture of failures into `regressions/`.
- `soak.yml` — weekly high-case property runs.

### Known limitations of v0.1

- `pgevolve rewrite-table` is a CLI skeleton — invoking it errors with
  "not yet implemented." The implementation lands with a v0.2 sub-spec
  (partitioning / column-type-change).
- `pgevolve dump` writes a single `schema.sql` file. Multi-file layout
  following `[project].layout_profile` is deferred to v0.1.x+.
- Views, materialized views, functions, procedures, triggers,
  user-defined types, extensions, and declarative partitioning are
  **not** in v0.1; they land per v0.2 sub-spec series.
- `--shadow-validate` is a scaffold cross-check. v0.1 has no body-
  bearing objects so the cross-check has nothing to do beyond a
  trivial structural-edge count; v0.2 sub-specs deepen it.

[Unreleased]: https://github.com/saosebastiao/pgevolve/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/saosebastiao/pgevolve/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/saosebastiao/pgevolve/releases/tag/v0.1.0
