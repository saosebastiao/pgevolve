---
status: skeleton
target_version: v0.5.0
sub_spec: fdw-family
---

# FDW family — implementation plan (skeleton)

## Problem
Postgres' foreign-data ecosystem (`postgres_fdw`, `file_fdw`, etc.) lets
applications reference data outside the local cluster as if it were
local. pgevolve manages none of the moving parts:
`FOREIGN DATA WRAPPER`, `SERVER`, `USER MAPPING`, `FOREIGN TABLE`, or
`IMPORT FOREIGN SCHEMA`. Without these, schemas that use FDWs are
unmanaged.

## Scope
- In: all five object kinds, full CRUD + comment surface, plus the
  secrets-handling story for `USER MAPPING` OPTIONS (mirrors the
  `${VAR}` env-var interpolation pattern already used by subscriptions
  in v0.3.5).
- Out: `IMPORT FOREIGN SCHEMA` as a *runtime* operation (it imports
  many foreign tables at once); pgevolve declarative model lists each
  imported foreign table explicitly, with an optional lint pointing at
  the source statement.

## IR sketch
TBD — five new collections under `Catalog`:
- `foreign_data_wrappers: Vec<ForeignDataWrapper>`
- `servers: Vec<Server>`
- `user_mappings: Vec<UserMapping>` — secrets-bearing
- `foreign_tables: Vec<ForeignTable>`
Plus reuse of existing `Table` machinery where possible (foreign tables
have columns and constraints).

## Catalog reader notes
TBD — `pg_foreign_data_wrapper`, `pg_foreign_server`,
`pg_user_mapping`, `pg_foreign_table`, `pg_attribute`. The
`umoptions` array on `pg_user_mapping` contains the secret material;
the executor must redact it from any diff output the same way
subscription `CONNECTION` strings are redacted.

## Conformance fixtures
TBD — `objects/fdws/create-postgres-fdw-server-and-foreign-table` (the
golden-path), plus per-object create/drop/alter fixtures. Secrets
fixtures must verify the env-var substitution path end-to-end.

## Open questions
- Should `USER MAPPING` be considered a *cluster* object (since it
  associates with a role) or a *schema* object? Probably cluster-ish —
  follow the cluster-tablespace pattern.
- IMPORT FOREIGN SCHEMA reconciliation: how to detect drift between
  source-declared foreign tables and what the foreign server actually
  exposes? Likely a lint, not a diff.

## Dependencies on other roadmap items
- Hard dep on the existing extensions surface (FDWs ship as extensions).
- Hard dep on the cluster-roles surface (USER MAPPING references roles).
- Reuses the secrets-interpolation machinery from v0.3.5 subscriptions.
