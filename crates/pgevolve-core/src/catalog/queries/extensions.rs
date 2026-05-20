//! Catalog query for `pg_extension` — one row per installed extension.

/// Reads installed extensions from `pg_extension`, scoped to managed schemas.
///
/// Filters by `extnamespace IN ($1)` so that built-in/system extensions
/// (e.g. `plpgsql` in `pg_catalog`) are excluded. Only extensions explicitly
/// installed into a managed schema are returned.
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
WHERE n.nspname = ANY($1::text[])
ORDER BY e.extname
";
