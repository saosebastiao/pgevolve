//! Shared helpers for cluster parsers.

use crate::identifier::Identifier;
use crate::ir::cluster::role::RoleAttributes;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply the parsed `WITH (option…)` list to `attrs`. Each option mutates one
/// field. Unknown options surface as [`ParseError::Structural`]; PASSWORD-related
/// options are silently dropped (the spec says passwords are out-of-band).
/// Membership options must be filtered out by the caller before invoking this.
pub(super) fn apply_options(
    options: &[pg_query::protobuf::Node],
    attrs: &mut RoleAttributes,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    for opt_node in options {
        let Some(pg_query::NodeEnum::DefElem(def)) = opt_node.node.as_ref() else {
            continue;
        };
        apply_one(def, attrs, loc)?;
    }
    Ok(())
}

#[allow(clippy::cognitive_complexity)]
fn apply_one(
    def: &pg_query::protobuf::DefElem,
    attrs: &mut RoleAttributes,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    match def.defname.as_str() {
        "superuser" => attrs.superuser = extract_bool(def, loc)?,
        "createdb" => attrs.createdb = extract_bool(def, loc)?,
        "createrole" => attrs.createrole = extract_bool(def, loc)?,
        "inherit" => attrs.inherit = extract_bool(def, loc)?,
        "canlogin" => attrs.login = extract_bool(def, loc)?,
        "isreplication" => attrs.replication = extract_bool(def, loc)?,
        "bypassrls" => attrs.bypass_rls = extract_bool(def, loc)?,
        "connectionlimit" => {
            attrs.connection_limit = match extract_int(def, loc)? {
                -1 => None,
                n => Some(n),
            };
        }
        // pg_query serializes this as camelCase.
        "validUntil" => attrs.valid_until = Some(extract_string(def, loc)?),
        // Spec: passwords are not stored in source. Silently drop.
        "password" | "encryptedpassword" | "unencryptedpassword" => {}
        "rolemembers" | "addroleto" | "adminmembers" => {
            // Membership options — handled by create_role.rs / grant_membership.rs.
            // Caller must filter these out before calling apply_options.
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "internal: membership option '{}' should be handled by create_role::apply, \
                     not shared::apply_options",
                    def.defname
                ),
            });
        }
        other => {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("unknown role option '{other}'"),
            });
        }
    }
    Ok(())
}

fn extract_bool(
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<bool, ParseError> {
    let int = extract_int(def, loc)?;
    Ok(int != 0)
}

fn extract_int(def: &pg_query::protobuf::DefElem, loc: &SourceLocation) -> Result<i64, ParseError> {
    let Some(arg) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("option '{}' missing argument", def.defname),
        });
    };
    match arg {
        pg_query::NodeEnum::Integer(i) => Ok(i64::from(i.ival)),
        pg_query::NodeEnum::Boolean(b) => Ok(i64::from(b.boolval)),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "option '{}' expected integer/boolean, got {other:?}",
                def.defname
            ),
        }),
    }
}

fn extract_string(
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<String, ParseError> {
    let Some(arg) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("option '{}' missing argument", def.defname),
        });
    };
    match arg {
        pg_query::NodeEnum::String(s) => Ok(s.sval.clone()),
        pg_query::NodeEnum::TypeName(tn) => {
            // pg_query encodes VALID UNTIL 'timestamp' as a TypeName node
            // when the value looks like a type name.
            if let Some(name) = tn.names.first().and_then(|n| n.node.as_ref())
                && let pg_query::NodeEnum::String(s) = name
            {
                return Ok(s.sval.clone());
            }
            Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "option '{}' expected string, got TypeName {tn:?}",
                    def.defname
                ),
            })
        }
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("option '{}' expected string, got {other:?}", def.defname),
        }),
    }
}

/// Decode a list of role-name option-nodes (`IN ROLE x, y`) into `Identifier`s.
pub(super) fn extract_role_name_list(
    def: &pg_query::protobuf::DefElem,
    loc: &SourceLocation,
) -> Result<Vec<Identifier>, ParseError> {
    let Some(arg_node) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Ok(vec![]);
    };
    let pg_query::NodeEnum::List(list) = arg_node else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected list for option '{}', got {arg_node:?}",
                def.defname
            ),
        });
    };
    let mut out = Vec::with_capacity(list.items.len());
    for item in &list.items {
        let Some(node) = item.node.as_ref() else {
            continue;
        };
        let name_str = match node {
            pg_query::NodeEnum::RoleSpec(rs) => rs.rolename.clone(),
            pg_query::NodeEnum::String(s) => s.sval.clone(),
            other => {
                return Err(ParseError::Structural {
                    location: loc.clone(),
                    message: format!("expected role name, got {other:?}"),
                });
            }
        };
        out.push(
            Identifier::from_unquoted(&name_str).map_err(|e| ParseError::Structural {
                location: loc.clone(),
                message: format!("invalid role name {name_str:?}: {e}"),
            })?,
        );
    }
    Ok(out)
}

/// Extract role name from the `object` field of `COMMENT ON ROLE r IS '...'`.
pub(super) fn extract_role_name_from_object_node(
    node: Option<&pg_query::protobuf::Node>,
    loc: &SourceLocation,
) -> Result<Identifier, ParseError> {
    let Some(n) = node else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "COMMENT ON ROLE missing target".into(),
        });
    };
    match n.node.as_ref() {
        Some(pg_query::NodeEnum::RoleSpec(rs)) => {
            Identifier::from_unquoted(&rs.rolename).map_err(|e| ParseError::Structural {
                location: loc.clone(),
                message: format!("invalid role name {:?}: {e}", rs.rolename),
            })
        }
        Some(pg_query::NodeEnum::String(s)) => {
            Identifier::from_unquoted(&s.sval).map_err(|e| ParseError::Structural {
                location: loc.clone(),
                message: format!("invalid role name {:?}: {e}", s.sval),
            })
        }
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unexpected COMMENT target {other:?}"),
        }),
    }
}
