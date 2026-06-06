//! `CREATE`/`ALTER`/`COMMENT` `TABLESPACE` builders for cluster source.
//!
//! `DROP TABLESPACE` and `ALTER TABLESPACE … RENAME` are rejected by the router
//! (drops come from the diff; rename is modeled as drop+create), so this module
//! only handles the mutations that map onto the [`Tablespace`] IR.

use std::collections::BTreeMap;

use pg_query::protobuf::{
    AlterOwnerStmt, AlterTableSpaceOptionsStmt, CreateTableSpaceStmt, DefElem, Node, RoleSpec,
};

use crate::identifier::Identifier;
use crate::ir::cluster::catalog::ClusterCatalog;
use crate::ir::cluster::tablespace::Tablespace;
use crate::parse::error::{ParseError, SourceLocation};

/// `CREATE TABLESPACE name [OWNER role] LOCATION '/path' [WITH (opt = val …)]`.
pub(super) fn apply_create(
    s: &CreateTableSpaceStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let name = parse_name(&s.tablespacename, loc)?;
    if cat.tablespaces.iter().any(|t| t.name == name) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("tablespace {name} declared more than once"),
        });
    }
    if s.location.is_empty() {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("CREATE TABLESPACE {name} requires a non-empty LOCATION"),
        });
    }
    let owner = role_spec_to_owner(s.owner.as_ref(), loc)?;
    let options = def_elems_to_map(&s.options, loc)?;

    cat.tablespaces.push(Tablespace {
        name,
        location: s.location.clone(),
        owner,
        options,
        comment: None,
    });
    Ok(())
}

/// `ALTER TABLESPACE name SET|RESET (opt …)` — merges or removes options on a
/// previously-declared tablespace.
pub(super) fn apply_set(
    s: &AlterTableSpaceOptionsStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let name = parse_name(&s.tablespacename, loc)?;
    let ts = find_mut(cat, &name, loc, "ALTER TABLESPACE")?;

    if s.is_reset {
        // RESET (opt …): args are absent, so key off the defname only.
        for node in &s.options {
            if let Some(pg_query::NodeEnum::DefElem(def)) = node.node.as_ref() {
                ts.options.remove(&def.defname);
            }
        }
    } else {
        let merged = def_elems_to_map(&s.options, loc)?;
        ts.options.extend(merged);
    }
    Ok(())
}

/// `ALTER TABLESPACE name OWNER TO role`. The target name is a bare `String`
/// node in `object`; the new owner is a [`RoleSpec`].
pub(super) fn apply_owner(
    s: &AlterOwnerStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let name = match s.object.as_deref().and_then(|n| n.node.as_ref()) {
        Some(pg_query::NodeEnum::String(str_node)) => parse_name(&str_node.sval, loc)?,
        other => {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("ALTER TABLESPACE OWNER TO has unexpected target {other:?}"),
            });
        }
    };
    let owner =
        role_spec_to_owner(s.newowner.as_ref(), loc)?.ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: format!("ALTER TABLESPACE {name} OWNER TO missing new owner"),
        })?;

    let ts = find_mut(cat, &name, loc, "ALTER TABLESPACE")?;
    ts.owner = Some(owner);
    Ok(())
}

/// `COMMENT ON TABLESPACE name IS '…'` — sets (or clears) the comment.
pub(super) fn apply_comment(
    cat: &mut ClusterCatalog,
    name: &str,
    comment: Option<String>,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let name = parse_name(name, loc)?;
    let ts = find_mut(cat, &name, loc, "COMMENT ON TABLESPACE")?;
    ts.comment = comment;
    Ok(())
}

fn parse_name(raw: &str, loc: &SourceLocation) -> Result<Identifier, ParseError> {
    Identifier::from_unquoted(raw).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid tablespace name {raw:?}: {e}"),
    })
}

fn find_mut<'a>(
    cat: &'a mut ClusterCatalog,
    name: &Identifier,
    loc: &SourceLocation,
    stmt: &str,
) -> Result<&'a mut Tablespace, ParseError> {
    cat.tablespaces
        .iter_mut()
        .find(|t| &t.name == name)
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "{stmt} references unknown tablespace {name} — declare with CREATE TABLESPACE first"
            ),
        })
}

/// Decode a [`RoleSpec`] owner into an [`Identifier`]. `None` → unmanaged owner.
fn role_spec_to_owner(
    spec: Option<&RoleSpec>,
    loc: &SourceLocation,
) -> Result<Option<Identifier>, ParseError> {
    let Some(rs) = spec else {
        return Ok(None);
    };
    let id = Identifier::from_unquoted(&rs.rolename).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid owner role name {:?}: {e}", rs.rolename),
    })?;
    Ok(Some(id))
}

/// Convert a `WITH (opt = val …)` / `SET (opt = val …)` `DefElem` list into a
/// `name → value` map. Each value is the stringified argument node. Options
/// missing an argument are an error here (RESET handles its own arg-less case).
fn def_elems_to_map(
    options: &[Node],
    loc: &SourceLocation,
) -> Result<BTreeMap<String, String>, ParseError> {
    let mut out = BTreeMap::new();
    for node in options {
        let Some(pg_query::NodeEnum::DefElem(def)) = node.node.as_ref() else {
            continue;
        };
        let value = def_elem_value(def, loc)?;
        out.insert(def.defname.clone(), value);
    }
    Ok(out)
}

/// Stringify a `DefElem` argument (`Integer`/`Float`/`String`/`Boolean`).
fn def_elem_value(def: &DefElem, loc: &SourceLocation) -> Result<String, ParseError> {
    let Some(arg) = def.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("tablespace option '{}' missing argument", def.defname),
        });
    };
    match arg {
        pg_query::NodeEnum::Integer(i) => Ok(i.ival.to_string()),
        pg_query::NodeEnum::Float(f) => Ok(f.fval.clone()),
        pg_query::NodeEnum::String(s) => Ok(s.sval.clone()),
        pg_query::NodeEnum::Boolean(b) => Ok(b.boolval.to_string()),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "tablespace option '{}' has unsupported value {other:?}",
                def.defname
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::ir::cluster::catalog::ClusterCatalog;
    use crate::parse::error::ParseError;

    use super::super::apply_file;

    fn parse(sql: &str) -> Result<ClusterCatalog, ParseError> {
        let mut cat = ClusterCatalog::empty();
        apply_file(sql, Path::new("tablespaces/test.sql"), &mut cat)?;
        Ok(cat)
    }

    #[test]
    fn create_simple() {
        let cat = parse("CREATE TABLESPACE t LOCATION '/x';").unwrap();
        assert_eq!(cat.tablespaces.len(), 1);
        let ts = &cat.tablespaces[0];
        assert_eq!(ts.name.as_str(), "t");
        assert_eq!(ts.location, "/x");
        assert!(ts.owner.is_none());
        assert!(ts.options.is_empty());
        assert!(ts.comment.is_none());
    }

    #[test]
    fn create_with_owner_and_options() {
        let cat =
            parse("CREATE TABLESPACE t OWNER app_owner LOCATION '/x' WITH (seq_page_cost = 2.0);")
                .unwrap();
        let ts = &cat.tablespaces[0];
        assert_eq!(ts.owner.as_ref().map(Identifier::as_str), Some("app_owner"));
        assert_eq!(
            ts.options.get("seq_page_cost").map(String::as_str),
            Some("2.0")
        );
    }

    #[test]
    fn alter_set_merges_option() {
        let cat = parse(
            "CREATE TABLESPACE t LOCATION '/x';\
             ALTER TABLESPACE t SET (random_page_cost = 1.5);",
        )
        .unwrap();
        let ts = &cat.tablespaces[0];
        assert_eq!(
            ts.options.get("random_page_cost").map(String::as_str),
            Some("1.5")
        );
    }

    #[test]
    fn alter_reset_removes_option() {
        let cat = parse(
            "CREATE TABLESPACE t LOCATION '/x' WITH (seq_page_cost = 2.0);\
             ALTER TABLESPACE t RESET (seq_page_cost);",
        )
        .unwrap();
        assert!(cat.tablespaces[0].options.is_empty());
    }

    #[test]
    fn alter_owner_to() {
        let cat = parse(
            "CREATE TABLESPACE t LOCATION '/x';\
             ALTER TABLESPACE t OWNER TO app_owner;",
        )
        .unwrap();
        assert_eq!(
            cat.tablespaces[0].owner.as_ref().map(Identifier::as_str),
            Some("app_owner")
        );
    }

    #[test]
    fn comment_on_tablespace() {
        let cat = parse(
            "CREATE TABLESPACE t LOCATION '/x';\
             COMMENT ON TABLESPACE t IS 'fast storage';",
        )
        .unwrap();
        assert_eq!(cat.tablespaces[0].comment.as_deref(), Some("fast storage"));
    }

    #[test]
    fn duplicate_name_errors() {
        let err = parse(
            "CREATE TABLESPACE t LOCATION '/x';\
             CREATE TABLESPACE t LOCATION '/y';",
        )
        .unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { message, .. } if message.contains("more than once"))
        );
    }

    #[test]
    fn alter_before_create_errors() {
        let err = parse("ALTER TABLESPACE t SET (seq_page_cost = 2.0);").unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { message, .. } if message.contains("unknown tablespace"))
        );
    }

    #[test]
    fn drop_tablespace_rejected() {
        let err = parse("DROP TABLESPACE t;").unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { message, .. } if message.contains("DROP TABLESPACE"))
        );
    }

    #[test]
    fn rename_tablespace_rejected() {
        let err = parse("ALTER TABLESPACE t RENAME TO u;").unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { message, .. } if message.contains("RENAME"))
        );
    }

    use crate::identifier::Identifier;
}
