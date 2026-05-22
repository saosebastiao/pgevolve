//! SQL strings shared across PG 14–17.
//!
//! Per-version differences live in `pg14.rs`/`pg15.rs`/etc. The query strings
//! are designed so that adapters can run them as `query(sql, &[&managed_schemas])`
//! — every query that scopes by schema takes a single `text[]` parameter.
//! The `PgVersion` query takes no parameters.

/// `SHOW server_version_num` as an integer column. Returns one row.
pub const PG_VERSION_QUERY: &str =
    "SELECT current_setting('server_version_num')::bigint AS server_version_num";

/// Schemas (namespaces). Reserved schemas are excluded unconditionally.
pub const SCHEMAS_QUERY: &str = r"
SELECT
  n.oid::bigint AS oid,
  n.nspname     AS name,
  owner_role.rolname AS owner,
  coalesce(n.nspacl::text[], '{}'::text[]) AS acl,
  d.description AS comment
FROM pg_catalog.pg_namespace n
JOIN pg_catalog.pg_authid owner_role ON owner_role.oid = n.nspowner
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = n.oid
 AND d.classoid = 'pg_catalog.pg_namespace'::regclass
 AND d.objsubid = 0
WHERE n.nspname <> ALL (ARRAY['pg_catalog','pg_toast','information_schema','pgevolve'])
  AND n.nspname NOT LIKE 'pg\_temp\_%' ESCAPE '\'
  AND n.nspname NOT LIKE 'pg\_toast\_temp\_%' ESCAPE '\'
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname
";

/// Ordinary tables (relkind='r') and partitioned-parent tables (relkind='p').
/// Child partitions are also ordinary tables (`relkind='r'` with
/// `relispartition=true`), so they are included here as well.
pub const TABLES_QUERY: &str = r"
SELECT
  c.oid::bigint AS oid,
  n.nspname     AS schema,
  c.relname     AS name,
  owner_role.rolname AS owner,
  coalesce(c.relacl::text[], '{}'::text[]) AS acl,
  d.description AS comment
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_authid owner_role ON owner_role.oid = c.relowner
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = c.oid
 AND d.classoid = 'pg_catalog.pg_class'::regclass
 AND d.objsubid = 0
WHERE c.relkind IN ('r', 'p')
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

/// Columns. Returns one row per non-dropped, user-visible column.
///
/// `pg_type_string` uses `format_type(atttypid, atttypmod)` which returns the
/// canonical name including typmod, e.g., `varchar(50)`, `numeric(10,2)`,
/// `timestamp(3) with time zone`. That string round-trips through
/// [`crate::ir::column_type::ColumnType::parse_from_pg_type_string`].
pub const COLUMNS_QUERY: &str = r"
SELECT
  c.oid::bigint                                     AS table_oid,
  n.nspname                                         AS schema,
  c.relname                                         AS table_name,
  a.attnum::bigint                                  AS attnum,
  a.attname                                         AS name,
  pg_catalog.format_type(a.atttypid, a.atttypmod)   AS pg_type_string,
  a.attnotnull                                      AS not_null,
  pg_catalog.pg_get_expr(ad.adbin, ad.adrelid)      AS default_expr,
  a.attidentity::text                               AS attidentity,
  a.attgenerated::text                              AS attgenerated,
  coalesce(s.seqstart, 1)::bigint                   AS identity_start,
  coalesce(s.seqincrement, 1)::bigint               AS identity_increment,
  s.seqmin                                          AS identity_min,
  s.seqmax                                          AS identity_max,
  coalesce(s.seqcache, 1)::bigint                   AS identity_cache,
  coalesce(s.seqcycle, false)                       AS identity_cycle,
  coll_n.nspname                                    AS collation_schema,
  coll.collname                                     AS collation_name,
  d.description                                     AS comment,
  a.attstorage::text                                AS attstorage,
  a.attcompression::text                            AS attcompression,
  coalesce(a.attacl::text[], '{}'::text[])          AS attacl
FROM pg_catalog.pg_attribute a
JOIN pg_catalog.pg_class     c  ON c.oid = a.attrelid
JOIN pg_catalog.pg_namespace n  ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_attrdef ad
  ON ad.adrelid = a.attrelid
 AND ad.adnum   = a.attnum
LEFT JOIN pg_catalog.pg_collation coll
  ON coll.oid = a.attcollation
 AND a.attcollation <> 0
LEFT JOIN pg_catalog.pg_namespace coll_n
  ON coll_n.oid = coll.collnamespace
LEFT JOIN pg_catalog.pg_depend dep
  ON dep.refclassid = 'pg_catalog.pg_class'::regclass
 AND dep.refobjid   = a.attrelid
 AND dep.refobjsubid = a.attnum
 AND dep.classid    = 'pg_catalog.pg_class'::regclass
 AND dep.deptype    = 'i'
LEFT JOIN pg_catalog.pg_sequence s
  ON s.seqrelid = dep.objid
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = a.attrelid
 AND d.classoid = 'pg_catalog.pg_class'::regclass
 AND d.objsubid = a.attnum
WHERE c.relkind IN ('r', 'p')
  AND a.attnum > 0
  AND NOT a.attisdropped
  AND n.nspname = ANY($1::text[])
ORDER BY n.nspname, c.relname, a.attnum
";

/// Constraints. Includes PK, UNIQUE, FK, CHECK only (`contype IN ('p','u','f','c')`).
/// Exclusion constraints are out of v0.1 scope.
pub const CONSTRAINTS_QUERY: &str = r"
SELECT
  c.oid::bigint              AS oid,
  c.conname                  AS name,
  cn.nspname                 AS schema,
  cl.relname                 AS table_name,
  cln.nspname                AS table_schema,
  c.contype::text            AS contype,
  c.condeferrable            AS deferrable,
  c.condeferred              AS deferred,
  c.conkey                   AS conkey,
  c.confkey                  AS confkey,
  fcl.relname                AS fk_table,
  fcln.nspname               AS fk_schema,
  c.confupdtype::text        AS on_update,
  c.confdeltype::text        AS on_delete,
  c.confmatchtype::text      AS match_type,
  c.connoinherit             AS no_inherit,
  c.conindid::bigint         AS conindid,
  c.convalidated             AS convalidated,
  pg_catalog.pg_get_constraintdef(c.oid, true) AS constraint_def,
  d.description              AS comment
FROM pg_catalog.pg_constraint c
JOIN pg_catalog.pg_namespace cn  ON cn.oid  = c.connamespace
JOIN pg_catalog.pg_class     cl  ON cl.oid  = c.conrelid
JOIN pg_catalog.pg_namespace cln ON cln.oid = cl.relnamespace
LEFT JOIN pg_catalog.pg_class     fcl  ON fcl.oid  = c.confrelid
LEFT JOIN pg_catalog.pg_namespace fcln ON fcln.oid = fcl.relnamespace
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = c.oid
 AND d.classoid = 'pg_catalog.pg_constraint'::regclass
WHERE c.contype IN ('p','u','f','c')
  AND cln.nspname = ANY($1::text[])
ORDER BY cln.nspname, cl.relname, c.conname
";

/// Indexes (PG 15+). Excludes constraint-backing indexes.
///
/// Includes `indnullsnotdistinct` and indexes on materialized views
/// (`tc.relkind = 'm'`) as well as ordinary tables.
pub const INDEXES_QUERY: &str = r"
SELECT
  c.oid::bigint              AS oid,
  c.relname                  AS name,
  n.nspname                  AS schema,
  tc.relname                 AS table_name,
  tn.nspname                 AS table_schema,
  tc.relkind::text           AS parent_relkind,
  am.amname                  AS method,
  i.indisunique              AS unique,
  i.indisvalid               AS indisvalid,
  i.indnullsnotdistinct      AS nulls_not_distinct,
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
  AND tc.relkind IN ('r','m')
  AND NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_constraint cc
    WHERE cc.conindid = i.indexrelid
  )
  AND NOT EXISTS (
      SELECT 1
      FROM pg_catalog.pg_depend dep
      WHERE dep.classid = 'pg_catalog.pg_class'::regclass
        AND dep.objid = c.oid
        AND dep.deptype = 'e'
  )
ORDER BY n.nspname, c.relname
";

/// Sequences. Returns the per-sequence options from `pg_sequence`.
pub const SEQUENCES_QUERY: &str = r"
SELECT
  c.oid::bigint     AS oid,
  c.relname         AS name,
  n.nspname         AS schema,
  owner_role.rolname AS owner,
  coalesce(c.relacl::text[], '{}'::text[]) AS acl,
  pg_catalog.format_type(s.seqtypid, NULL) AS data_type_string,
  s.seqstart::bigint     AS start,
  s.seqincrement::bigint AS increment,
  s.seqmin::bigint  AS min_value,
  s.seqmax::bigint  AS max_value,
  s.seqcache::bigint AS cache,
  s.seqcycle        AS cycle,
  d.description     AS comment
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_authid owner_role ON owner_role.oid = c.relowner
JOIN pg_catalog.pg_sequence  s ON s.seqrelid = c.oid
LEFT JOIN pg_catalog.pg_description d
  ON d.objoid = c.oid
 AND d.classoid = 'pg_catalog.pg_class'::regclass
 AND d.objsubid = 0
WHERE c.relkind = 'S'
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

/// Comments query placeholder.
///
/// Currently inlined into per-object queries; this query is a placeholder for
/// future use (e.g., bulk re-fetch of comments). Returns the (rare) comments
/// not picked up by other queries: schema, table, sequence, index, constraint.
pub const COMMENTS_QUERY: &str = "SELECT NULL::bigint AS objoid, NULL::text AS classname, NULL::int4 AS objsubid, NULL::text AS description WHERE false";

/// `pg_depend` rows linking sequences to their owning columns.
/// `deptype = 'a'` is automatic ownership (`SERIAL` / `IDENTITY`).
pub const DEPENDENCIES_QUERY: &str = r"
SELECT
  c.relname        AS sequence_name,
  cn.nspname       AS sequence_schema,
  refclass.relname AS owner_table,
  refn.nspname     AS owner_schema,
  a.attname        AS owner_column
FROM pg_catalog.pg_depend dep
JOIN pg_catalog.pg_class     c  ON c.oid  = dep.objid
JOIN pg_catalog.pg_namespace cn ON cn.oid = c.relnamespace
JOIN pg_catalog.pg_class     refclass ON refclass.oid = dep.refobjid
JOIN pg_catalog.pg_namespace refn     ON refn.oid     = refclass.relnamespace
JOIN pg_catalog.pg_attribute a
  ON a.attrelid = dep.refobjid
 AND a.attnum   = dep.refobjsubid
WHERE c.relkind = 'S'
  AND dep.classid    = 'pg_catalog.pg_class'::regclass
  AND dep.refclassid = 'pg_catalog.pg_class'::regclass
  AND dep.deptype    = 'a'
  AND cn.nspname = ANY($1::text[])
ORDER BY cn.nspname, c.relname
";
