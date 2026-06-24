# Constraints

Constraint kinds pgevolve models, plus their attributes.

See [`../README.md`](./README.md) for the status legend.

## Kinds

| Kind | Status | Notes |
|---|---|---|
| `PRIMARY KEY` | ✅ Implemented | Single- and multi-column. `INCLUDE` (covering) columns supported.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/constraint.rs::tests`; tier-2: `parser/equivalent_pairs/0006-pk-inline-vs-table-constraint`; tier-C: `objects/tables/create-simple` |
| `UNIQUE` | ✅ Implemented | Single- and multi-column. `INCLUDE` and `NULLS NOT DISTINCT` (PG 15+) supported.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/constraint.rs::tests`; tier-C: `objects/tables/add-constraint-unique` |
| `FOREIGN KEY` | ✅ Implemented | Full attribute matrix below. The forward-reference cycle case is broken by the planner's FK-extraction post-pass.<br>**Tests:** tier-1: `crates/pgevolve-core/src/ir/constraint.rs::tests`, `plan/rewrite/fk_not_valid_validate.rs`; tier-C: `objects/tables/add-constraint-foreign-key`, `failure/ast-resolution/fk-to-missing-table` |
| `CHECK` | ✅ Implemented | Expression preserved as canonical text; redundant string-literal casts that Postgres's deparser adds (`'x'::text`) are normalized away so the source matches the introspected catalog. `NO INHERIT` flag supported.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/check_not_valid_validate.rs`; tier-C: `objects/tables/add-constraint-check`, `drop-constraint-check`, `check-string-literal` |
| `NOT NULL` (column-level) | ✅ Implemented | Modeled as `Column::nullable` rather than as a `Constraint`. The `SET NOT NULL via CHECK pattern` rewrite avoids long locks (see [`pipeline.md`](./pipeline.md)).<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/tests::set_not_null_on_existing_column_emits_four_steps` |
| `EXCLUSION` (`EXCLUDE USING gist (...)`) | 🔮 Future | Used primarily with range types; lands alongside range-type column support. |

## FOREIGN KEY attributes

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/constraint.rs::tests`, `parse/builder/create_stmt.rs::tests`; tier-C: `objects/tables/add-constraint-foreign-key`.

| Attribute | Status | Notes |
|---|---|---|
| Local columns + referenced columns | ✅ Implemented | Order significant. change_kinds: [add] |
| `ON UPDATE { NO ACTION | RESTRICT | CASCADE | SET NULL | SET DEFAULT }` | ✅ Implemented | change_kinds: [add] |
| `ON DELETE { NO ACTION | RESTRICT | CASCADE | SET NULL | SET DEFAULT }` | ✅ Implemented | change_kinds: [add] |
| `SET NULL (col, …)` / `SET DEFAULT (col, …)` (column-restricted action; PG 15+) | ✅ Implemented | change_kinds: [add] |
| `MATCH SIMPLE` (default) | ✅ Implemented | change_kinds: [add] |
| `MATCH FULL` | ✅ Implemented | change_kinds: [add] |
| `MATCH PARTIAL` | ⛔ Not planned | Never implemented by Postgres itself. |
| `DEFERRABLE` / `NOT DEFERRABLE`, `INITIALLY DEFERRED` / `INITIALLY IMMEDIATE` | ✅ Implemented | change_kinds: [add, set_deferrable] |
| `NOT VALID` + `VALIDATE CONSTRAINT` rewrite for adds on existing tables | ✅ Implemented | See [`pipeline.md`](./pipeline.md).<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/fk_not_valid_validate.rs` |
| `NOT VALID` constraints persisted as-is | ⛔ Not planned | The IR represents only fully-validated constraints; the `NOT VALID` state is an intermediate planner artifact. |
| NOT VALID drift detection and auto-resolution | ✅ Implemented | The catalog reader detects `pg_constraint.convalidated = false` (from a partial-apply) and the differ emits `Change::ValidateConstraint`. The planner emits `ALTER TABLE ... VALIDATE CONSTRAINT`. No user action required. See [`pipeline.md`](./pipeline.md).<br>**Tests:** tier-2: `crates/pgevolve-core/tests/catalog_drift.rs` |

## CHECK attributes

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/constraint.rs::tests`, `plan/rewrite/check_not_valid_validate.rs`; tier-C: `objects/tables/add-constraint-check`, `drop-constraint-check`.

| Attribute | Status | Notes |
|---|---|---|
| Boolean predicate | ✅ Implemented | Preserved as canonical text. change_kinds: [add] |
| `NO INHERIT` | ✅ Implemented | change_kinds: [add] |
| `NOT VALID` + `VALIDATE CONSTRAINT` rewrite for adds on existing tables | ✅ Implemented | change_kinds: [validate] |
| Table-level vs. column-level placement | ✅ Implemented | Treated identically at IR level. change_kinds: [add] |

## PRIMARY KEY / UNIQUE attributes

**Tests (whole section):** tier-1: `crates/pgevolve-core/src/ir/constraint.rs::tests`; tier-2: `parser/equivalent_pairs/0006-pk-inline-vs-table-constraint`.

| Attribute | Status | Notes |
|---|---|---|
| `INCLUDE (col, …)` covering columns | ✅ Implemented | change_kinds: [add] |
| `NULLS NOT DISTINCT` (UNIQUE only, PG 15+) | ✅ Implemented | change_kinds: [add] |
| `WITH (storage_parameter = ...)` (constraint reloptions) | 🔮 Future | The IR doesn't yet model constraint storage parameters. |
| `USING INDEX <name>` (attach existing index) | ⛔ Not planned | pgevolve always creates the underlying index implicitly; reusing pre-existing indexes is an adoption-path corner case. |
| `USING INDEX TABLESPACE <name>` | 🔮 Future | Lands with broader tablespace modeling. |

## Constraint-level features

| Feature | Status | Notes |
|---|---|---|
| Constraint name preserved across diff | ✅ Implemented | Constraints are paired by qname; renaming a constraint registers as drop+add.<br>**Tests:** tier-1: `crates/pgevolve-core/src/diff/constraints.rs::tests` |
| Unnamed-constraint auto-naming | ✅ Implemented | An unnamed constraint is auto-named exactly as Postgres's `ChooseIndexName`/`ChooseConstraintName` would, so it pairs with the introspected catalog instead of showing a spurious drop+add: `{table}_pkey`, `{table}_{col}_key` (UNIQUE), `{table}_{col}_check` (column CHECK) / `{table}_check` (table CHECK), with `makeObjectName` length truncation and collision counters. `LIKE` copies constraint names verbatim, as Postgres does. Verified byte-for-byte against live PG 14–18.<br>**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/choose_name.rs::tests`, `parse/builder/create_stmt.rs::tests`; tier-3: `crates/pgevolve-core/tests/table_like_round_trip.rs` |
| Constraint comments | ✅ Implemented | **Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/comment_stmt.rs::tests` |
| `ALTER TABLE ... VALIDATE CONSTRAINT` as an explicit step | ✅ Implemented | Used by the FK / CHECK NOT VALID rewrites.<br>**Tests:** tier-1: `crates/pgevolve-core/src/plan/rewrite/fk_not_valid_validate.rs`, `check_not_valid_validate.rs`; tier-2: `crates/pgevolve-core/tests/catalog_drift.rs` |
| `ALTER TABLE ... RENAME CONSTRAINT` | 🔮 Future | Today a rename diffs as drop+add (semantically equivalent but a larger lock). |
| Multi-column constraint reordering | ⛔ Not planned | The column order inside a constraint is semantically meaningful; changing it is a drop+add. |
