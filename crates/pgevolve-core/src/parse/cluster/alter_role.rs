//! `ALTER ROLE r [option…]` — mutates a previously-declared role's attributes.

use pg_query::protobuf::AlterRoleStmt;

use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::parse::error::{ParseError, SourceLocation};

use super::shared;

pub(super) fn apply(
    s: &AlterRoleStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    // AlterRoleStmt.role is Option<RoleSpec> directly (not wrapped in Node).
    let role_spec = s.role.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "ALTER ROLE missing role name".into(),
    })?;
    let name =
        Identifier::from_unquoted(&role_spec.rolename).map_err(|e| ParseError::Structural {
            location: loc.clone(),
            message: format!("invalid role name {:?}: {e}", role_spec.rolename),
        })?;

    let role = cat
        .roles
        .iter_mut()
        .find(|r| r.name == name)
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "ALTER ROLE references unknown role {name} — declare with CREATE ROLE first"
            ),
        })?;

    shared::apply_options(&s.options, &mut role.attributes, loc)?;
    Ok(())
}
