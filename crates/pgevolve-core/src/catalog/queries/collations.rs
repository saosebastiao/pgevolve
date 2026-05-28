//! SQL queries for reading user-defined collations from `pg_collation`.
//!
//! Built-in collations (those in `pg_catalog` / `information_schema` namespaces)
//! and extension-owned collations (e.g., `citext`) are filtered out — only
//! user-created collations surface. Extension ownership is detected via the
//! `pg_depend.deptype = 'e'` subquery.
//!
//! PG 16 introduced `pg_collation.colliculocale` (ICU-only) for ICU
//! collations: ICU rows on PG 16 store their locale in `colliculocale` and
//! leave `collcollate` / `collctype` NULL. PG 17 added the `builtin` provider
//! and renamed `colliculocale` → `colllocale` (generic, since `builtin` rows
//! also use it). PG ≤15 has neither column and uses `collcollate` / `collctype`
//! for every provider.
//!
//! Three per-version SQL strings keep the dispatch simple:
//! - PG 14/15 query the legacy columns directly.
//! - PG 16 COALESCEs through `colliculocale`.
//! - PG 17/18 COALESCE through `colllocale`.

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

/// Per-managed-schema query for `pg_collation` on PG 16.
///
/// PG 16 added `colliculocale` (ICU-only) to hold the ICU locale string; for
/// ICU rows `collcollate` / `collctype` are NULL and the real locale lives in
/// `colliculocale`. We `COALESCE` so a single text column surfaces for every
/// provider.
pub const SELECT_COLLATIONS_PG16: &str = "\
SELECT \
    n.nspname::text AS schema, \
    c.collname::text AS name, \
    c.collprovider::text AS provider, \
    COALESCE(c.colliculocale, c.collcollate)::text AS lc_collate, \
    COALESCE(c.collctype, c.colliculocale, c.collcollate)::text AS lc_ctype, \
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

/// Per-managed-schema query for `pg_collation` on PG 17 and 18.
///
/// PG 17 renamed `colliculocale` → `colllocale` (generic, since the new
/// `builtin` provider also stores its locale in it). Otherwise identical to
/// the PG 16 variant.
pub const SELECT_COLLATIONS_PG17_PLUS: &str = "\
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
