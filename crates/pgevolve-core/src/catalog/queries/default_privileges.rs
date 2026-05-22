//! `pg_default_acl` query — ALTER DEFAULT PRIVILEGES rules.

/// Query `pg_default_acl` joined to `pg_authid` for the target role name and
/// (optionally) `pg_namespace` for the schema name.
///
/// `defaclobjtype` is cast to `text` so the adapter returns it as a
/// [`crate::catalog::rows::Value::Text`]; the assembler extracts the first
/// character and decodes it via
/// [`crate::ir::default_privileges::DefaultPrivObjectType::from_pg_char`].
///
/// Rows where `r.rolname` starts with `pg_` are filtered out — these are
/// predefined `PostgreSQL` roles we never manage.
///
/// This query takes **no** `$1::text[]` parameter;
/// [`crate::catalog::CatalogQuery::takes_text_array_param`] returns `false` for
/// [`crate::catalog::CatalogQuery::DefaultPrivileges`].
pub const DEFAULT_PRIVILEGES_QUERY: &str = r"
SELECT r.rolname AS target_role,
       n.nspname AS schema_name,
       d.defaclobjtype::text AS object_type,
       coalesce(d.defaclacl::text[], '{}'::text[]) AS acl
FROM pg_default_acl d
JOIN pg_authid r ON r.oid = d.defaclrole
LEFT JOIN pg_namespace n ON n.oid = d.defaclnamespace
WHERE r.rolname NOT LIKE 'pg\_%' ESCAPE '\'
ORDER BY r.rolname, n.nspname, d.defaclobjtype
";
