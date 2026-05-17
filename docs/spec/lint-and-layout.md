# Lint and layout

Universal lint rules (always applied), built-in layout profiles (one of
four selected per project), and the custom-profile mechanism.

See [`../README.md`](./README.md) for the status legend.

## Universal rules

These apply regardless of layout profile and most are enforced at parse
time. The lint engine runs them defensively over a built `SourceTree`.

| Rule | Status | Enforced by |
|---|---|---|
| Every statement parses cleanly under `pg_query` | ✅ Implemented | Parser. |
| Every `CREATE` is schema-qualified or has a file-level `-- @pgevolve schema=...` directive | ✅ Implemented | Parser. |
| No object qname appears twice across the tree | ✅ Implemented | Parser (raises `DuplicateObject`); lint double-checks. |
| Only v0.1 MVP object kinds appear in source | ✅ Implemented | Parser (raises `UnsupportedObjectKind`). |
| No `ALTER` statement outside the FK forward-reference whitelist | ✅ Implemented | Parser. |
| Every FK target table exists in the source tree | ✅ Implemented | Lint engine (`closed_world_references`). |
| Every indexed table exists in the source tree | ✅ Implemented | Lint engine. |
| Every sequence's `OWNED BY` target exists in the source tree | ✅ Implemented | Lint engine. |
| `[managed].schemas` matches the schemas declared in source (two-way) | ✅ Implemented | Lint engine (`managed_schemas_match`). Silent when `managed.schemas` is empty. |
| Every column referenced by a constraint exists in its parent table | 🔮 Future | Mostly caught by Postgres at apply time; could be brought forward to lint time. |
| Every type referenced by a column exists (or is built-in) | 🔮 Future | Same: caught by Postgres today. |
| `column-position-drift` — table's column order in source disagrees with target catalog | ✅ Implemented | Severity `LintAtPlan` (see below). Source is canonical. Resolution: reorder source, add `[[lint_waiver]]` in `intent.toml`, or run `pgevolve rewrite-table`. |
| `plpgsql-dynamic-sql` — PL/pgSQL function body contains opaque `EXECUTE` patterns | 📋 Planned, v0.2 | Severity `Warning`. Resolved by adding `-- @pgevolve dep:` directives. |

## Severity tiers

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

| Convention | Status | Notes |
|---|---|---|
| Tables, indexes, sequences live at `<schema>/<kind_plural>/<name>.sql` | ✅ Implemented | `<kind_plural>` is `tables` / `indexes` / `sequences`. |
| Schemas live at `<schema>/_schema.sql` | ✅ Implemented | Where you put the `CREATE SCHEMA` for that schema. |
| One object per file (schemas excepted) | ✅ Implemented | |

### `kind-grouped`

| Convention | Status | Notes |
|---|---|---|
| Tables / indexes / sequences live at `<kind_plural>/<schema>.<name>.sql` | ✅ Implemented | |
| Schemas live at `schemas/<name>.sql` | ✅ Implemented | |
| One object per file | ✅ Implemented | |

### `feature-grouped`

| Convention | Status | Notes |
|---|---|---|
| Every file lives under `<schema_dir>/<some-feature-dir>/` (no direct children) | ✅ Implemented | |
| Multiple objects per file are allowed | ✅ Implemented | |
| Cross-feature overlap forbidden (no object spans two feature dirs) | 🔮 Future | Rigorously defining "overlap" was non-trivial; lighter spec-only check ships now, fuller version when there is clear demand. |

### `free-form`

| Convention | Status | Notes |
|---|---|---|
| No path constraints | ✅ Implemented | Only universal rules apply. |

### `custom`

A user-defined profile loaded from a TOML path passed in
`[project].layout_profile`.

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
| `Severity::Error` / `Severity::Warning` | ✅ Implemented | Errors fail the lint (exit 1); warnings don't. |
| Stable rule identifiers (`managed_schemas_match`, `schema_mirror_path`, …) | ✅ Implemented | Used for filtering and `--explain` in the future. |
| Source location (`file:line:column`) on every finding | ✅ Implemented | When available; some findings (e.g., aggregated profile rules) don't have a single location. |
| `--explain <rule>` to print the rule's rationale + example fix | 🔮 Future | Lands when there are enough rules to make explanations valuable. |
| `--deny <rule>` / `--allow <rule>` overrides | 🔮 Future | Configurable per-rule severity. |
| `--format json` lint output | ✅ Implemented | `pgevolve lint --format json` emits a stable structured document with `findings[]`, `total`, and `errors`. Severity values are stringified (`"error"`, `"warning"`, `"lint-at-plan"`). `--format sql` is rejected for lint (sql output is meaningful only for `diff`). |
