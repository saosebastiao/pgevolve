//! Function + procedure catalog query.
//!
//! One row per routine (`pg_proc.prokind` IN 'f','p'). The assembler dispatches
//! by prokind. Aggregates and window functions are filtered out at the SQL
//! level — they're explicitly out of v0.2 scope.

/// SQL query that fetches all user-defined functions and procedures from
/// `pg_proc`. Aggregates and window functions are excluded.
pub const SELECT_FUNCTIONS: &str = "\
SELECT \
    n.nspname                                   AS schema_name, \
    p.proname                                   AS name, \
    p.prokind::text                             AS kind, \
    pg_get_function_identity_arguments(p.oid)   AS arg_signature, \
    pg_get_function_arguments(p.oid)            AS arg_full, \
    pg_get_function_result(p.oid)               AS return_type, \
    l.lanname                                   AS language, \
    p.provolatile::text                         AS volatility, \
    p.proisstrict                               AS strict, \
    p.prosecdef                                 AS security_definer, \
    p.proparallel::text                         AS parallel, \
    p.proleakproof                              AS leakproof, \
    p.procost::text                             AS cost, \
    p.prorows::text                             AS rows, \
    pg_get_functiondef(p.oid)                   AS full_def, \
    obj_description(p.oid, 'pg_proc')           AS comment \
FROM pg_proc p \
JOIN pg_namespace n ON p.pronamespace = n.oid \
JOIN pg_language l ON p.prolang = l.oid \
WHERE n.nspname = ANY($1::text[]) \
  AND p.prokind IN ('f', 'p') \
  AND NOT EXISTS ( \
      SELECT 1 \
      FROM pg_catalog.pg_depend dep \
      WHERE dep.classid = 'pg_catalog.pg_proc'::regclass \
        AND dep.objid = p.oid \
        AND dep.deptype = 'e' \
  ) \
ORDER BY n.nspname, p.proname, pg_get_function_identity_arguments(p.oid)";
