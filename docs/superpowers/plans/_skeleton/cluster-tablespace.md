---
status: skeleton
target_version: v0.4.2
sub_spec: cluster-tablespace
---

# `TABLESPACE` (cluster object) — implementation plan (skeleton)

## Problem
`CREATE TABLESPACE name OWNER role LOCATION '/path'` is currently
out-of-scope ("cluster-level admin object outside the schema-management
remit"). The 2026-05-26 roadmap reverses this: the `pgevolve cluster …`
surface already manages roles, so tablespaces fit the same model.
Filesystem-layout management (directory creation, mount points) stays
out of scope; only the SQL `CREATE TABLESPACE` step is managed.

## Scope
- In: `CREATE TABLESPACE`, `ALTER TABLESPACE ... OWNER TO`,
  `ALTER TABLESPACE ... RENAME TO`, `ALTER TABLESPACE ... SET (option)`,
  `DROP TABLESPACE`, `COMMENT ON TABLESPACE`. Owner attribution.
- Out: filesystem directory creation; `pg_tablespace_location()`
  validation that the path exists on disk; backup-relocation rules.

## IR sketch
TBD — new `cluster::tablespace::Tablespace` analogous to
`cluster::role::Role`. Fields: `name`, `owner: Identifier`,
`location: String`, `options: BTreeMap<String, String>` (seq_page_cost,
random_page_cost, effective_io_concurrency, maintenance_io_concurrency),
`comment`.

## Catalog reader notes
TBD — `pg_tablespace` joined with `pg_authid` for owner.

## Conformance fixtures
TBD — `cluster/tablespaces/create-simple`, `alter-owner`,
`alter-set-option`, `drop`, `comment-on`. Each fixture must provide a
real directory; testkit will need a temp-dir helper.

## Open questions
- Where do tablespace path strings live in `pgevolve.toml` — under
  `[cluster.tablespaces]`? Spec-level decision needed.
- Drift: what to do when the catalog has a tablespace at a different
  filesystem path than source declares? Re-create is destructive; lint
  is likely safer.

## Dependencies on other roadmap items
- Pairs with v0.4.0 per-partition `TABLESPACE` for the complete picture.
