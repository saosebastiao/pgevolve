//! `CREATE ROLE` / `CREATE USER` builder.
//!
//! `CREATE USER r WITH ...` is sugar for `CREATE ROLE r WITH LOGIN ...`.
//! `pg_query` stamps `stmt_type == 2` (`RoleStmtType::RolestmtUser`) for
//! `CREATE USER`; `stmt_type == 1` is `RolestmtRole`.

use pg_query::protobuf::CreateRoleStmt;

use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::role::{Role, RoleAttributes};
use crate::parse::error::{ParseError, SourceLocation};

use super::shared;

pub(super) fn apply(
    s: &CreateRoleStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&s.role).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid role name {:?}: {e}", s.role),
    })?;
    if cat.roles.iter().any(|r| r.name == name) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("role {name} declared more than once"),
        });
    }
    let mut attrs = RoleAttributes::default();
    let mut member_of: Vec<Identifier> = Vec::new();

    // pg_query RoleStmtType: RolestmtRole = 1, RolestmtUser = 2, RolestmtGroup = 3.
    // CREATE USER: stmt_type == 2. Sets LOGIN = true.
    let is_user_sugar = s.stmt_type == 2; // RolestmtUser
    if is_user_sugar {
        attrs.login = true;
    }

    // First pass: extract membership options.
    for opt_node in &s.options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        match def.defname.as_str() {
            "addroleto" => member_of.extend(shared::extract_role_name_list(def, loc)?),
            "rolemembers" => {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: "CREATE ROLE r ROLE x (reverse-membership) is not supported; \
                               use GRANT x TO r"
                        .into(),
                });
            }
            "adminmembers" => {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: "CREATE ROLE r ADMIN x is not supported; \
                               use GRANT x TO r WITH ADMIN OPTION"
                        .into(),
                });
            }
            _ => {}
        }
    }

    // Second pass: attribute options (membership filtered out).
    let attribute_opts: Vec<pg_query::protobuf::Node> = s
        .options
        .iter()
        .filter(|opt_node| match opt_node.node.as_ref() {
            Some(pg_query::NodeEnum::DefElem(def)) => !matches!(
                def.defname.as_str(),
                "addroleto" | "rolemembers" | "adminmembers"
            ),
            _ => true,
        })
        .cloned()
        .collect();
    shared::apply_options(&attribute_opts, &mut attrs, loc)?;

    cat.roles.push(Role {
        name,
        attributes: attrs,
        member_of,
        comment: None,
    });
    Ok(())
}
