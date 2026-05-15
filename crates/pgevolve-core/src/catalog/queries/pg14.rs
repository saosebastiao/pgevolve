//! PG 14-only query overrides.
//!
//! `pg_index.indnullsnotdistinct` was added in PG 15. For PG 14, the column
//! is omitted from the SELECT list and the IR field defaults to `false` at
//! assembly time.

/// Indexes for PG 14 — same as the shared query but without `indnullsnotdistinct`.
pub const INDEXES_QUERY: &str = r"
SELECT
  c.oid::bigint              AS oid,
  c.relname                  AS name,
  n.nspname                  AS schema,
  tc.relname                 AS table_name,
  tn.nspname                 AS table_schema,
  am.amname                  AS method,
  i.indisunique              AS unique,
  i.indisvalid               AS indisvalid,
  false                      AS nulls_not_distinct,
  i.indkey::int2[]::int8[]   AS column_attnums,
  i.indnatts::bigint         AS total_columns,
  i.indnkeyatts::bigint      AS key_columns,
  pg_catalog.pg_get_indexdef(c.oid, 0, true) AS indexdef,
  d.description              AS comment
FROM pg_catalog.pg_index i
JOIN pg_catalog.pg_class     c  ON c.oid  = i.indexrelid
JOIN pg_catalog.pg_namespace n  ON n.oid  = c.relnamespace
JOIN pg_catalog.pg_class     tc ON tc.oid = i.indrelid
JOIN pg_catalog.pg_namespace tn ON tn.oid = tc.relnamespace
JOIN pg_catalog.pg_am        am ON am.oid = c.relam
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = c.oid
 AND d.classoid = 'pg_catalog.pg_class'::regclass
 AND d.objsubid = 0
WHERE n.nspname = ANY($1::text[])
  AND NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_constraint cc
    WHERE cc.conindid = i.indexrelid
  )
ORDER BY n.nspname, c.relname
";
