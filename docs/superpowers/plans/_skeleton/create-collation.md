---
status: skeleton
target_version: v0.3.8
sub_spec: create-collation
---

# `CREATE COLLATION` — implementation plan (skeleton)

## Problem
pgevolve already references collations on columns (`per-column
collation` is supported), but `CREATE COLLATION` (defining new
collations from `lc_collate` / `lc_ctype` / `provider` / `locale` /
`deterministic`) is not managed. Text-search configurations and other
locale-sensitive objects need this prerequisite.

## Scope
- In: `CREATE COLLATION`, `DROP COLLATION`, `ALTER COLLATION ... RENAME
  TO`, `ALTER COLLATION ... REFRESH VERSION`, `COMMENT ON COLLATION`.
  Both libc and ICU providers.
- Out: `CREATE COLLATION ... FROM existing_collation` if catalog
  round-trip ambiguity proves intractable — re-evaluate during
  brainstorm.

## IR sketch
TBD — `Catalog::collations: Vec<Collation>` with fields: `qname`,
`provider` (`Libc` | `Icu`), `locale`, `lc_collate`, `lc_ctype`,
`deterministic: bool`, `version: Option<String>`, `comment`.

## Catalog reader notes
TBD — primary table is `pg_collation` joined with `pg_namespace`. Filter
out built-in collations (e.g., `default`, `C`, `POSIX`).

## Conformance fixtures
TBD — `objects/collations/create-icu`, `create-libc`,
`create-nondeterministic`, `alter-refresh-version`, `drop`, `comment-on`.

## Open questions
- How to handle collations whose `version` drifts after `pg_upgrade`?
  Probably a lint, not a diff.
- Built-in collation filter precision — exclude by `collprovider = 'c'
  AND collnamespace = 'pg_catalog'`?

## Dependencies on other roadmap items
- Unblocks `TEXT SEARCH` (v0.4.3).
