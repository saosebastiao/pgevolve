//! Cluster-level source parser. Reads `roles/*.sql` (alphabetical) into a
//! [`ClusterCatalog`]. Mirrors the shape of the per-DB `parse::parse_directory`.

mod alter_role;
mod create_role;
mod grant_membership;
mod shared;

use std::path::Path;

use crate::ir::cluster::catalog::ClusterCatalog;
use crate::parse::error::{ParseError, SourceLocation};

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
    cat.canonicalize();
    Ok(cat)
}

fn collect_sql_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, ParseError> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| ParseError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?
        .filter_map(Result::ok)
        .map(|d| d.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
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
    use pg_query::protobuf::ObjectType;
    let kind = ObjectType::try_from(s.objtype).unwrap_or(ObjectType::Undefined);
    if kind != ObjectType::ObjectRole {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "only COMMENT ON ROLE is supported in cluster source".into(),
        });
    }
    let role_name = shared::extract_role_name_from_object_node(s.object.as_deref(), loc)?;
    let comment = if s.comment.is_empty() {
        None
    } else {
        Some(s.comment.clone())
    };
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
