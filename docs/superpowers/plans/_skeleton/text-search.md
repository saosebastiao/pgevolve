---
status: skeleton
target_version: v0.4.3
sub_spec: text-search
---

# `TEXT SEARCH` family — implementation plan (skeleton)

## Problem
Full-text-search-aware indexes (`USING gin` on `tsvector` columns)
already work, but the configuration objects driving the tokenizer +
dictionary pipeline are not managed: `TEXT SEARCH CONFIGURATION`,
`TEXT SEARCH DICTIONARY`, `TEXT SEARCH PARSER`, `TEXT SEARCH TEMPLATE`.

## Scope
- In: all four object kinds — `CREATE`, `DROP`, `ALTER`,
  `COMMENT ON`, all variants documented in Postgres.
- Out: `CREATE TEXT SEARCH TEMPLATE` (templates need C functions —
  treat like base types: ⛔ Not planned within the spec but still
  *read* from catalog as opaque references).

## IR sketch
TBD — four new `Catalog::ts_*` collections:
- `ts_configurations: Vec<TsConfiguration>`
- `ts_dictionaries: Vec<TsDictionary>`
- `ts_parsers: Vec<TsParser>`
- `ts_templates: Vec<TsTemplate>` (read-only — write surface ⛔)

## Catalog reader notes
TBD — `pg_ts_config`, `pg_ts_config_map`, `pg_ts_dict`, `pg_ts_parser`,
`pg_ts_template`.

## Conformance fixtures
TBD — `objects/text_search/create-configuration`,
`add-mapping`, `alter-mapping`, `create-dictionary`, `drop`,
`comment-on`. Plus the index-on-tsvector regression fixture that
verifies search continues to work end-to-end.

## Open questions
- Configurations reference parsers + dictionaries — ordering in the
  dep graph matters; verify cycles aren't possible.
- COLLATION inputs to dictionaries — confirm v0.3.8 CREATE COLLATION
  is sufficient prereq.

## Dependencies on other roadmap items
- Depends on `CREATE COLLATION` (v0.3.8).
- Soft coupling with the extensions surface (e.g., `pg_trgm` registers
  dictionaries).
