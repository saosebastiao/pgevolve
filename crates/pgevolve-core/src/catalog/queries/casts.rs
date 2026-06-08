//! `pg_cast` catalog query.
//!
//! Returns one row per user-defined cast (`c.oid >= 16384`), excluding
//! extension-owned casts (`pg_depend.deptype = 'e'`). Casts are
//! database-global (not schema-scoped); the query takes **no** `$1::text[]`
//! parameter ([`crate::catalog::CatalogQuery::takes_text_array_param`] returns
//! `false` for [`crate::catalog::CatalogQuery::Casts`]).
//!
//! The `pg_get_function_identity_arguments` string for the cast function is
//! decoded by the assembler through the same
//! [`crate::ir::column_type::ColumnType`] path the `CREATE CAST` parser uses,
//! so source-side and catalog-side `arg_types` compare equal.

/// SQL query that fetches all user-defined casts in the database.
///
/// Extension-owned casts (`pg_depend.deptype = 'e'`) are excluded at the SQL
/// layer. System / built-in casts (`c.oid < 16384`) are excluded via the
/// `c.oid >= 16384` predicate. Takes **no** `$1::text[]` parameter.
pub const SELECT_CASTS: &str = "\
SELECT \
    ns.nspname::text                                        AS source_schema, \
    ts.typname::text                                        AS source_name, \
    nt.nspname::text                                        AS target_schema, \
    tt.typname::text                                        AS target_name, \
    c.castmethod::text                                      AS castmethod, \
    c.castcontext::text                                     AS castcontext, \
    np.nspname::text                                        AS func_schema, \
    p.proname::text                                         AS func_name, \
    pg_get_function_identity_arguments(c.castfunc)::text    AS func_arg_signature, \
    l.lanname::text                                         AS func_lang, \
    coalesce(obj_description(c.oid, 'pg_cast'), '')         AS comment \
FROM pg_catalog.pg_cast c \
JOIN pg_catalog.pg_type      ts ON ts.oid = c.castsource \
JOIN pg_catalog.pg_namespace ns ON ns.oid = ts.typnamespace \
JOIN pg_catalog.pg_type      tt ON tt.oid = c.casttarget \
JOIN pg_catalog.pg_namespace nt ON nt.oid = tt.typnamespace \
LEFT JOIN pg_catalog.pg_proc      p  ON p.oid  = NULLIF(c.castfunc, 0) \
LEFT JOIN pg_catalog.pg_namespace np ON np.oid = p.pronamespace \
LEFT JOIN pg_catalog.pg_language  l  ON l.oid  = p.prolang \
WHERE c.oid >= 16384 \
  AND NOT EXISTS ( \
      SELECT 1 \
      FROM pg_catalog.pg_depend dep \
      WHERE dep.classid = 'pg_catalog.pg_cast'::regclass \
        AND dep.objid   = c.oid \
        AND dep.deptype = 'e' \
  ) \
ORDER BY ns.nspname, ts.typname, nt.nspname, tt.typname";
