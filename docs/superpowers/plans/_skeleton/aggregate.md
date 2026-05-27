---
status: skeleton
target_version: v0.4.1
sub_spec: aggregate
---

# `AGGREGATE` — implementation plan (skeleton)

## Problem
User-defined aggregates (`CREATE AGGREGATE`) wrap a state function plus
optional final/serial/deserial/combine functions to define application
aggregates (e.g., `weighted_avg(numeric, numeric)`). pgevolve doesn't
manage them.

## Scope
- In: `CREATE AGGREGATE`, `ALTER AGGREGATE ... RENAME TO`,
  `ALTER AGGREGATE ... OWNER TO`, `DROP AGGREGATE`, `COMMENT ON AGGREGATE`.
  Ordinary aggregates only (`sfunc` + `stype` + optional `finalfunc`,
  `initcond`).
- Out: ordered-set aggregates (`CREATE AGGREGATE ... ORDER BY`); moving
  aggregates (`MSFUNC` etc.); aggregates whose state function is in a PL
  language pgevolve does not yet read. Latter case is rejected at
  IR-build with a structured error; the constraint relaxes in v0.4.2
  when PL-language wiring lands.

## IR sketch
TBD — `Catalog::aggregates: Vec<Aggregate>` with fields: `qname`,
`arg_types: Vec<QualifiedName>`, `sfunc: QualifiedName`,
`stype: QualifiedName`, `finalfunc: Option<QualifiedName>`,
`initcond: Option<String>`, `comment`.

## Catalog reader notes
TBD — `pg_aggregate` joined with `pg_proc` for the wrapper proc and
again for `aggtransfn`. Identity is `(schema, name, arg_types)`.

## Conformance fixtures
TBD — `objects/aggregates/create-simple`, `create-with-finalfunc`,
`create-with-initcond`, `drop`, `comment-on`,
`failure/aggregates/reject-plpython-state-fn`.

## Open questions
- Identity collision with overloaded `FUNCTION`s: aggregates share the
  proc namespace; ensure dep graph routes correctly when an aggregate
  and a function have the same `(qname, arg_types)`.

## Dependencies on other roadmap items
- Soft dependency on the function surface for state functions
  (already supported for SQL / plpgsql).
- PL-language wiring (v0.4.2) lifts the language constraint.
