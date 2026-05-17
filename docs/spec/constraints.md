# Constraints

Constraint kinds pgevolve models, plus their attributes.

See [`../README.md`](./README.md) for the status legend.

## Kinds

| Kind | Status | Notes |
|---|---|---|
| `PRIMARY KEY` | ✅ Implemented | Single- and multi-column. `INCLUDE` (covering) columns supported. change_kinds: [add, drop] |
| `UNIQUE` | ✅ Implemented | Single- and multi-column. `INCLUDE` and `NULLS NOT DISTINCT` (PG 15+) supported. change_kinds: [add, drop] |
| `FOREIGN KEY` | ✅ Implemented | Full attribute matrix below. The forward-reference cycle case is broken by the planner's FK-extraction post-pass. change_kinds: [add, drop, validate] |
| `CHECK` | ✅ Implemented | Expression preserved as canonical text. `NO INHERIT` flag supported. change_kinds: [add, drop, validate] |
| `NOT NULL` (column-level) | ✅ Implemented | Modeled as `Column::nullable` rather than as a `Constraint`. The `SET NOT NULL via CHECK pattern` rewrite avoids long locks (see [`pipeline.md`](./pipeline.md)). change_kinds: [add, drop] |
| `EXCLUSION` (`EXCLUDE USING gist (...)`) | 🔮 Future | Used primarily with range types; lands alongside range-type column support. |

## FOREIGN KEY attributes

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
| `NOT VALID` + `VALIDATE CONSTRAINT` rewrite for adds on existing tables | ✅ Implemented | See [`pipeline.md`](./pipeline.md). change_kinds: [validate] |
| `NOT VALID` constraints persisted as-is | ⛔ Not planned | The IR represents only fully-validated constraints; the `NOT VALID` state is an intermediate planner artifact. |
| NOT VALID drift detection and auto-resolution | ✅ Implemented | The catalog reader detects `pg_constraint.convalidated = false` (from a partial-apply) and the differ emits `Change::ValidateConstraint`. The planner emits `ALTER TABLE ... VALIDATE CONSTRAINT`. No user action required. See [`pipeline.md`](./pipeline.md). |

## CHECK attributes

| Attribute | Status | Notes |
|---|---|---|
| Boolean predicate | ✅ Implemented | Preserved as canonical text. change_kinds: [add] |
| `NO INHERIT` | ✅ Implemented | change_kinds: [add] |
| `NOT VALID` + `VALIDATE CONSTRAINT` rewrite for adds on existing tables | ✅ Implemented | change_kinds: [validate] |
| Table-level vs. column-level placement | ✅ Implemented | Treated identically at IR level. change_kinds: [add] |

## PRIMARY KEY / UNIQUE attributes

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
| Constraint name preserved across diff | ✅ Implemented | Constraints are paired by qname; renaming a constraint registers as drop+add. change_kinds: [add, drop] |
| Constraint comments | ✅ Implemented | change_kinds: [set_comment] |
| `ALTER TABLE ... VALIDATE CONSTRAINT` as an explicit step | ✅ Implemented | Used by the FK / CHECK NOT VALID rewrites. change_kinds: [validate] |
| `ALTER TABLE ... RENAME CONSTRAINT` | 🔮 Future | Today a rename diffs as drop+add (semantically equivalent but a larger lock). |
| Multi-column constraint reordering | ⛔ Not planned | The column order inside a constraint is semantically meaningful; changing it is a drop+add. |
