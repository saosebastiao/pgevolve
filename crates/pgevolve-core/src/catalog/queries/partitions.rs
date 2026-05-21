//! Reads child partitions (`pg_class.relispartition = true`).
//!
//! Filters:
//! - `relispartition = true` — child partitions only.
//! - `pg_depend deptype='e'` — excludes extension-owned tables.
//! - `n.nspname = ANY($1)` — scopes to managed schemas.

/// SQL query for child partitions.
pub const SELECT_PARTITIONS: &str = r"
SELECT
    n.nspname                          AS schema_name,
    c.relname                          AS table_name,
    parent_n.nspname                   AS parent_schema,
    parent_c.relname                   AS parent_name,
    pg_get_expr(c.relpartbound, c.oid) AS partbound_def
FROM pg_class c
JOIN pg_namespace n        ON n.oid = c.relnamespace
JOIN pg_inherits i         ON i.inhrelid = c.oid
JOIN pg_class parent_c     ON parent_c.oid = i.inhparent
JOIN pg_namespace parent_n ON parent_n.oid = parent_c.relnamespace
WHERE c.relispartition = true
  AND n.nspname = ANY($1::text[])
  AND NOT EXISTS (
      SELECT 1 FROM pg_depend d
      WHERE d.objid = c.oid AND d.deptype = 'e'
  )
ORDER BY n.nspname, c.relname
";
