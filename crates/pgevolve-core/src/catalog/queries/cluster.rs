//! SQL constants for cluster-catalog queries.
//!
//! Both queries use `$1::text[]` as their parameter, but the semantics differ
//! from per-DB queries: here the array is a bootstrap-roles filter list (role
//! names to exclude), not a managed-schemas list. Both are dispatched through
//! the same [`CatalogQuery::takes_text_array_param`] mechanism.
//!
//! The [`crate::catalog::cluster::read_cluster_catalog`] caller is responsible
//! for passing the bootstrap-roles slice; per-DB callers pass the
//! managed-schemas slice; the trait does not police which is which.
//!
//! [`CatalogQuery::takes_text_array_param`]: crate::catalog::CatalogQuery::takes_text_array_param

/// Query `pg_authid` (joined to `pg_shdescription` for comments) and return
/// one row per managed role.
///
/// Predefined `pg_*` roles and any role named in `$1::text[]` are excluded.
/// `rolconnlimit` is cast to `bigint` so the adapter decodes it as
/// [`crate::catalog::rows::Value::Integer`]; `-1` means unlimited.
/// `rolvaliduntil` is formatted as an RFC 3339 UTC string (or `NULL` when not
/// set).
pub const CLUSTER_ROLES_QUERY: &str = r#"
SELECT r.rolname,
       r.rolsuper,
       r.rolcreatedb,
       r.rolcreaterole,
       r.rolinherit,
       r.rolcanlogin,
       r.rolreplication,
       r.rolbypassrls,
       r.rolconnlimit::bigint AS rolconnlimit,
       to_char(r.rolvaliduntil AT TIME ZONE 'UTC',
               'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS valid_until,
       d.description AS comment
FROM pg_authid r
LEFT JOIN pg_shdescription d
  ON d.objoid = r.oid
 AND d.classoid = 'pg_authid'::regclass
WHERE r.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND r.rolname <> ALL($1::text[])
ORDER BY r.rolname
"#;

/// Query `pg_tablespace` (joined to `pg_shdescription` for comments) and return
/// one row per managed tablespace.
///
/// The built-in `pg_default` / `pg_global` tablespaces and any tablespace named
/// in `$1::text[]` (the bootstrap filter) are excluded. `location` comes from
/// `pg_tablespace_location(oid)` (the removed-in-PG-9.2 `spclocation` column is
/// never used); `owner` from `pg_get_userbyid(spcowner)`; `options` from the
/// `spcoptions` `text[]` of `key=value` entries (`NULL` when none).
pub const CLUSTER_TABLESPACES_QUERY: &str = r"
SELECT t.spcname                                AS name,
       pg_get_userbyid(t.spcowner)              AS owner,
       pg_tablespace_location(t.oid)            AS location,
       t.spcoptions                             AS options,
       d.description                            AS comment
FROM pg_tablespace t
LEFT JOIN pg_shdescription d
  ON d.objoid = t.oid
 AND d.classoid = 'pg_tablespace'::regclass
WHERE t.spcname NOT IN ('pg_default', 'pg_global')
  AND t.spcname <> ALL($1::text[])
ORDER BY t.spcname
";

/// Query `pg_auth_members` joined twice to `pg_authid` to resolve oids to names.
///
/// Returns one row per (member, parent) edge where both sides are non-predefined
/// and non-bootstrap. The same `$1::text[]` bootstrap filter applies to both
/// member and parent sides of the edge.
pub const CLUSTER_MEMBERS_QUERY: &str = r"
SELECT memb.rolname   AS member,
       parent.rolname AS member_of
FROM pg_auth_members am
JOIN pg_authid memb   ON memb.oid   = am.member
JOIN pg_authid parent ON parent.oid = am.roleid
WHERE memb.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND parent.rolname NOT LIKE 'pg\_%' ESCAPE '\'
  AND memb.rolname <> ALL($1::text[])
  AND parent.rolname <> ALL($1::text[])
";
