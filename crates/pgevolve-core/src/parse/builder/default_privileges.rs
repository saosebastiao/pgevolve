//! `ALTER DEFAULT PRIVILEGES ... GRANT ...` — future-object grants.
//!
//! Decodes the options list to extract `FOR ROLE` targets and optional
//! `IN SCHEMA` scopes, then decodes the nested `GrantStmt` for privileges and
//! grantees. Produces one [`DefaultPrivilegeRule`] per (`target_role` × schema)
//! cross-product entry.
//!
//! REVOKE in source → [`ParseError::Structural`]. Missing `FOR ROLE` → error.

use pg_query::NodeEnum;
use pg_query::protobuf::{AlterDefaultPrivilegesStmt, ObjectType, RoleSpecType};

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
use crate::ir::grant::{Grant, GrantTarget, Privilege};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply an `ALTER DEFAULT PRIVILEGES` statement to the catalog.
///
/// The statement produces one [`DefaultPrivilegeRule`] per
/// (`target_role` × schema) cross-product. Each rule carries the full grants
/// decoded from the nested `GrantStmt.action`.
#[allow(clippy::too_many_lines)] // exhaustive `ALTER DEFAULT PRIVILEGES` decoder; one arm per object-type / action combination.
pub(crate) fn apply(
    s: &AlterDefaultPrivilegesStmt,
    cat: &mut Catalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    // Decode options: `FOR ROLE` and `IN SCHEMA`.
    let (target_roles, schemas) = decode_options(&s.options, loc)?;

    // FOR ROLE is required — no current_user fallback in source.
    if target_roles.is_empty() {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "ALTER DEFAULT PRIVILEGES requires a FOR ROLE clause in source DDL; \
                      current_user-scoped defaults are not supported in source files"
                .into(),
        });
    }

    // Decode the nested GrantStmt (`action`).
    let action = s.action.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "ALTER DEFAULT PRIVILEGES missing GRANT/REVOKE action".into(),
    })?;

    // Reject REVOKE in source.
    if !action.is_grant {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "REVOKE in ALTER DEFAULT PRIVILEGES in source is not supported — \
                      revocations come from diff"
                .into(),
        });
    }

    // Reject GRANTED BY.
    if let Some(ref grantor) = action.grantor {
        let roletype = pg_query::protobuf::RoleSpecType::try_from(grantor.roletype)
            .unwrap_or(RoleSpecType::Undefined);
        if roletype != RoleSpecType::Undefined || !grantor.rolename.is_empty() {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: "ALTER DEFAULT PRIVILEGES ... GRANT ... GRANTED BY is not supported \
                          in source DDL (v0.3.1)"
                    .into(),
            });
        }
    }

    // Object type.
    let objtype = ObjectType::try_from(action.objtype).unwrap_or(ObjectType::Undefined);
    let default_obj_type = default_priv_obj_type(objtype, loc)?;

    // Privileges — expand GRANT ALL per object type.
    let privs: Vec<Privilege> = if action.privileges.is_empty() {
        all_privs_for_default(default_obj_type)
    } else {
        let mut out = Vec::new();
        for node in &action.privileges {
            let Some(NodeEnum::AccessPriv(ap)) = node.node.as_ref() else {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: format!(
                        "expected AccessPriv in ALTER DEFAULT PRIVILEGES privileges list, \
                         got {:?}",
                        node.node.as_ref().map(std::mem::discriminant)
                    ),
                });
            };
            if ap.priv_name.is_empty() {
                // ALL
                out.extend(all_privs_for_default(default_obj_type));
            } else {
                out.push(priv_from_keyword(&ap.priv_name, loc)?);
            }
        }
        out
    };

    // Grantees.
    let grantees = decode_grantees(&action.grantees, loc)?;

    // Build one rule per (target_role × schema) cross-product, merging into
    // any existing rule with the same key so that multiple
    // `ALTER DEFAULT PRIVILEGES FOR ROLE x GRANT ... TO ...` statements
    // accumulate their grants rather than silently overwriting each other.
    for target_role in &target_roles {
        let target_schemas: Vec<Option<Identifier>> = if schemas.is_empty() {
            vec![None]
        } else {
            schemas.iter().map(|s| Some(s.clone())).collect()
        };
        for schema in target_schemas {
            let new_grants: Vec<Grant> = grantees
                .iter()
                .flat_map(|grantee| {
                    privs.iter().map(move |&pv| Grant {
                        grantee: grantee.clone(),
                        privilege: pv,
                        with_grant_option: action.grant_option,
                        columns: None,
                    })
                })
                .collect();
            // Find an existing rule with the same (target_role, schema,
            // object_type) and extend it; push a fresh rule if none exists.
            let existing = cat.default_privileges.iter_mut().find(|r| {
                r.target_role == *target_role
                    && r.schema == schema
                    && r.object_type == default_obj_type
            });
            if let Some(rule) = existing {
                rule.grants.extend(new_grants);
            } else {
                cat.default_privileges.push(DefaultPrivilegeRule {
                    target_role: target_role.clone(),
                    schema,
                    object_type: default_obj_type,
                    grants: new_grants,
                });
            }
        }
    }

    Ok(())
}

// ─── Options decoding ─────────────────────────────────────────────────────────

/// Decode the `options` list of `AlterDefaultPrivilegesStmt`.
///
/// Returns `(target_roles, schemas)`. Schemas is empty if no `IN SCHEMA` was given.
fn decode_options(
    options: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<(Vec<Identifier>, Vec<Identifier>), ParseError> {
    let mut target_roles: Vec<Identifier> = Vec::new();
    let mut schemas: Vec<Identifier> = Vec::new();

    for opt_node in options {
        let Some(NodeEnum::DefElem(de)) = opt_node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "expected DefElem in ALTER DEFAULT PRIVILEGES options, got {:?}",
                    opt_node.node.as_ref().map(std::mem::discriminant)
                ),
            });
        };
        match de.defname.as_str() {
            "roles" => {
                // arg is a List of RoleSpec nodes.
                let arg_node = de
                    .arg
                    .as_ref()
                    .and_then(|a| a.node.as_ref())
                    .ok_or_else(|| ParseError::Structural {
                        location: loc.clone(),
                        message: "FOR ROLE DefElem missing arg".into(),
                    })?;
                let NodeEnum::List(list) = arg_node else {
                    return Err(ParseError::Structural {
                        location: loc.clone(),
                        message: format!(
                            "expected List in FOR ROLE DefElem arg, got {:?}",
                            std::mem::discriminant(arg_node)
                        ),
                    });
                };
                for item in &list.items {
                    let Some(NodeEnum::RoleSpec(rs)) = item.node.as_ref() else {
                        return Err(ParseError::Structural {
                            location: loc.clone(),
                            message: format!(
                                "expected RoleSpec in FOR ROLE list, got {:?}",
                                item.node.as_ref().map(std::mem::discriminant)
                            ),
                        });
                    };
                    target_roles.push(shared::ident(&rs.rolename, loc)?);
                }
            }
            "schemas" => {
                // arg is a List of String nodes.
                let arg_node = de
                    .arg
                    .as_ref()
                    .and_then(|a| a.node.as_ref())
                    .ok_or_else(|| ParseError::Structural {
                        location: loc.clone(),
                        message: "IN SCHEMA DefElem missing arg".into(),
                    })?;
                let NodeEnum::List(list) = arg_node else {
                    return Err(ParseError::Structural {
                        location: loc.clone(),
                        message: format!(
                            "expected List in IN SCHEMA DefElem arg, got {:?}",
                            std::mem::discriminant(arg_node)
                        ),
                    });
                };
                for item in &list.items {
                    let Some(NodeEnum::String(s)) = item.node.as_ref() else {
                        return Err(ParseError::Structural {
                            location: loc.clone(),
                            message: format!(
                                "expected String in IN SCHEMA list, got {:?}",
                                item.node.as_ref().map(std::mem::discriminant)
                            ),
                        });
                    };
                    schemas.push(shared::ident(&s.sval, loc)?);
                }
            }
            other => {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: format!(
                        "unknown ALTER DEFAULT PRIVILEGES option '{other}'; \
                         expected 'roles' or 'schemas'"
                    ),
                });
            }
        }
    }

    Ok((target_roles, schemas))
}

// ─── Privilege helpers ────────────────────────────────────────────────────────

/// Map an `ObjectType` to a [`DefaultPrivObjectType`].
fn default_priv_obj_type(
    objtype: ObjectType,
    loc: &SourceLocation,
) -> Result<DefaultPrivObjectType, ParseError> {
    match objtype {
        ObjectType::ObjectTable => Ok(DefaultPrivObjectType::Tables),
        ObjectType::ObjectSequence => Ok(DefaultPrivObjectType::Sequences),
        ObjectType::ObjectFunction | ObjectType::ObjectRoutine => {
            Ok(DefaultPrivObjectType::Functions)
        }
        ObjectType::ObjectType => Ok(DefaultPrivObjectType::Types),
        ObjectType::ObjectSchema => Ok(DefaultPrivObjectType::Schemas),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "ALTER DEFAULT PRIVILEGES: unsupported object type {other:?}; \
                 expected TABLES, SEQUENCES, FUNCTIONS, TYPES, or SCHEMAS"
            ),
        }),
    }
}

/// Privileges applicable for GRANT ALL on a given `DefaultPrivObjectType`.
fn all_privs_for_default(kind: DefaultPrivObjectType) -> Vec<Privilege> {
    match kind {
        DefaultPrivObjectType::Tables => vec![
            Privilege::Select,
            Privilege::Insert,
            Privilege::Update,
            Privilege::Delete,
            Privilege::Truncate,
            Privilege::References,
            Privilege::Trigger,
        ],
        DefaultPrivObjectType::Sequences => {
            vec![Privilege::Usage, Privilege::Select, Privilege::Update]
        }
        DefaultPrivObjectType::Functions => vec![Privilege::Execute],
        DefaultPrivObjectType::Types => vec![Privilege::Usage],
        DefaultPrivObjectType::Schemas => vec![Privilege::Usage, Privilege::Create],
    }
}

/// Parse a SQL privilege keyword into [`Privilege`].
fn priv_from_keyword(kw: &str, loc: &SourceLocation) -> Result<Privilege, ParseError> {
    match kw.to_ascii_uppercase().as_str() {
        "SELECT" => Ok(Privilege::Select),
        "INSERT" => Ok(Privilege::Insert),
        "UPDATE" => Ok(Privilege::Update),
        "DELETE" => Ok(Privilege::Delete),
        "TRUNCATE" => Ok(Privilege::Truncate),
        "REFERENCES" => Ok(Privilege::References),
        "TRIGGER" => Ok(Privilege::Trigger),
        "USAGE" => Ok(Privilege::Usage),
        "EXECUTE" => Ok(Privilege::Execute),
        "CREATE" => Ok(Privilege::Create),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unknown privilege keyword '{other}'"),
        }),
    }
}

/// Decode the grantees list from the nested `GrantStmt`.
fn decode_grantees(
    nodes: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<Vec<GrantTarget>, ParseError> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let Some(NodeEnum::RoleSpec(rs)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "expected RoleSpec in GRANT grantees, got {:?}",
                    n.node.as_ref().map(std::mem::discriminant)
                ),
            });
        };
        let roletype = RoleSpecType::try_from(rs.roletype).unwrap_or(RoleSpecType::Undefined);
        let target = if roletype == RoleSpecType::RolespecPublic {
            GrantTarget::Public
        } else {
            GrantTarget::Role(shared::ident(&rs.rolename, loc)?)
        };
        out.push(target);
    }
    Ok(out)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn empty_cat() -> Catalog {
        Catalog::empty()
    }

    fn parse_adp(sql: &str) -> AlterDefaultPrivilegesStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterDefaultPrivilegesStmt(s) = stmt else {
            panic!("not AlterDefaultPrivilegesStmt")
        };
        s
    }

    #[test]
    fn in_schema_tables() {
        let mut cat = empty_cat();
        let s = parse_adp(
            "ALTER DEFAULT PRIVILEGES FOR ROLE x IN SCHEMA y GRANT SELECT ON TABLES TO z;",
        );
        apply(&s, &mut cat, &loc()).unwrap();
        assert_eq!(cat.default_privileges.len(), 1);
        let rule = &cat.default_privileges[0];
        assert_eq!(rule.target_role, id("x"));
        assert_eq!(rule.schema, Some(id("y")));
        assert_eq!(rule.object_type, DefaultPrivObjectType::Tables);
        assert_eq!(rule.grants.len(), 1);
        assert_eq!(rule.grants[0].privilege, Privilege::Select);
        assert_eq!(rule.grants[0].grantee, GrantTarget::Role(id("z")));
    }

    #[test]
    fn global_functions() {
        let mut cat = empty_cat();
        let s = parse_adp(
            "ALTER DEFAULT PRIVILEGES FOR ROLE owner_role GRANT EXECUTE ON FUNCTIONS TO app_role;",
        );
        apply(&s, &mut cat, &loc()).unwrap();
        assert_eq!(cat.default_privileges.len(), 1);
        let rule = &cat.default_privileges[0];
        assert_eq!(rule.target_role, id("owner_role"));
        assert_eq!(rule.schema, None);
        assert_eq!(rule.object_type, DefaultPrivObjectType::Functions);
        assert_eq!(rule.grants[0].privilege, Privilege::Execute);
    }

    #[test]
    fn for_role_required() {
        let mut cat = empty_cat();
        // Omit FOR ROLE — pg_query will parse this as current_user; the options
        // list will have no "roles" entry.
        let s = parse_adp("ALTER DEFAULT PRIVILEGES GRANT SELECT ON TABLES TO alice;");
        let err = apply(&s, &mut cat, &loc()).unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. }
                if message.contains("FOR ROLE") || message.contains("for role") || message.contains("requires")),
            "expected FOR ROLE error, got: {err:?}"
        );
    }

    #[test]
    fn revoke_rejected() {
        let mut cat = empty_cat();
        let s = parse_adp("ALTER DEFAULT PRIVILEGES FOR ROLE x REVOKE SELECT ON TABLES FROM z;");
        let err = apply(&s, &mut cat, &loc()).unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. }
                if message.contains("REVOKE") || message.contains("revoke")),
            "expected REVOKE error, got: {err:?}"
        );
    }

    #[test]
    fn grant_all_expansion_for_default_privileges() {
        let mut cat = empty_cat();
        let s =
            parse_adp("ALTER DEFAULT PRIVILEGES FOR ROLE x IN SCHEMA s GRANT ALL ON TABLES TO z;");
        apply(&s, &mut cat, &loc()).unwrap();
        assert_eq!(cat.default_privileges.len(), 1);
        let rule = &cat.default_privileges[0];
        // 7 table privileges: SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER
        assert_eq!(rule.grants.len(), 7);
        let privs: Vec<Privilege> = rule.grants.iter().map(|g| g.privilege).collect();
        assert!(privs.contains(&Privilege::Select));
        assert!(privs.contains(&Privilege::Insert));
        assert!(privs.contains(&Privilege::Update));
        assert!(privs.contains(&Privilege::Delete));
        assert!(privs.contains(&Privilege::Truncate));
        assert!(privs.contains(&Privilege::References));
        assert!(privs.contains(&Privilege::Trigger));
    }
}
