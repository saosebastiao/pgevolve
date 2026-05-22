//! `pg_policies` query — row-level security policies.

/// Query `pg_policies` scoped to the managed schemas.
///
/// `permissive` is cast to `bool` using the literal comparison
/// `(p.permissive = 'PERMISSIVE')::bool` — the underlying `pg_policies` view
/// exposes a `text` column with values `'PERMISSIVE'` / `'RESTRICTIVE'`.
///
/// `roles` is cast to `text[]` so the adapter returns a
/// [`crate::catalog::rows::Value::TextArray`]. An empty `TO` clause is stored
/// by Postgres as `{public}`, so `coalesce` is used to normalise NULL to the
/// empty array; in practice PG always stores at least `{public}`.
///
/// `qual` and `with_check` are nullable `pg_node_tree` columns — `pg_policies`
/// already casts them to `text` for us.
///
/// Takes `$1::text[]` (managed-schema list);
/// [`crate::catalog::CatalogQuery::takes_text_array_param`] returns `true`.
pub const POLICIES_QUERY: &str = r"
SELECT p.schemaname,
       p.tablename,
       p.policyname,
       (p.permissive = 'PERMISSIVE')::bool AS permissive,
       p.cmd,
       coalesce(p.roles::text[], '{}'::text[]) AS roles,
       p.qual::text       AS using_text,
       p.with_check::text AS with_check_text
FROM pg_policies p
WHERE p.schemaname = ANY($1::text[])
ORDER BY p.schemaname, p.tablename, p.policyname
";
