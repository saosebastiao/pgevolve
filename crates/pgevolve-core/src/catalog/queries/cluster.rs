//! SQL strings for cluster-wide catalog queries.
//!
//! These queries target `pg_authid` and `pg_auth_members`, which are stable
//! across PG 14–17. Both use `$1::text[]` as the bootstrap-role filter (role
//! names to exclude, e.g. `["postgres"]`), mirroring the schema-list parameter
//! convention used by per-database queries.

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
