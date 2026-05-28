//! SQL queries for reading user-defined collations from `pg_collation`.
//!
//! Built-in collations (those in `pg_catalog` / `information_schema` namespaces)
//! and extension-owned collations (e.g., `citext`) are filtered out — only
//! user-created collations surface. Extension ownership is detected via the
//! `pg_depend.deptype = 'e'` subquery.
//!
//! PG 16 introduced `pg_collation.colllocale` for ICU collations: ICU rows on
//! PG 16+ store their locale in `colllocale` and leave `collcollate` / `collctype`
//! NULL. PG ≤15 has no `colllocale` column and uses `collcollate` / `collctype`
//! for every provider. Two per-version SQL strings keep the dispatch simple:
//! PG 14/15 query the legacy columns directly; PG 16+ COALESCE the new column
//! into both `lc_collate` and `lc_ctype` so ICU rows decode correctly.
//!
//! PG 17 adds the `builtin` provider (`collprovider = 'b'`) but does not change
//! the column set, so the PG 16+ query string serves PG 16, 17, and 18.

/// Per-managed-schema query for `pg_collation` on PG 14 and 15.
///
/// Takes a single `text[]` parameter listing managed-schema names.
/// Returns one row per user-defined collation, ordered by `(schema, name)`.
///
/// Neither PG 14 nor PG 15 has the `colllocale` column, so we read
/// `collcollate` / `collctype` directly.
pub const SELECT_COLLATIONS_PG14_15: &str = "\
SELECT \
    n.nspname::text AS schema, \
    c.collname::text AS name, \
    c.collprovider::text AS provider, \
    c.collcollate::text AS lc_collate, \
    c.collctype::text AS lc_ctype, \
    c.collisdeterministic AS deterministic, \
    c.collversion::text AS version, \
    pg_catalog.pg_get_userbyid(c.collowner)::text AS owner, \
    pg_catalog.obj_description(c.oid, 'pg_collation')::text AS comment \
FROM pg_catalog.pg_collation c \
JOIN pg_catalog.pg_namespace n ON n.oid = c.collnamespace \
WHERE n.nspname <> 'pg_catalog' \
  AND n.nspname <> 'information_schema' \
  AND NOT EXISTS ( \
      SELECT 1 FROM pg_catalog.pg_depend d \
      WHERE d.classid = 'pg_catalog.pg_collation'::regclass \
        AND d.objid = c.oid \
        AND d.deptype = 'e' \
  ) \
  AND n.nspname = ANY($1::text[]) \
ORDER BY n.nspname, c.collname";

/// Per-managed-schema query for `pg_collation` on PG 16, 17, and 18.
///
/// PG 16 added `colllocale` to hold the ICU locale string; for ICU rows
/// `collcollate` / `collctype` are NULL and the real locale lives in
/// `colllocale`. We `COALESCE` so a single text column surfaces for every
/// provider: libc/builtin rows continue to use `collcollate` / `collctype`;
/// ICU rows fall back to `colllocale`.
///
/// `lc_ctype` additionally coalesces through `colllocale` because ICU rows
/// have NULL `collctype` as well — the locale is shared across collate / ctype
/// for ICU.
pub const SELECT_COLLATIONS_PG16_PLUS: &str = "\
SELECT \
    n.nspname::text AS schema, \
    c.collname::text AS name, \
    c.collprovider::text AS provider, \
    COALESCE(c.colllocale, c.collcollate)::text AS lc_collate, \
    COALESCE(c.collctype, c.colllocale, c.collcollate)::text AS lc_ctype, \
    c.collisdeterministic AS deterministic, \
    c.collversion::text AS version, \
    pg_catalog.pg_get_userbyid(c.collowner)::text AS owner, \
    pg_catalog.obj_description(c.oid, 'pg_collation')::text AS comment \
FROM pg_catalog.pg_collation c \
JOIN pg_catalog.pg_namespace n ON n.oid = c.collnamespace \
WHERE n.nspname <> 'pg_catalog' \
  AND n.nspname <> 'information_schema' \
  AND NOT EXISTS ( \
      SELECT 1 FROM pg_catalog.pg_depend d \
      WHERE d.classid = 'pg_catalog.pg_collation'::regclass \
        AND d.objid = c.oid \
        AND d.deptype = 'e' \
  ) \
  AND n.nspname = ANY($1::text[]) \
ORDER BY n.nspname, c.collname";
