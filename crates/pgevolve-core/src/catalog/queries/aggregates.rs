//! User-defined aggregate catalog query.
//!
//! One row per ordinary aggregate (`pg_aggregate` joined to its wrapper
//! `pg_proc` entry via `aggfnoid`). The assembler ([`crate::catalog::assemble`])
//! skips ordered-set / hypothetical-set aggregates (`aggkind <> 'n'`) and any
//! aggregate whose state or final function is written in a language pgevolve
//! does not manage (anything other than `sql` / `plpgsql`), recording those in
//! [`crate::catalog::DriftReport::unmanaged_aggregates`].
//!
//! Argument types are resolved by re-parsing the
//! `pg_get_function_identity_arguments` signature through `pg_query` (the same
//! AST → [`crate::ir::column_type::ColumnType`] path the source-side
//! `CREATE AGGREGATE` parser uses), so the catalog-side and source-side
//! `arg_types` compare equal. The state type arrives as a `format_type` string
//! and is parsed via `ColumnType::parse_from_pg_type_string`.

/// SQL query that fetches all user-defined aggregates in the managed schemas.
///
/// Schema-filtered via `$1::text[]` (the same param group as
/// [`super::functions::SELECT_FUNCTIONS`]). Extension-owned aggregates
/// (`pg_depend.deptype = 'e'`) are excluded at the SQL layer.
pub const SELECT_AGGREGATES: &str = "\
SELECT \
    pn.nspname                                  AS schema_name, \
    p.proname                                   AS name, \
    pg_get_function_identity_arguments(p.oid)   AS arg_signature, \
    a.aggkind::text                             AS aggkind, \
    sfn.nspname                                 AS sfunc_schema, \
    sf.proname                                  AS sfunc_name, \
    sl.lanname                                  AS sfunc_lang, \
    pg_catalog.format_type(a.aggtranstype, NULL) AS state_type, \
    ffn.nspname                                 AS finalfunc_schema, \
    ff.proname                                  AS finalfunc_name, \
    fl.lanname                                  AS finalfunc_lang, \
    a.agginitval                                AS initcond, \
    owner_role.rolname                          AS owner, \
    obj_description(p.oid, 'pg_proc')           AS comment \
FROM pg_catalog.pg_aggregate a \
JOIN pg_catalog.pg_proc p ON p.oid = a.aggfnoid \
JOIN pg_catalog.pg_namespace pn ON pn.oid = p.pronamespace \
JOIN pg_catalog.pg_proc sf ON sf.oid = a.aggtransfn \
JOIN pg_catalog.pg_namespace sfn ON sfn.oid = sf.pronamespace \
JOIN pg_catalog.pg_language sl ON sl.oid = sf.prolang \
JOIN pg_catalog.pg_authid owner_role ON owner_role.oid = p.proowner \
LEFT JOIN pg_catalog.pg_proc ff ON ff.oid = NULLIF(a.aggfinalfn, 0) \
LEFT JOIN pg_catalog.pg_namespace ffn ON ffn.oid = ff.pronamespace \
LEFT JOIN pg_catalog.pg_language fl ON fl.oid = ff.prolang \
WHERE pn.nspname = ANY($1::text[]) \
  AND NOT EXISTS ( \
      SELECT 1 \
      FROM pg_catalog.pg_depend dep \
      WHERE dep.classid = 'pg_catalog.pg_proc'::regclass \
        AND dep.objid = p.oid \
        AND dep.deptype = 'e' \
  ) \
ORDER BY pn.nspname, p.proname, pg_get_function_identity_arguments(p.oid)";
