//! PG 14-only query overrides.
//!
//! `pg_index.indnullsnotdistinct` was added in PG 15. For PG 14, the column
//! is omitted from the SELECT list and the IR field defaults to `false` at
//! assembly time.

/// Per-table publication entries for PG 14.
///
/// PG 14 lacks `prqual` (added PG 15) and `prattrs` (added PG 15). Both are
/// returned as NULL so the decoder produces `row_filter = None` and
/// `col_attnums = None` for every row, which maps to "publish all columns,
/// no row filter".
pub const PUBLICATION_REL_QUERY_PG14: &str = "\
    SELECT \
        pr.prpubid::bigint AS pub_oid, \
        ns.nspname::text AS schema, \
        c.relname::text AS table_name, \
        NULL::text AS row_filter, \
        NULL::int8[] AS col_attnums, \
        c.oid::bigint AS rel_oid \
    FROM pg_publication_rel pr \
    JOIN pg_class c ON c.oid = pr.prrelid \
    JOIN pg_namespace ns ON ns.oid = c.relnamespace \
    ORDER BY pr.prpubid, ns.nspname, c.relname";

/// PG 14 has no `pg_publication_namespace` (added PG 15). Returns no rows.
pub const PUBLICATION_NAMESPACE_QUERY_PG14: &str =
    "SELECT NULL::bigint AS pub_oid, NULL::text AS schema WHERE false";

/// PG 14 publication attributes query.
///
/// PG 14 lacks `prattrs` in `pg_publication_rel`; since no column list is
/// ever present, this returns an empty result — the assembler will never
/// find any attnums to resolve.
pub const PUBLICATION_ATTRIBUTES_QUERY_PG14: &str =
    "SELECT NULL::bigint AS rel_oid, NULL::bigint AS attnum, NULL::text AS attname WHERE false";

/// Subscriptions for PG 14.
///
/// PG 14 lacks:
///   - `subtwophasestate`  (PG 15+)
///   - `subdisableonerr`   (PG 15+)
///   - `subpasswordrequired`, `subrunasowner`, `suborigin` (PG 16+)
///   - `subfailover`       (PG 17+)
///
/// `substream` is `bool` in PG 14; the `::text` cast returns `'f'` or `'t'`
/// which the decoder handles identically to the PG 16+ text column.
///
/// `two_phase_state` is returned as NULL so the IR field is always `None`
/// when reading from a PG 14 instance (`two_phase` was added in PG 15).
pub const SUBSCRIPTIONS_QUERY_PG14: &str = "\
    SELECT \
        s.oid::bigint AS oid, \
        s.subname::text AS name, \
        coalesce(a.rolname, '') AS owner, \
        s.subenabled AS enabled, \
        s.subconninfo::text AS connection, \
        coalesce(s.subslotname::text, '') AS slot_name, \
        s.subsynccommit::text AS synchronous_commit, \
        s.subpublications::text[] AS publications, \
        s.subbinary AS binary, \
        s.substream::text AS streaming, \
        NULL::text AS two_phase_state, \
        NULL::bool AS disable_on_error, \
        NULL::bool AS password_required, \
        NULL::bool AS run_as_owner, \
        NULL::text AS origin, \
        NULL::bool AS failover, \
        coalesce(d.description, '') AS comment \
    FROM pg_subscription s \
    JOIN pg_authid a ON a.oid = s.subowner \
    LEFT JOIN pg_description d \
        ON d.classoid = 'pg_subscription'::regclass AND d.objoid = s.oid AND d.objsubid = 0 \
    ORDER BY s.subname";

/// Indexes for PG 14 — same as the shared query but without `indnullsnotdistinct`.
/// Includes indexes on materialized views (`tc.relkind = 'm'`) as well as tables.
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
  false                      AS nulls_not_distinct,
  i.indkey::int2[]::int8[]   AS column_attnums,
  i.indnatts::bigint         AS total_columns,
  i.indnkeyatts::bigint      AS key_columns,
  pg_catalog.pg_get_indexdef(c.oid, 0, true) AS indexdef,
  coalesce(c.reloptions, '{}'::text[])       AS reloptions,
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
