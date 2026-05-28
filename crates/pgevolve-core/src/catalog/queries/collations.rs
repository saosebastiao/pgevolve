//! SQL queries for reading user-defined collations from `pg_collation`.
//!
//! Built-in collations (those in `pg_catalog` / `information_schema` namespaces)
//! and extension-owned collations (e.g., `citext`) are filtered out — only
//! user-created collations surface. Extension ownership is detected via the
//! `pg_depend.deptype = 'e'` subquery.
//!
//! The shape is stable across PG 14–18; PG 17 adds the `builtin` provider
//! (`collprovider = 'b'`) but the column set is unchanged. A single shared
//! string serves every supported version.

/// Per-managed-schema query for `pg_collation`.
///
/// Takes a single `text[]` parameter listing managed-schema names.
/// Returns one row per user-defined collation, ordered by `(schema, name)`.
pub const SELECT_COLLATIONS: &str = "\
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
