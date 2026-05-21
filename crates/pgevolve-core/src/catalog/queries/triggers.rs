//! Catalog query for `pg_trigger` — one row per user-visible trigger.
//!
//! Filters:
//! - `NOT t.tgisinternal` — excludes PG's auto-generated FK/RI triggers.
//! - `pg_depend deptype='e'` — excludes extension-owned triggers.
//! - `n.nspname = ANY($1)` — scopes to managed schemas.

/// SQL query for `pg_trigger`.
pub const SELECT_TRIGGERS: &str = r"
SELECT
    t.tgname::text                            AS name,
    n.nspname::text                           AS table_schema,
    c.relname::text                           AS table_name,
    pg_catalog.pg_get_triggerdef(t.oid, true) AS triggerdef,
    d.description                             AS comment
FROM pg_catalog.pg_trigger t
JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_description d
    ON d.objoid = t.oid
   AND d.classoid = 'pg_catalog.pg_trigger'::regclass
WHERE NOT t.tgisinternal
  AND n.nspname = ANY($1::text[])
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend dep
      WHERE dep.classid = 'pg_catalog.pg_trigger'::regclass
        AND dep.objid = t.oid
        AND dep.deptype = 'e'
  )
ORDER BY n.nspname, c.relname, t.tgname
";
