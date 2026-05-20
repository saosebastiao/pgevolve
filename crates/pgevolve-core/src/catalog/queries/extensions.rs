//! Catalog query for `pg_extension` — one row per installed extension.

/// Reads every installed extension from `pg_extension`.
///
/// Returns name, schema, version, and optional comment. Not filtered by
/// managed schemas — extensions are cluster-global; pgevolve lists them
/// all and lets the differ decide which to keep.
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
ORDER BY e.extname
";
