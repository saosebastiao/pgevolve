//! `CREATE POLICY name ON tablename ...` — RLS policy declarations.
//!
//! `ALTER POLICY` and `DROP POLICY` are rejected in source (diff-driven only).

use pg_query::protobuf::CreatePolicyStmt;

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::grant::GrantTarget;
use crate::ir::policy::{Policy, PolicyCommand};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply a `CREATE POLICY` statement to the partial catalog.
///
/// The named table must already be present in `cat` (declared via
/// `CREATE TABLE` earlier in the source). Policy expressions (`USING` /
/// `WITH CHECK`) are canonicalized via [`crate::parse::normalize_expr::from_pg_node`].
pub(crate) fn apply(
    s: &CreatePolicyStmt,
    cat: &mut Catalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let name = Identifier::from_unquoted(&s.policy_name).map_err(|e| ParseError::Structural {
        location: loc.clone(),
        message: format!("invalid policy name {:?}: {e}", s.policy_name),
    })?;

    let rv = s.table.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "CREATE POLICY missing target table".into(),
    })?;
    let table_qname = qname_from_rangevar(rv, loc)?;

    let command = decode_cmd(&s.cmd_name, loc)?;
    let permissive = s.permissive;

    let mut roles = decode_role_targets(&s.roles, loc)?;
    if roles.is_empty() {
        roles.push(GrantTarget::Public);
    }

    let using = s
        .qual
        .as_ref()
        .map(|expr| decode_expr_node(expr, loc))
        .transpose()?;
    let with_check = s
        .with_check
        .as_ref()
        .map(|expr| decode_expr_node(expr, loc))
        .transpose()?;

    // Validation: WITH CHECK invalid on FOR SELECT / FOR DELETE.
    if with_check.is_some() && !command.allows_with_check() {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "WITH CHECK is invalid on FOR {} policies; PG rejects",
                command.sql_keyword()
            ),
        });
    }

    // Attach to the named table.
    let table = cat
        .tables
        .iter_mut()
        .find(|t| t.qname == table_qname)
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "CREATE POLICY {name} ON {table_qname} — unknown table; \
                 declare with CREATE TABLE first"
            ),
        })?;

    if table.policies.iter().any(|p| p.name == name) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("policy {name} declared more than once on {table_qname}"),
        });
    }

    table.policies.push(Policy {
        name,
        permissive,
        command,
        roles,
        using,
        with_check,
    });
    Ok(())
}

fn decode_cmd(s: &str, loc: &SourceLocation) -> Result<PolicyCommand, ParseError> {
    match s.to_ascii_lowercase().as_str() {
        "all" => Ok(PolicyCommand::All),
        "select" => Ok(PolicyCommand::Select),
        "insert" => Ok(PolicyCommand::Insert),
        "update" => Ok(PolicyCommand::Update),
        "delete" => Ok(PolicyCommand::Delete),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unknown policy command kind {other:?}"),
        }),
    }
}

fn decode_role_targets(
    nodes: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<Vec<GrantTarget>, ParseError> {
    use pg_query::protobuf::RoleSpecType;
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let Some(pg_query::NodeEnum::RoleSpec(rs)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!("expected RoleSpec in TO clause, got {n:?}"),
            });
        };
        let role_type = RoleSpecType::try_from(rs.roletype).unwrap_or(RoleSpecType::Undefined);
        if role_type == RoleSpecType::RolespecPublic {
            out.push(GrantTarget::Public);
        } else {
            let ident =
                Identifier::from_unquoted(&rs.rolename).map_err(|e| ParseError::Structural {
                    location: loc.clone(),
                    message: format!("invalid role name {:?}: {e}", rs.rolename),
                })?;
            out.push(GrantTarget::Role(ident));
        }
    }
    Ok(out)
}

fn qname_from_rangevar(
    rv: &pg_query::protobuf::RangeVar,
    loc: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    // Delegate to the shared helper which already handles schema defaulting.
    // For CREATE POLICY there is no file-level `-- @pgevolve schema=` directive
    // available at this call site, so we pass `None`. Unqualified table names
    // will be rejected with [`ParseError::UnqualifiedName`].
    shared::resolve_qname(rv, None, loc)
}

/// Decode an expression node to a [`NormalizedExpr`].
///
/// Reuses [`crate::parse::normalize_expr::from_pg_node`], which wraps the
/// expression in a `SELECT <expr>` scaffold, deparses through `pg_query`,
/// strips the prefix, and lowercases reserved keywords before hashing.
/// This is the same canonicalizer used by check constraints and trigger
/// WHEN clauses.
fn decode_expr_node(
    expr: &pg_query::protobuf::Node,
    loc: &SourceLocation,
) -> Result<NormalizedExpr, ParseError> {
    let inner_enum = expr.node.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "policy expression node has no inner node".into(),
    })?;
    crate::parse::normalize_expr::from_pg_node(inner_enum, None, loc).map_err(|e| {
        ParseError::Structural {
            location: loc.clone(),
            message: format!("failed to canonicalize policy expression: {e}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use crate::ir::grant::GrantTarget;
    use crate::ir::policy::PolicyCommand;
    use crate::parse::ParseError;
    use crate::parse::parse_directory;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    /// Parse a multi-statement SQL string by writing it to a temp dir.
    fn parse_source(sql: &str) -> Result<crate::ir::catalog::Catalog, ParseError> {
        let tmp = tempdir().expect("tempdir");
        write(tmp.path(), "schema.sql", sql);
        parse_directory(tmp.path(), &[])
    }

    #[test]
    fn create_simple_policy() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY docs_sel ON app.docs USING (true);
        ";
        let cat = parse_source(sql).expect("parses");
        let table = cat
            .tables
            .iter()
            .find(|t| t.qname.to_string() == "app.docs")
            .unwrap();
        assert_eq!(table.policies.len(), 1);
        let p = &table.policies[0];
        assert_eq!(p.name.as_str(), "docs_sel");
        assert!(p.permissive);
        assert_eq!(p.command, PolicyCommand::All);
        assert_eq!(p.roles, vec![GrantTarget::Public]);
        assert!(p.using.is_some());
        assert!(p.with_check.is_none());
    }

    #[test]
    fn restrictive_with_check_on_select_errors() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p ON app.docs FOR SELECT WITH CHECK (true);
        ";
        let err = parse_source(sql).expect_err("should fail");
        let msg = err.to_string();
        assert!(msg.contains("WITH CHECK"), "got: {msg}");
    }

    #[test]
    fn create_policy_on_unknown_table_errors() {
        let sql = "
            CREATE SCHEMA app;
            CREATE POLICY p ON app.missing_table USING (true);
        ";
        let err = parse_source(sql).expect_err("should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown table") || msg.contains("missing_table"),
            "got: {msg}"
        );
    }

    #[test]
    fn duplicate_policy_name_errors() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p ON app.docs USING (true);
            CREATE POLICY p ON app.docs USING (false);
        ";
        let err = parse_source(sql).expect_err("should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("more than once") || msg.contains('p'),
            "got: {msg}"
        );
    }

    #[test]
    fn alter_table_enable_rls() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
        ";
        let cat = parse_source(sql).expect("parses");
        let table = cat
            .tables
            .iter()
            .find(|t| t.qname.to_string() == "app.docs")
            .unwrap();
        assert!(table.rls_enabled, "rls_enabled should be true");
        assert!(!table.rls_forced, "rls_forced should be false");
    }

    #[test]
    fn alter_table_force_rls() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
        ";
        let cat = parse_source(sql).expect("parses");
        let table = cat
            .tables
            .iter()
            .find(|t| t.qname.to_string() == "app.docs")
            .unwrap();
        assert!(table.rls_forced, "rls_forced should be true");
    }

    #[test]
    fn alter_table_disable_after_enable() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
            ALTER TABLE app.docs DISABLE ROW LEVEL SECURITY;
        ";
        let cat = parse_source(sql).expect("parses");
        let table = cat
            .tables
            .iter()
            .find(|t| t.qname.to_string() == "app.docs")
            .unwrap();
        assert!(
            !table.rls_enabled,
            "rls_enabled should be false after disable"
        );
    }

    #[test]
    fn alter_policy_in_source_errors() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p ON app.docs USING (true);
            ALTER POLICY p ON app.docs USING (false);
        ";
        let err = parse_source(sql).expect_err("should fail");
        let msg = err.to_string();
        assert!(msg.contains("ALTER POLICY"), "got: {msg}");
    }

    #[test]
    fn drop_policy_in_source_errors() {
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY p ON app.docs USING (true);
            DROP POLICY p ON app.docs;
        ";
        let err = parse_source(sql).expect_err("should fail");
        let msg = err.to_string();
        assert!(msg.contains("DROP POLICY"), "got: {msg}");
    }

    #[test]
    fn restrictive_with_roles_round_trips() {
        // PostgreSQL syntax: AS clause follows ON table_name.
        let sql = "
            CREATE SCHEMA app;
            CREATE TABLE app.docs (id bigint);
            CREATE POLICY docs_write
                ON app.docs AS RESTRICTIVE FOR INSERT
                TO app_writer
                WITH CHECK (true);
        ";
        let cat = parse_source(sql).expect("parses");
        let table = cat
            .tables
            .iter()
            .find(|t| t.qname.to_string() == "app.docs")
            .unwrap();
        assert_eq!(table.policies.len(), 1);
        let p = &table.policies[0];
        assert!(!p.permissive);
        assert_eq!(p.command, PolicyCommand::Insert);
        assert_eq!(p.roles.len(), 1);
        assert!(
            matches!(&p.roles[0], GrantTarget::Role(r) if r.as_str() == "app_writer"),
            "got: {:?}",
            p.roles[0]
        );
        assert!(p.with_check.is_some());
    }
}
