# Collations

pgevolve models `CREATE COLLATION` as a first-class declarative IR object.
A collation is a schema-qualified object (`pg_collation`) that names a
locale-data provider plus the `lc_collate` / `lc_ctype` strings used by
sort and ctype-aware comparisons. Per-column `COLLATE` clauses (always
supported) reference collations by qname; v0.3.8 adds the ability to
manage the collation objects themselves.

## Source surface

Three syntactic forms are supported, mirroring Postgres:

```sql
-- Form 1: libc + locale shorthand (default provider).
CREATE COLLATION app.de_DE (locale = 'de_DE.utf8');

-- Form 2: explicit provider + locale shorthand.
CREATE COLLATION app.case_insensitive
    (provider = icu, locale = 'und', deterministic = false);

-- Form 3: explicit lc_collate + lc_ctype.
CREATE COLLATION app.mixed
    (provider = libc, lc_collate = 'C', lc_ctype = 'en_US.utf8');
```

The IR always stores `lc_collate` + `lc_ctype` separately. When the source
used `locale = 'X'`, the parser normalizes to `lc_collate = 'X'` +
`lc_ctype = 'X'`. The renderer collapses back to the `locale = '...'`
shorthand when the two are equal, so the round-trip is lossless and
canonical.

**Tests:** tier-1: `crates/pgevolve-core/src/parse/builder/create_collation_stmt.rs`;
tier-C: `objects/collations/`.

## IR shape

`Collation` is a flat struct in `pgevolve-core::ir::collation`:

| Field | Type | Notes |
|---|---|---|
| `qname` | `QualifiedName` | `schema.collation_name` |
| `provider` | `CollationProvider` | `Libc` \| `Icu` \| `Builtin` (PG 17+) |
| `lc_collate` | `String` | From `pg_collation.collcollate` |
| `lc_ctype` | `String` | From `pg_collation.collctype` |
| `deterministic` | `bool` | Default `true`. PG 12+; ICU only when `false` |
| `version` | `Option<String>` | Read-only `pg_collation.collversion`. Differ ignores; `ALTER COLLATION … REFRESH VERSION` deferred to v0.3.9 |
| `owner` | `Option<Identifier>` | Lenient: `None` = unmanaged, `Some(role)` = differ emits `ALTER COLLATION … OWNER TO` |
| `comment` | `Option<String>` | `COMMENT ON COLLATION qname IS '…'` |

`Catalog::collations: Vec<Collation>` — flat collection, sorted by
`qname` after `canonicalize()`. The canon pass also rejects the invalid
`libc + nondeterministic` combination (Postgres would reject at runtime;
pgevolve surfaces it at canon time with a clearer error).

`BUILTIN_COLLATIONS: &[&str]` — `default`, `C`, `POSIX`, `und-x-icu`,
`unicode`, `ucs_basic`. These shortnames bypass the
`column-references-unmanaged-collation` lint even when they have no
schema qualifier, because Postgres seeds them at `initdb` and they are
always available.

## Semantics — lenient at the collation grain

pgevolve applies leniency at the **whole-collation** level (consistent
with publications / subscriptions / statistics):

| source | catalog | differ action |
|---|---|---|
| Collation absent | Collation absent | no-op |
| Collation absent | Collation present | **no-op** — surfaced as `unmanaged-collation` lint warning |
| Collation present | Collation absent | `CREATE COLLATION` |
| Collation present, same | Collation present, same | no-op |
| Collation present, comment differs | Collation present, differs | `COMMENT ON COLLATION` |
| Collation present, owner differs | Collation present, differs | `ALTER COLLATION … OWNER TO` |
| Collation present, structural change (provider / lc_collate / lc_ctype / deterministic) | Collation present, differs | `DROP COLLATION` + `CREATE COLLATION` (`ReplaceCollation`; destructive) |

Postgres has no in-place `ALTER COLLATION` for provider / locale /
deterministic, so any structural change emits `ReplaceCollation`, which
is destructive and requires intent approval.

## 5 StepKind variants

| Step kind | SQL emitted |
|---|---|
| `CreateCollation` | `CREATE COLLATION qname (provider = …, locale = …, deterministic = …)` |
| `DropCollation` | `DROP COLLATION qname` (destructive; intent required) |
| `RenameCollation` | `ALTER COLLATION qname RENAME TO new_name` |
| `ReplaceCollation` | `DROP COLLATION` + `CREATE COLLATION` (structural change; destructive) |
| `CommentOnCollation` | `COMMENT ON COLLATION qname IS '…'` |

`ALTER COLLATION … OWNER TO` is emitted via the standard
`Change::AlterObjectOwner` path with `OwnedObjectId::Qualified`, shared
with every other ownable IR kind.

## 5 lint rules

| Rule | Severity | Condition | Waivable? |
|---|---|---|---|
| `unmanaged-collation` | Warning | A collation is in the catalog but not in source | Yes |
| `column-references-unmanaged-collation` | Warning | A column's `collation` references a collation outside `[managed].schemas` and not in `BUILTIN_COLLATIONS` | Yes |
| `range-type-references-unmanaged-subtype` | Warning | A `Range` user type's `subtype` references a user type outside `[managed].schemas` and not a `pg_catalog` built-in | Yes |
| `nondeterministic-collation-requires-pg-12` | Error | A `Collation` has `deterministic = false` but `[managed].min_pg_version < 12` | No |
| `builtin-provider-requires-pg-17` | Error | A `Collation` uses `CollationProvider::Builtin` but `[managed].min_pg_version < 17` | No |

**Tests:** tier-1:
`crates/pgevolve-core/src/lint/rules/unmanaged_collation.rs::tests`,
`column_references_unmanaged_collation.rs::tests`,
`range_type_references_unmanaged_subtype.rs::tests`,
`nondeterministic_collation_requires_pg_12.rs::tests`,
`builtin_provider_requires_pg_17.rs::tests`;
tier-C: `objects/collations/`.

## Conformance fixture pointers

7 fixtures cover the collation surface (+1 cross-cutting scenario):

| Fixture | Covers |
|---|---|
| `objects/collations/create-libc` | Basic libc + locale shorthand |
| `objects/collations/create-icu` | ICU provider + locale shorthand |
| `objects/collations/create-nondeterministic` | ICU + `deterministic = false` |
| `objects/collations/drop` | `DropCollation` |
| `objects/collations/comment-on` | `CommentOnCollation` |
| `objects/collations/replace-on-locale-change` | Structural change → `ReplaceCollation` |
| `scenarios/column-references-managed-collation` | Cross-cutting: column `COLLATE` references a managed `Collation` |

## Dependency edges

| Edge | Meaning |
|---|---|
| `Table → Collation` | A column with `collation = Some(qname)` adds an edge so the collation is created before the table that uses it |

The edge is `DepSource::Structural`. `Range → Collation` is also added
when a `UserTypeKind::Range` carries a `collation: Some(qname)`.

## Property tests (v0.3.8)

`crates/pgevolve-testkit/src/ir_generator/collation.rs` generates 0–2
libc collations per managed schema with a deterministic-only, safe-locale
pool. ICU + nondeterministic + builtin variants are deliberately excluded
from the proptest soak to avoid PG-version gating; the conformance
fixtures cover those paths instead.

## Out of scope (deferred to v0.3.9+)

- **`CREATE COLLATION FROM existing_collation`** — clone syntax. Round-trip
  identity is ambiguous; the source declares a clone but the catalog
  shows the resolved provider / locale. Deferred. See the
  [design doc](../superpowers/specs/2026-05-28-collation-and-range-type-design.md).
- **`ALTER COLLATION … REFRESH VERSION`** — read-only `version` field is
  modeled but the differ ignores it. A `collation-version-drift` lint
  and explicit REFRESH step are planned for v0.3.9. See the design doc
  linked above.
- **`GRANT` on collations** — Postgres does not support object-level grants
  on `pg_collation` objects. Out of scope by PG design.
