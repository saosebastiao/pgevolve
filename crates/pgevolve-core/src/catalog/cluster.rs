//! Cluster catalog reader. Queries `pg_authid` + `pg_auth_members`.
//!
//! [`read_cluster_catalog`] is the top-level entry point. It issues two
//! queries — one for role attributes (including an optional comment from
//! `pg_shdescription`) and one for membership edges — filters predefined
//! `pg_*` roles and caller-supplied bootstrap roles, then returns a
//! canonicalized [`ClusterCatalog`].

use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::catalog::{CatalogQuerier, CatalogQuery};
use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::{Role, RoleAttributes};

/// Read the full cluster catalog from a live Postgres instance.
///
/// `querier` must be connected with a superuser DSN (required to read
/// `pg_authid`). `bootstrap_roles` are role names (e.g., `["postgres"]`) that
/// pgevolve treats as PG-owned and never diffs; they are filtered from both
/// the role list and all membership edges.
///
/// Returns a canonicalized [`ClusterCatalog`] — roles sorted by name, each
/// role's `member_of` list sorted lexicographically.
///
/// # Errors
/// Returns [`CatalogError`] on query failure or unexpected column types.
pub fn read_cluster_catalog(
    querier: &dyn CatalogQuerier,
    bootstrap_roles: &[String],
) -> Result<ClusterCatalog, CatalogError> {
    let bootstrap: Vec<&str> = bootstrap_roles.iter().map(String::as_str).collect();

    let roles_rows = querier.fetch(CatalogQuery::ClusterRoles, &bootstrap)?;
    let mut roles: Vec<Role> = roles_rows
        .iter()
        .map(decode_role)
        .collect::<Result<_, _>>()?;

    let member_rows = querier.fetch(CatalogQuery::ClusterMembers, &bootstrap)?;
    for row in &member_rows {
        append_membership_edge(&mut roles, row)?;
    }

    let mut cat = ClusterCatalog {
        roles,
        tablespaces: vec![],
    };
    cat.canonicalize()?;
    Ok(cat)
}

/// Decode a single `pg_authid` row (with optional `pg_shdescription` columns)
/// into a [`Role`].
fn decode_role(row: &Row) -> Result<Role, CatalogError> {
    let q = CatalogQuery::ClusterRoles;

    let name_str = row.get_text(q, "rolname")?;
    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "rolname".to_string(),
        message: format!("invalid role name {name_str:?}: {e}"),
    })?;

    // `rolconnlimit` is cast to bigint in the SQL; -1 means unlimited.
    let connection_limit = match row.get_int(q, "rolconnlimit")? {
        -1 => None,
        n => Some(n),
    };

    Ok(Role {
        name,
        attributes: RoleAttributes {
            superuser: row.get_bool(q, "rolsuper")?,
            createdb: row.get_bool(q, "rolcreatedb")?,
            createrole: row.get_bool(q, "rolcreaterole")?,
            inherit: row.get_bool(q, "rolinherit")?,
            login: row.get_bool(q, "rolcanlogin")?,
            replication: row.get_bool(q, "rolreplication")?,
            bypass_rls: row.get_bool(q, "rolbypassrls")?,
            connection_limit,
            valid_until: row.get_opt_text(q, "valid_until")?,
        },
        member_of: Vec::new(),
        comment: row.get_opt_text(q, "comment")?,
    })
}

/// Parse a `pg_auth_members` row and push the parent identifier onto the
/// matching role's `member_of` list.
///
/// If the member role has already been filtered (predefined or bootstrap) it
/// will not appear in `roles`; the edge is silently skipped in that case.
fn append_membership_edge(roles: &mut [Role], row: &Row) -> Result<(), CatalogError> {
    let q = CatalogQuery::ClusterMembers;

    let member_str = row.get_text(q, "member")?;
    let parent_str = row.get_text(q, "member_of")?;

    let member_id =
        Identifier::from_unquoted(&member_str).map_err(|e| CatalogError::BadColumnType {
            query: q,
            column: "member".to_string(),
            message: format!("invalid role name {member_str:?}: {e}"),
        })?;
    let parent_id =
        Identifier::from_unquoted(&parent_str).map_err(|e| CatalogError::BadColumnType {
            query: q,
            column: "member_of".to_string(),
            message: format!("invalid role name {parent_str:?}: {e}"),
        })?;

    if let Some(role) = roles.iter_mut().find(|r| r.name == member_id) {
        role.member_of.push(parent_id);
    }
    // If the member role was filtered (predefined/bootstrap), skip silently.

    Ok(())
}
