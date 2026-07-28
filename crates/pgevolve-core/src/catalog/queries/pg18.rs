//! PG 18-specific query overrides.

/// Constraints, plus the two PG 18-only columns that change what a constraint
/// *means*.
///
/// `conenforced` (false for `NOT ENFORCED`) and `conperiod` (true for temporal
/// `PRIMARY KEY`/`UNIQUE ... WITHOUT OVERLAPS` and `FOREIGN KEY ... PERIOD`)
/// do not exist before PG 18, which is why this is an override rather than a
/// change to [`super::shared::CONSTRAINTS_QUERY`].
///
/// pgevolve cannot yet represent either feature, so the assembler refuses a
/// constraint carrying them rather than reading it as an ordinary enforced,
/// non-temporal constraint. Detecting them from these columns is exact;
/// detecting them from `pg_get_constraintdef` text is not, because a boolean
/// column named `enforced` renders as `CHECK ((NOT enforced))` and a column
/// named `period` is entirely ordinary.
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
  c.conenforced              AS conenforced,
  c.conperiod                AS conperiod,
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
