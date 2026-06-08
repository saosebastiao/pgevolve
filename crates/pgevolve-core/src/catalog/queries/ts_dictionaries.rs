//! `pg_ts_dict` catalog query.
//!
//! Returns one row per user-defined text-search dictionary in the managed
//! schemas. Each row joins `pg_ts_dict` to its `pg_namespace` (schema),
//! `pg_ts_template` (template reference), and the template's `pg_namespace` so
//! that the reader can produce a fully schema-qualified template
//! [`crate::identifier::QualifiedName`].
//!
//! Extension-owned dictionaries (`pg_depend.deptype = 'e'`) are excluded at the
//! SQL layer. The query takes a `$1::text[]` managed-schema list
//! ([`crate::catalog::CatalogQuery::takes_text_array_param`] returns `true` for
//! [`crate::catalog::CatalogQuery::TsDictionaries`]).
//!
//! `dictinitoption` is a `text` blob in PG's canonical `key = 'value'[, …]`
//! format; the assembler parses it into an ordered `Vec<(String, String)>`.

/// SQL query that fetches all user-defined text-search dictionaries in the
/// managed schemas.
///
/// Extension-owned dictionaries (`pg_depend.deptype = 'e'`) are excluded at the
/// SQL layer. Takes a `$1::text[]` managed-schema list.
pub const SELECT_TS_DICTIONARIES: &str = "\
SELECT \
    n.nspname::text                                               AS schema_name, \
    d.dictname::text                                              AS name, \
    tn.nspname::text                                              AS template_schema, \
    t.tmplname::text                                              AS template_name, \
    d.dictinitoption::text                                        AS options, \
    pg_catalog.pg_get_userbyid(d.dictowner)::text                 AS owner, \
    de.description::text                                          AS comment \
FROM pg_catalog.pg_ts_dict d \
JOIN pg_catalog.pg_namespace n   ON n.oid  = d.dictnamespace \
JOIN pg_catalog.pg_ts_template t ON t.oid  = d.dicttemplate \
JOIN pg_catalog.pg_namespace tn  ON tn.oid = t.tmplnamespace \
LEFT JOIN pg_catalog.pg_description de \
       ON de.objoid    = d.oid \
      AND de.classoid  = 'pg_catalog.pg_ts_dict'::regclass \
      AND de.objsubid  = 0 \
WHERE n.nspname = ANY($1::text[]) \
  AND NOT EXISTS ( \
      SELECT 1 \
      FROM pg_catalog.pg_depend dep \
      WHERE dep.classid  = 'pg_catalog.pg_ts_dict'::regclass \
        AND dep.objid    = d.oid \
        AND dep.deptype  = 'e' \
  ) \
ORDER BY n.nspname, d.dictname";
