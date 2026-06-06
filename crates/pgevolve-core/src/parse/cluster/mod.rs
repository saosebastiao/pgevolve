//! Cluster-level source parser — per-file SQL router for cluster objects.
//!
//! [`parse_cluster_sources`] reads both `roles/*.sql` and `tablespaces/*.sql`
//! (alphabetical) into a single [`ClusterCatalog`];
//! [`parse_cluster_directory`] reads a single directory.
//! Mirrors the shape of the per-DB `parse::parse_directory`.

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

/// Parse every `*.sql` file under `dir`, alphabetical order. Returns a
/// canonicalized [`ClusterCatalog`].
///
/// For reading both roles and tablespaces together, use
/// [`parse_cluster_sources`] instead — it combines both directories into a
/// single catalog.
pub fn parse_cluster_directory(roles_dir: &Path) -> Result<ClusterCatalog, ParseError> {
    let mut cat = ClusterCatalog::empty();
    apply_dir_into(roles_dir, &mut cat)?;
    cat.canonicalize().map_err(|e| ParseError::Structural {
        location: SourceLocation::new(roles_dir.to_path_buf(), 0, 0),
        message: e.to_string(),
    })?;
    Ok(cat)
}

/// Parse cluster source from both the roles and tablespaces directories into one [`ClusterCatalog`].
///
/// A directory that does not exist is skipped. Roles are parsed first, then
/// tablespaces; the result is canonicalized once.
pub fn parse_cluster_sources(
    roles_dir: &Path,
    tablespaces_dir: &Path,
) -> Result<ClusterCatalog, ParseError> {
    let mut cat = ClusterCatalog::empty();
    apply_dir_into(roles_dir, &mut cat)?;
    apply_dir_into(tablespaces_dir, &mut cat)?;
    // Use the roles_dir as the location anchor for structural errors (it's the
    // primary source directory).
    cat.canonicalize().map_err(|e| ParseError::Structural {
        location: SourceLocation::new(roles_dir.to_path_buf(), 0, 0),
        message: e.to_string(),
    })?;
    Ok(cat)
}

/// Append every `*.sql` file from `dir` into `cat`, alphabetical order.
/// If the directory does not exist, returns `Ok(())` immediately.
/// Does **not** canonicalize — callers are responsible for that.
fn apply_dir_into(dir: &Path, cat: &mut ClusterCatalog) -> Result<(), ParseError> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = collect_sql_files(dir)?;
    for path in entries {
        let sql = std::fs::read_to_string(&path).map_err(|e| ParseError::Io {
            path: path.clone(),
            source: e,
        })?;
        apply_file(&sql, &path, cat)?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse_cluster_sources` combines roles and tablespaces from separate dirs
    /// into a single catalog.
    #[test]
    fn parse_cluster_sources_combines_roles_and_tablespaces() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let roles_dir = tmp.path().join("roles");
        let tablespaces_dir = tmp.path().join("tablespaces");
        std::fs::create_dir_all(&roles_dir).unwrap();
        std::fs::create_dir_all(&tablespaces_dir).unwrap();

        std::fs::write(roles_dir.join("r.sql"), "CREATE ROLE cluster_test_role;\n").unwrap();
        std::fs::write(
            tablespaces_dir.join("t.sql"),
            "CREATE TABLESPACE cluster_test_ts LOCATION '/tmp/cluster_test_ts';\n",
        )
        .unwrap();

        let cat =
            parse_cluster_sources(&roles_dir, &tablespaces_dir).expect("parse_cluster_sources ok");

        assert!(
            cat.roles
                .iter()
                .any(|r| r.name.as_str() == "cluster_test_role"),
            "expected role cluster_test_role in catalog; got roles: {:?}",
            cat.roles
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            cat.tablespaces
                .iter()
                .any(|t| t.name.as_str() == "cluster_test_ts"),
            "expected tablespace cluster_test_ts in catalog; got tablespaces: {:?}",
            cat.tablespaces
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// A missing tablespaces directory is silently skipped; only the role appears.
    #[test]
    fn parse_cluster_sources_skips_missing_tablespaces_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let roles_dir = tmp.path().join("roles");
        let tablespaces_dir = tmp.path().join("tablespaces"); // not created
        std::fs::create_dir_all(&roles_dir).unwrap();

        std::fs::write(roles_dir.join("r.sql"), "CREATE ROLE cluster_only_role;\n").unwrap();

        let cat =
            parse_cluster_sources(&roles_dir, &tablespaces_dir).expect("parse_cluster_sources ok");

        assert!(
            cat.roles
                .iter()
                .any(|r| r.name.as_str() == "cluster_only_role"),
            "expected role cluster_only_role in catalog; got roles: {:?}",
            cat.roles
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            cat.tablespaces.is_empty(),
            "expected no tablespaces when dir is missing, got: {:?}",
            cat.tablespaces
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
        );
    }
}
