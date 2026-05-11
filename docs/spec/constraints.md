# Constraints

Constraint kinds pgevolve models, plus their attributes.

See [`../README.md`](./README.md) for the status legend.

## Kinds

| Kind | Status | Notes |
|---|---|---|
| `PRIMARY KEY` | ✅ Implemented | Single- and multi-column. `INCLUDE` (covering) columns supported. |
| `UNIQUE` | ✅ Implemented | Single- and multi-column. `INCLUDE` and `NULLS NOT DISTINCT` (PG 15+) supported. |
| `FOREIGN KEY` | ✅ Implemented | Full attribute matrix below. The forward-reference cycle case is broken by the planner's FK-extraction post-pass. |
| `CHECK` | ✅ Implemented | Expression preserved as canonical text. `NO INHERIT` flag supported. |
| `NOT NULL` (column-level) | ✅ Implemented | Modeled as `Column::nullable` rather than as a `Constraint`. The `SET NOT NULL via CHECK pattern` rewrite avoids long locks (see [`pipeline.md`](./pipeline.md)). |
| `EXCLUSION` (`EXCLUDE USING gist (...)`) | 🔮 Future | Used primarily with range types; lands alongside range-type column support. |

## FOREIGN KEY attributes

| Attribute | Status | Notes |
|---|---|---|
| Local columns + referenced columns | ✅ Implemented | Order significant. |
| `ON UPDATE { NO ACTION | RESTRICT | CASCADE | SET NULL | SET DEFAULT }` | ✅ Implemented | |
| `ON DELETE { NO ACTION | RESTRICT | CASCADE | SET NULL | SET DEFAULT }` | ✅ Implemented | |
| `SET NULL (col, …)` / `SET DEFAULT (col, …)` (column-restricted action; PG 15+) | ✅ Implemented | |
| `MATCH SIMPLE` (default) | ✅ Implemented | |
| `MATCH FULL` | ✅ Implemented | |
| `MATCH PARTIAL` | ⛔ Not planned | Never implemented by Postgres itself. |
| `DEFERRABLE` / `NOT DEFERRABLE`, `INITIALLY DEFERRED` / `INITIALLY IMMEDIATE` | ✅ Implemented | |
| `NOT VALID` + `VALIDATE CONSTRAINT` rewrite for adds on existing tables | ✅ Implemented | See [`pipeline.md`](./pipeline.md). |
| `NOT VALID` constraints persisted as-is | ⛔ Not planned | The IR represents only fully-validated constraints; the `NOT VALID` state is an intermediate planner artifact. |

## CHECK attributes

| Attribute | Status | Notes |
|---|---|---|
| Boolean predicate | ✅ Implemented | Preserved as canonical text. |
| `NO INHERIT` | ✅ Implemented | |
| `NOT VALID` + `VALIDATE CONSTRAINT` rewrite for adds on existing tables | ✅ Implemented | |
| Table-level vs. column-level placement | ✅ Implemented | Treated identically at IR level. |

## PRIMARY KEY / UNIQUE attributes

| Attribute | Status | Notes |
|---|---|---|
| `INCLUDE (col, …)` covering columns | ✅ Implemented | |
| `NULLS NOT DISTINCT` (UNIQUE only, PG 15+) | ✅ Implemented | |
| `WITH (storage_parameter = ...)` (constraint reloptions) | 🔮 Future | The IR doesn't yet model constraint storage parameters. |
| `USING INDEX <name>` (attach existing index) | ⛔ Not planned | pgevolve always creates the underlying index implicitly; reusing pre-existing indexes is an adoption-path corner case. |
| `USING INDEX TABLESPACE <name>` | 🔮 Future | Lands with broader tablespace modeling. |

## Constraint-level features

| Feature | Status | Notes |
|---|---|---|
| Constraint name preserved across diff | ✅ Implemented | Constraints are paired by qname; renaming a constraint registers as drop+add. |
| Constraint comments | ✅ Implemented | |
| `ALTER TABLE ... VALIDATE CONSTRAINT` as an explicit step | ✅ Implemented | Used by the FK / CHECK NOT VALID rewrites. |
| `ALTER TABLE ... RENAME CONSTRAINT` | 🔮 Future | Today a rename diffs as drop+add (semantically equivalent but a larger lock). |
| Multi-column constraint reordering | ⛔ Not planned | The column order inside a constraint is semantically meaningful; changing it is a drop+add. |
