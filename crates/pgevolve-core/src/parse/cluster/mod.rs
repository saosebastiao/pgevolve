//! Cluster-level source parser. Reads `roles/*.sql` (alphabetical) into a
//! [`ClusterCatalog`]. Mirrors the shape of the per-DB `parse::parse_directory`.

mod alter_role;
mod create_role;
mod grant_membership;
mod shared;
mod tablespace_stmt;

use std::path::Path;

use pg_query::protobuf::ObjectType;

use crate::ir::cluster::catalog::ClusterCatalog;
use crate::parse::error::{ParseError, SourceLocation};

/// Compare a raw `ObjectType` discriminant (`i32`) against a known variant,
/// mirroring [`apply_comment`]'s decode-then-compare style.
fn object_type_is(raw: i32, want: ObjectType) -> bool {
    ObjectType::try_from(raw).unwrap_or(ObjectType::Undefined) == want
}

/// Parse every `*.sql` file under `roles_dir`, alphabetical order. Returns a
/// canonicalized [`ClusterCatalog`].
pub fn parse_cluster_directory(roles_dir: &Path) -> Result<ClusterCatalog, ParseError> {
    let mut cat = ClusterCatalog::empty();
    let entries = collect_sql_files(roles_dir)?;
    for path in entries {
        let sql = std::fs::read_to_string(&path).map_err(|e| ParseError::Io {
            path: path.clone(),
            source: e,
        })?;
        apply_file(&sql, &path, &mut cat)?;
    }
    cat.canonicalize().map_err(|e| ParseError::Structural {
        location: SourceLocation::new(roles_dir.to_path_buf(), 0, 0),
        message: e.to_string(),
    })?;
    Ok(cat)
}

fn collect_sql_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, ParseError> {
    let entries = std::fs::read_dir(dir).map_err(|e| ParseError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| ParseError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "sql") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn apply_file(sql: &str, path: &Path, cat: &mut ClusterCatalog) -> Result<(), ParseError> {
    let loc = SourceLocation::new(path.to_path_buf(), 0, 0);
    let parsed = pg_query::parse(sql).map_err(|e| ParseError::PgQuery {
        location: loc.clone(),
        message: e.to_string(),
    })?;
    for stmt in &parsed.protobuf.stmts {
        let Some(node) = stmt.stmt.as_ref().and_then(|s| s.node.as_ref()) else {
            continue;
        };
        match node {
            pg_query::NodeEnum::CreateRoleStmt(s) => create_role::apply(s, cat, &loc)?,
            pg_query::NodeEnum::AlterRoleStmt(s) => alter_role::apply(s, cat, &loc)?,
            pg_query::NodeEnum::GrantRoleStmt(s) => grant_membership::apply(s, cat, &loc)?,
            pg_query::NodeEnum::CommentStmt(s) => apply_comment(s, cat, &loc)?,
            pg_query::NodeEnum::CreateTableSpaceStmt(s) => {
                tablespace_stmt::apply_create(s, cat, &loc)?;
            }
            pg_query::NodeEnum::AlterTableSpaceOptionsStmt(s) => {
                tablespace_stmt::apply_set(s, cat, &loc)?;
            }
            pg_query::NodeEnum::AlterOwnerStmt(s)
                if object_type_is(s.object_type, ObjectType::ObjectTablespace) =>
            {
                tablespace_stmt::apply_owner(s, cat, &loc)?;
            }
            pg_query::NodeEnum::RenameStmt(s)
                if object_type_is(s.rename_type, ObjectType::ObjectTablespace) =>
            {
                return Err(ParseError::Structural {
                    location: loc,
                    message: "ALTER TABLESPACE … RENAME is not supported — rename is drop+create"
                        .into(),
                });
            }
            pg_query::NodeEnum::DropTableSpaceStmt(_) => {
                return Err(ParseError::Structural {
                    location: loc,
                    message: "DROP TABLESPACE in source is not supported — drops happen via diff"
                        .into(),
                });
            }
            pg_query::NodeEnum::DropRoleStmt(_) => {
                return Err(ParseError::Structural {
                    location: loc,
                    message: "DROP ROLE in source is not supported — drops happen via diff".into(),
                });
            }
            other => {
                return Err(ParseError::Structural {
                    location: loc,
                    message: format!(
                        "{other:?} is not supported in cluster source (roles/); \
                         allowed: CREATE ROLE, CREATE USER, ALTER ROLE, GRANT role TO target, \
                         COMMENT ON ROLE"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn apply_comment(
    s: &pg_query::protobuf::CommentStmt,
    cat: &mut ClusterCatalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let kind = ObjectType::try_from(s.objtype).unwrap_or(ObjectType::Undefined);
    let comment = if s.comment.is_empty() {
        None
    } else {
        Some(s.comment.clone())
    };
    if kind == ObjectType::ObjectTablespace {
        let name = comment_target_string(s.object.as_deref(), loc)?;
        return tablespace_stmt::apply_comment(cat, &name, comment, loc);
    }
    if kind != ObjectType::ObjectRole {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "only COMMENT ON ROLE and COMMENT ON TABLESPACE are supported in cluster \
                      source"
                .into(),
        });
    }
    let role_name = shared::extract_role_name_from_object_node(s.object.as_deref(), loc)?;
    let role = cat
        .roles
        .iter_mut()
        .find(|r| r.name == role_name)
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: format!("COMMENT ON ROLE references unknown role {role_name}"),
        })?;
    role.comment = comment;
    Ok(())
}

/// Extract a bare-`String` comment target (`COMMENT ON TABLESPACE name …` keeps
/// the object name as a plain `String` node, not a `RoleSpec`).
fn comment_target_string(
    node: Option<&pg_query::protobuf::Node>,
    loc: &SourceLocation,
) -> Result<String, ParseError> {
    match node.and_then(|n| n.node.as_ref()) {
        Some(pg_query::NodeEnum::String(s)) => Ok(s.sval.clone()),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unexpected COMMENT ON TABLESPACE target {other:?}"),
        }),
    }
}
