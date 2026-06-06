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
use crate::ir::cluster::tablespace::Tablespace;
use std::collections::BTreeMap;

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

    let tablespace_rows = querier.fetch(CatalogQuery::ClusterTablespaces, &bootstrap)?;
    let tablespaces: Vec<Tablespace> = tablespace_rows
        .iter()
        .map(decode_tablespace)
        .collect::<Result<_, _>>()?;

    let mut cat = ClusterCatalog { roles, tablespaces };
    cat.canonicalize()?;
    Ok(cat)
}

/// Decode a single `pg_tablespace` row (with optional `pg_shdescription`
/// columns) into a [`Tablespace`].
///
/// Options arrive as a `text[]` of `key=value` entries (`NULL` when the
/// tablespace has none); each entry is split on its **first** `'='` so a value
/// may itself contain `'='`. An empty `owner` / `comment` is treated as absent.
fn decode_tablespace(row: &Row) -> Result<Tablespace, CatalogError> {
    let q = CatalogQuery::ClusterTablespaces;

    let name_str = row.get_text(q, "name")?;
    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: q,
        column: "name".to_string(),
        message: format!("invalid tablespace name {name_str:?}: {e}"),
    })?;

    let owner = match row.get_opt_text(q, "owner")? {
        Some(s) if !s.is_empty() => {
            let id = Identifier::from_unquoted(&s).map_err(|e| CatalogError::BadColumnType {
                query: q,
                column: "owner".to_string(),
                message: format!("invalid owner name {s:?}: {e}"),
            })?;
            Some(id)
        }
        _ => None,
    };

    let location = row.get_text(q, "location")?;

    let mut options = BTreeMap::new();
    if !row.is_null("options") {
        for entry in row.get_text_array(q, "options")? {
            if let Some((key, value)) = entry.split_once('=') {
                options.insert(key.to_string(), value.to_string());
            }
        }
    }

    let comment = match row.get_opt_text(q, "comment")? {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    };

    Ok(Tablespace {
        name,
        location,
        owner,
        options,
        comment,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).expect("valid identifier")
    }

    #[test]
    fn decode_tablespace_simple() {
        let row = Row::new()
            .with("name", Value::Text("fast_ssd".into()))
            .with("owner", Value::Text("postgres".into()))
            .with("location", Value::Text("/x".into()))
            .with("options", Value::Null)
            .with("comment", Value::Null);

        let ts = decode_tablespace(&row).expect("decodes");
        assert_eq!(ts.name, id("fast_ssd"));
        assert_eq!(ts.owner, Some(id("postgres")));
        assert_eq!(ts.location, "/x");
        assert!(ts.options.is_empty());
        assert_eq!(ts.comment, None);
    }

    #[test]
    fn decode_tablespace_with_options() {
        let row = Row::new()
            .with("name", Value::Text("ts".into()))
            .with("owner", Value::Text("postgres".into()))
            .with("location", Value::Text("/x".into()))
            .with(
                "options",
                Value::TextArray(vec![
                    "seq_page_cost=2.0".into(),
                    "random_page_cost=3".into(),
                ]),
            )
            .with("comment", Value::Null);

        let ts = decode_tablespace(&row).expect("decodes");
        assert_eq!(
            ts.options.get("seq_page_cost").map(String::as_str),
            Some("2.0")
        );
        assert_eq!(
            ts.options.get("random_page_cost").map(String::as_str),
            Some("3")
        );
    }

    #[test]
    fn decode_tablespace_value_containing_equals() {
        let row = Row::new()
            .with("name", Value::Text("ts".into()))
            .with("owner", Value::Text("postgres".into()))
            .with("location", Value::Text("/x".into()))
            .with("options", Value::TextArray(vec!["k=a=b".into()]))
            .with("comment", Value::Null);

        let ts = decode_tablespace(&row).expect("decodes");
        assert_eq!(ts.options.get("k").map(String::as_str), Some("a=b"));
    }

    #[test]
    fn decode_tablespace_null_options_and_comment() {
        let row = Row::new()
            .with("name", Value::Text("ts".into()))
            .with("owner", Value::Text("postgres".into()))
            .with("location", Value::Text("/x".into()))
            .with("options", Value::Null)
            .with("comment", Value::Null);

        let ts = decode_tablespace(&row).expect("decodes");
        assert!(ts.options.is_empty());
        assert_eq!(ts.comment, None);
    }

    #[test]
    fn decode_tablespace_empty_owner_is_none() {
        let row = Row::new()
            .with("name", Value::Text("ts".into()))
            .with("owner", Value::Text(String::new()))
            .with("location", Value::Text("/x".into()))
            .with("options", Value::Null)
            .with("comment", Value::Null);

        let ts = decode_tablespace(&row).expect("decodes");
        assert_eq!(ts.owner, None);
    }
}
