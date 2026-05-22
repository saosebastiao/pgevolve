//! `GRANT role TO target` — cluster-level role membership.

use pg_query::protobuf::GrantRoleStmt;

use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::parse::error::{ParseError, SourceLocation};

pub(super) fn apply(
    s: &GrantRoleStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    if !s.is_grant {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "REVOKE role FROM target in source is not supported — \
                       revocations happen via diff"
                .into(),
        });
    }
    let parents = extract_role_specs(&s.granted_roles, loc, "granted role")?;
    let members = extract_role_specs(&s.grantee_roles, loc, "grantee role")?;
    for member_name in &members {
        let member_role = cat
            .roles
            .iter_mut()
            .find(|r| &r.name == member_name)
            .ok_or_else(|| ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "GRANT ... TO {member_name} — unknown role; \
                     declare with CREATE ROLE first"
                ),
            })?;
        for parent_name in &parents {
            if !member_role.member_of.contains(parent_name) {
                member_role.member_of.push(parent_name.clone());
            }
        }
    }
    Ok(())
}

fn extract_role_specs(
    nodes: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
    label: &str,
) -> Result<Vec<Identifier>, ParseError> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let role_name_str = match n.node.as_ref() {
            Some(pg_query::NodeEnum::RoleSpec(rs)) => rs.rolename.clone(),
            Some(pg_query::NodeEnum::AccessPriv(ap)) => ap.priv_name.clone(),
            other => {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: format!("expected {label}, got {other:?}"),
                });
            }
        };
        out.push(Identifier::from_unquoted(&role_name_str).map_err(|e| {
            ParseError::Structural {
                location: loc.clone(),
                message: format!("invalid {label} {role_name_str:?}: {e}"),
            }
        })?);
    }
    Ok(out)
}
