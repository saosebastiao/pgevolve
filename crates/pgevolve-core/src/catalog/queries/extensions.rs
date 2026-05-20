//! Catalog query for `pg_extension` — one row per installed extension.

/// Reads installed extensions from `pg_extension`.
///
/// Extensions are database-level objects (not schema-level), so we return
/// all user-installed extensions. Built-in/system extensions (e.g. `plpgsql`,
/// `pg_catalog` internals) are excluded by filtering out `pg_catalog` and
/// `information_schema` namespaces. The `$1` parameter is accepted but ignored
/// for consistency with the [`crate::catalog::CatalogQuerier`] interface.
pub const SELECT_EXTENSIONS: &str = r"
SELECT
    e.extname::text         AS name,
    n.nspname::text         AS schema,
    e.extversion::text      AS version,
    d.description           AS comment
FROM pg_catalog.pg_extension e
JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
LEFT JOIN pg_catalog.pg_description d
    ON d.objoid = e.oid
   AND d.classoid = 'pg_catalog.pg_extension'::regclass
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
ORDER BY e.extname
";
