//! View / materialized-view catalog queries.
//!
//! Both views (`relkind='v'`) and materialized views (`relkind='m'`) are
//! introspected through the same SQL fragments. PG versions 14–17 expose the
//! same columns for these query shapes; the only version-sensitive reloption
//! (`security_invoker`) is simply absent from the `reloptions` array on PG 14
//! without requiring a different query string.

/// Query for all views and materialized views in the managed schemas.
///
/// Returns one row per view/MV, including the body text from
/// `pg_get_viewdef` in "pretty" mode (one expression per line when true;
/// we use `true` so PG normalises the output consistently).
pub const SELECT_VIEWS_AND_MVS: &str = "
SELECT
  n.nspname                            AS schema_name,
  c.relname                            AS name,
  c.relkind::text                      AS relkind,
  pg_get_viewdef(c.oid, true)          AS body_text,
  coalesce(c.reloptions, '{}'::text[]) AS reloptions,
  owner_role.rolname                   AS owner,
  coalesce(c.relacl::text[], '{}'::text[]) AS acl,
  obj_description(c.oid, 'pg_class')   AS comment
FROM pg_class c
JOIN pg_namespace n ON c.relnamespace = n.oid
JOIN pg_authid owner_role ON owner_role.oid = c.relowner
WHERE c.relkind IN ('v','m')
  AND n.nspname = ANY($1::text[])
  AND NOT EXISTS (
      SELECT 1
      FROM pg_catalog.pg_depend dep
      WHERE dep.classid = 'pg_catalog.pg_class'::regclass
        AND dep.objid = c.oid
        AND dep.deptype = 'e'
  )
ORDER BY n.nspname, c.relname
";

/// Query for columns belonging to views and materialized views.
///
/// Returns one row per non-dropped, user-visible column, together with the
/// canonical Postgres type string from `format_type` and an optional
/// `COMMENT ON COLUMN` text. Ordered by (schema, view, attnum) so that
/// columns arrive in declaration order.
pub const SELECT_VIEW_COLUMNS: &str = "
SELECT
  n.nspname                                     AS schema_name,
  c.relname                                     AS view_name,
  a.attnum                                      AS attnum,
  a.attname                                     AS column_name,
  format_type(a.atttypid, a.atttypmod)          AS column_type,
  d.description                                 AS column_comment,
  coalesce(a.attacl::text[], '{}'::text[])      AS attacl
FROM pg_class c
JOIN pg_namespace n  ON c.relnamespace = n.oid
JOIN pg_attribute a  ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
LEFT JOIN pg_description d ON d.objoid = c.oid AND d.objsubid = a.attnum
WHERE c.relkind IN ('v','m')
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, c.relname, a.attnum
";
