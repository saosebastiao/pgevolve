---
status: skeleton
target_version: v0.4.2
sub_spec: pl-language-wiring
---

# PL-language wiring → non-SQL `FUNCTION` bodies — implementation plan (skeleton)

## Problem
pgevolve manages function bodies in SQL and plpgsql today. Bodies in
PL/Python, PL/Perl, PL/Tcl, or PL/v8 fail at IR-build time because the
parser can't validate dependencies inside an opaque body string. The
extensions surface already supports `CREATE EXTENSION plpython3u`, so
the language presence is half-solved; what's missing is the parser
contract for the body.

## Scope
- In: parse and store the body verbatim for non-SQL/plpgsql languages;
  do not attempt to extract internal SQL deps; require an explicit
  `-- @pgevolve dep:` directive list for any internal references (same
  mechanism plpgsql uses for dynamic SQL today).
- Out: actual SQL-dep extraction inside foreign-language bodies (PRs
  could add per-language extractors later as separate work).

## IR sketch
TBD — extend `Function::language` to a `Language` enum
(`Sql` | `PlPgSql` | `External(Identifier)`). For `External`, the body
canonicalization is a no-op (verbatim string match).

## Catalog reader notes
TBD — `pg_proc.prolang` → `pg_language.lanname`. Already partially used
for SQL/plpgsql; extend the readout to keep the language name verbatim
for non-built-ins.

## Conformance fixtures
TBD — `objects/functions/create-plpython-simple` (gated on
`plpython3u` extension being installed in the test image),
`create-plperl-simple`, `verbatim-body-roundtrip`,
`failure/functions/plpython-without-dep-directive-rejects-internal-sql-ref`.

## Open questions
- Do we manage `CREATE LANGUAGE` directly, or rely entirely on
  `CREATE EXTENSION plpython3u` etc.? (Modern Postgres makes the
  former a no-op in favor of the latter.)
- Verbatim body comparison vs. some form of canonicalization (strip
  trailing whitespace, normalize line endings)?

## Dependencies on other roadmap items
- Loose coupling with `EXTENSION` (must be installed first).
- Unblocks `AGGREGATE` state functions in arbitrary PL languages (v0.4.1
  ships with the constraint that state functions must be SQL/plpgsql;
  this plan lifts that constraint).
