# Lint and layout

Universal lint rules (always applied), built-in layout profiles (one of
four selected per project), and the custom-profile mechanism.

See [`../README.md`](./README.md) for the status legend.

## Universal rules

These apply regardless of layout profile and most are enforced at parse
time. The lint engine runs them defensively over a built `SourceTree`.

| Rule | Status | Enforced by | Tests |
|---|---|---|---|
| Every statement parses cleanly under `pg_query` | ✅ Implemented | Parser. | tier-1: `crates/pgevolve-core/src/parse/statement.rs::tests`; tier-C: `failure/parse/duplicate-schema` |
| Every `CREATE` is schema-qualified or has a file-level `-- @pgevolve schema=...` directive | ✅ Implemented | Parser. | tier-1: `crates/pgevolve-core/src/parse/directives.rs::tests`, `parse/mod.rs::tests` |
| No object qname appears twice across the tree | ✅ Implemented | Parser (raises `DuplicateObject`); lint double-checks. | tier-1: `crates/pgevolve-core/src/lint/rules/no_duplicate_qnames.rs`; tier-C: `failure/parse/duplicate-schema` |
| Only v0.1 MVP object kinds appear in source | ✅ Implemented | Parser (raises `UnsupportedObjectKind`). | tier-1: `crates/pgevolve-core/src/parse/statement.rs::tests` |
| No `ALTER` statement outside the FK forward-reference whitelist | ✅ Implemented | Parser. | tier-1: `crates/pgevolve-core/src/parse/builder/alter_table_stmt.rs::tests` |
| Every FK target table exists in the source tree | ✅ Implemented | Lint engine (`closed_world_references`). | tier-1: `crates/pgevolve-core/src/lint/rules/closed_world_references.rs`; tier-C: `failure/ast-resolution/fk-to-missing-table` |
| Every indexed table exists in the source tree | ✅ Implemented | Lint engine. | tier-1: `crates/pgevolve-core/src/lint/rules/closed_world_references.rs` |
| Every sequence's `OWNED BY` target exists in the source tree | ✅ Implemented | Lint engine. | tier-1: `crates/pgevolve-core/src/lint/rules/closed_world_references.rs` |
| `[managed].schemas` matches the schemas declared in source (two-way) | ✅ Implemented | Lint engine (`managed_schemas_match`). Silent when `managed.schemas` is empty. | tier-1: `crates/pgevolve-core/src/lint/rules/managed_schemas_match.rs` |
| Every column referenced by a constraint exists in its parent table | 🔮 Future | Mostly caught by Postgres at apply time; could be brought forward to lint time. | |
| Every type referenced by a column exists (or is built-in) | 🔮 Future | Same: caught by Postgres today. | |
| `column-position-drift` — table's column order in source disagrees with target catalog | ✅ Implemented | Severity `LintAtPlan` (see below). Source is canonical. Resolution: reorder source, add `[[lint_waiver]]` in `intent.toml`, or run `pgevolve rewrite-table`. | tier-1: `crates/pgevolve-core/src/lint/rules/column_position_drift.rs`; tier-2: `crates/pgevolve-core/tests/lint_position_drift.rs`; tier-C: `failure/lint-at-plan/column-position-drift-no-waiver` |
| `view-shadows-table` — a VIEW or MATERIALIZED VIEW shares a qualified name with a managed table | ✅ Implemented | Severity `Error`. Views and tables occupy the same namespace in Postgres; pgevolve rejects the ambiguity at parse time. | tier-1: `crates/pgevolve-core/src/lint/rules/view_shadows_table.rs` |
| `mv-no-unique-index` — a MATERIALIZED VIEW has no unique index and thus cannot use `REFRESH CONCURRENTLY` | ✅ Implemented | Severity `Warning`. Resolution: add a unique index on the MV, or set `refresh_mv_concurrently = false` in `[planner.online_rewrites]`. | tier-1: `crates/pgevolve-core/src/lint/rules/mv_no_unique_index.rs`; tier-C: `objects/materialized_views/create-no-unique-index-online` |
| `view-body-references-unmanaged-schema` — a view body dependency edge points to a schema not in `[managed].schemas` | ✅ Implemented | Severity `Warning`. pgevolve cannot track schema changes for objects it does not manage; a cross-schema dependency is a portability risk. | tier-1: `crates/pgevolve-core/src/lint/rules/view_body_references_unmanaged_schema.rs` |
| `type-shadows-table` — a user-defined type shares a qualified name with a managed table, view, or MV | ✅ Implemented | Severity `Error`. Postgres uses one namespace for relations and types; the conflict would be rejected at apply time. | tier-1: `crates/pgevolve-core/src/lint/rules/type_shadows_table.rs` |
| `enum-value-collision` — an enum type declares duplicate value labels | ✅ Implemented | Severity `Error`. Defense-in-depth; the source parser also rejects duplicates. | tier-1: `crates/pgevolve-core/src/lint/rules/enum_value_collision.rs` |
| `composite-attribute-collision` — a composite type declares duplicate attribute names | ✅ Implemented | Severity `Error`. Defense-in-depth; the source parser also rejects duplicates. | tier-1: `crates/pgevolve-core/src/lint/rules/composite_attribute_collision.rs` |
| `domain-check-references-unmanaged-type` — a domain's CHECK expression references a schema not in `[managed].schemas` | ✅ Implemented | Severity `Warning`. pgevolve cannot track changes to objects it does not manage; the reference is a portability risk. Silent when `[managed].schemas` is empty. | tier-1: `crates/pgevolve-core/src/lint/rules/domain_check_references_unmanaged_type.rs` |
| `plpgsql-dynamic-sql` — PL/pgSQL body uses `EXECUTE` without a `-- @pgevolve dep:` directive | ✅ Implemented | Severity `Error`. Resolved by adding `-- @pgevolve dep: schema.name` directives to declare the dynamic references explicitly. | tier-1: `crates/pgevolve-core/src/lint/rules/pl_pgsql_dynamic_sql.rs`; tier-C: `objects/functions/function-with-dynamic-sql-directive`, `scenarios/function-with-dynamic-sql-directive-clears-lint` |
| `procedure-contains-commit` — procedure body contains `COMMIT` or `ROLLBACK` | ✅ Implemented | Severity `Warning`. pgevolve auto-detects transaction control statements and runs the step with `transactional=OutsideTransaction`. | tier-1: `crates/pgevolve-core/src/lint/rules/procedure_contains_commit.rs`; tier-C: `objects/procedures/create-with-commit` |
| `function-references-unmanaged-schema` — routine body dep edge targets an unmanaged schema | ✅ Implemented | Severity `Warning`. pgevolve cannot track changes to objects it does not manage; the cross-schema dependency is a portability risk. Silent when `[managed].schemas` is empty. | tier-1: `crates/pgevolve-core/src/lint/rules/function_references_unmanaged_schema.rs` |

## Severity tiers

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/lint/mod.rs` (Severity enum), `crates/pgevolve-core/src/lint/rules/mod.rs::tests`; tier-4: `crates/pgevolve/tests/lint_waiver_e2e.rs::plan_refuses_unwaived_column_position_drift`, `plan_proceeds_with_matching_lint_waiver`.

| Tier | Status | Behaviour |
|---|---|---|
| `Error` | ✅ Implemented | Fails lint (exit 1). |
| `Warning` | ✅ Implemented | Reported but does not fail lint. |
| `LintAtPlan` | ✅ Implemented | Drift / divergence detected at plan time that pgevolve declines to act on without explicit user instruction. `pgevolve plan` exits with code 2 unless the finding is waived via a matching `[[lint_waiver]]` row in `intent.toml`. |

## Layout profiles

A profile expresses *where* an object should live on disk. Selected by
`[project].layout_profile`. All built-ins ship in
`pgevolve_core::lint::profile`.

### `schema-mirror` (strictest)

**Tests:** tier-1: `crates/pgevolve-core/src/lint/profile/schema_mirror.rs::tests`.

| Convention | Status | Notes |
|---|---|---|
| Tables, indexes, sequences live at `<schema>/<kind_plural>/<name>.sql` | ✅ Implemented | `<kind_plural>` is `tables` / `indexes` / `sequences`. |
| Schemas live at `<schema>/_schema.sql` | ✅ Implemented | Where you put the `CREATE SCHEMA` for that schema. |
| One object per file (schemas excepted) | ✅ Implemented | |

### `kind-grouped`

**Tests:** tier-1: `crates/pgevolve-core/src/lint/profile/kind_grouped.rs::tests`.

| Convention | Status | Notes |
|---|---|---|
| Tables / indexes / sequences live at `<kind_plural>/<schema>.<name>.sql` | ✅ Implemented | |
| Schemas live at `schemas/<name>.sql` | ✅ Implemented | |
| One object per file | ✅ Implemented | |

### `feature-grouped`

**Tests:** tier-1: `crates/pgevolve-core/src/lint/profile/feature_grouped.rs::tests`.

| Convention | Status | Notes |
|---|---|---|
| Every file lives under `<schema_dir>/<some-feature-dir>/` (no direct children) | ✅ Implemented | |
| Multiple objects per file are allowed | ✅ Implemented | |
| Cross-feature overlap forbidden (no object spans two feature dirs) | 🔮 Future | Rigorously defining "overlap" was non-trivial; lighter spec-only check ships now, fuller version when there is clear demand. |

### `free-form`

**Tests:** tier-1: `crates/pgevolve-core/src/lint/profile/free_form.rs::tests`.

| Convention | Status | Notes |
|---|---|---|
| No path constraints | ✅ Implemented | Only universal rules apply. |

### `custom`

A user-defined profile loaded from a TOML path passed in
`[project].layout_profile`.

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/lint/profile/custom.rs::tests`.

| Mechanism | Status | Notes |
|---|---|---|
| `[[patterns]]` table with `regex` + `assertions` | ✅ Implemented | Regex applied to the path relative to `schema_dir`. First match wins. |
| Assertion: `schema_matches_capture` | ✅ Implemented | Requires the regex's `?P<schema>` capture to equal the object's `qname.schema`. |
| Assertion: `name_matches_capture` | ✅ Implemented | Requires the regex's `?P<name>` capture to equal the object's bare name. |
| Assertion: `kind_matches_capture` with `allowed_values = { capture_value = "kind", … }` | ✅ Implemented | Maps the regex's `?P<kind>` capture to one of `schema` / `table` / `index` / `sequence`. |
| Assertion: `one_object_per_file` | ✅ Implemented | |
| Embedded scripting (Rhai / Lua / …) | ⛔ Not planned | Out of scope for v0.1; the regex+assertion mechanism is intentionally declarative. |

## Lint output

| Aspect | Status | Notes |
|---|---|---|
| `Severity::Error` / `Severity::Warning` | ✅ Implemented | Errors fail the lint (exit 1); warnings don't.<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs::lint_default_format_is_human` |
| Stable rule identifiers (`managed_schemas_match`, `schema_mirror_path`, …) | ✅ Implemented | Used for filtering and `--explain` in the future.<br>**Tests:** tier-1: `crates/pgevolve-core/src/lint/rules/mod.rs::tests` |
| Source location (`file:line:column`) on every finding | ✅ Implemented | When available; some findings (e.g., aggregated profile rules) don't have a single location.<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs::lint_json_format_emits_structured_output` |
| `--explain <rule>` to print the rule's rationale + example fix | 🔮 Future | Lands when there are enough rules to make explanations valuable. |
| `--deny <rule>` / `--allow <rule>` overrides | 🔮 Future | Configurable per-rule severity. |
| `--format json` lint output | ✅ Implemented | `pgevolve lint --format json` emits a stable structured document with `findings[]`, `total`, and `errors`. Severity values are stringified (`"error"`, `"warning"`, `"lint-at-plan"`). `--format sql` is rejected for lint (sql output is meaningful only for `diff`).<br>**Tests:** tier-4: `crates/pgevolve/tests/lint_format.rs::lint_json_format_emits_structured_output`, `lint_sql_format_is_rejected` |
