//! Reads partitioned-table parents (`pg_class.relkind = 'p'`).
//!
//! Filters:
//! - `relkind = 'p'` — partitioned-table parents only.
//! - `pg_depend deptype='e'` — excludes extension-owned tables.
//! - `n.nspname = ANY($1)` — scopes to managed schemas.

/// SQL query for partitioned-table parents.
pub const SELECT_PARTITIONED_TABLES: &str = r"
SELECT
    n.nspname                    AS schema_name,
    c.relname                    AS table_name,
    pg_get_partkeydef(c.oid)     AS partkey_def
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind = 'p'
  AND n.nspname = ANY($1::text[])
  AND NOT EXISTS (
      SELECT 1 FROM pg_depend d
      WHERE d.objid = c.oid AND d.deptype = 'e'
  )
ORDER BY n.nspname, c.relname
";
