//! Assemble `pg_policies` rows into [`Policy`] structs and attach to their tables.

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::grant::GrantTarget;
use crate::ir::policy::{Policy, PolicyCommand};
use crate::ir::table::Table;

use super::reparse_expression_text;

const Q: CatalogQuery = CatalogQuery::Policies;

/// Decode policy rows from `pg_policies` and push each onto the matching
/// [`Table`]'s policies list.
///
/// Rows whose table is not present in `tables` (i.e., the table is in a
/// managed schema but filtered out by an ignore glob, or the policy references
/// an unmanaged schema that slipped through) are silently dropped. Order
/// matches the SQL: `ORDER BY schemaname, tablename, policyname`.
pub(super) fn attach_policies(rows: &[Row], tables: &mut [Table]) -> Result<(), CatalogError> {
    for row in rows {
        let schema_str = row.get_text(Q, "schemaname")?;
        let table_str = row.get_text(Q, "tablename")?;

        let schema_ident =
            Identifier::from_unquoted(&schema_str).map_err(|e| CatalogError::BadColumnType {
                query: Q,
                column: "schemaname".to_string(),
                message: format!("invalid schema {schema_str:?}: {e}"),
            })?;
        let table_ident =
            Identifier::from_unquoted(&table_str).map_err(|e| CatalogError::BadColumnType {
                query: Q,
                column: "tablename".to_string(),
                message: format!("invalid table {table_str:?}: {e}"),
            })?;
        let qname = QualifiedName::new(schema_ident, table_ident);

        let Some(table) = tables.iter_mut().find(|t| t.qname == qname) else {
            // Table not in managed set; silently drop the policy.
            continue;
        };

        let policy = decode_policy(row)?;
        table.policies.push(policy);
    }
    Ok(())
}

fn decode_policy(row: &Row) -> Result<Policy, CatalogError> {
    let name_str = row.get_text(Q, "policyname")?;
    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: Q,
        column: "policyname".to_string(),
        message: format!("invalid policy name {name_str:?}: {e}"),
    })?;

    let permissive = row.get_bool(Q, "permissive")?;

    let cmd_str = row.get_text(Q, "cmd")?;
    let command =
        PolicyCommand::from_pg_text(&cmd_str).ok_or_else(|| CatalogError::BadColumnType {
            query: Q,
            column: "cmd".to_string(),
            message: format!("unknown PolicyCommand {cmd_str:?}"),
        })?;

    let role_strs = row.get_text_array(Q, "roles")?;
    let mut roles = Vec::with_capacity(role_strs.len());
    for role_str in role_strs {
        if role_str.eq_ignore_ascii_case("public") {
            roles.push(GrantTarget::Public);
        } else {
            let ident =
                Identifier::from_unquoted(&role_str).map_err(|e| CatalogError::BadColumnType {
                    query: Q,
                    column: "roles".to_string(),
                    message: format!("invalid role {role_str:?}: {e}"),
                })?;
            roles.push(GrantTarget::Role(ident));
        }
    }

    let using = row
        .get_opt_text(Q, "using_text")?
        .map(|text| reparse_expression_text(&text))
        .transpose()?;

    let with_check = row
        .get_opt_text(Q, "with_check_text")?
        .map(|text| reparse_expression_text(&text))
        .transpose()?;

    Ok(Policy {
        name,
        permissive,
        command,
        roles,
        using,
        with_check,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    struct PolicyRowSpec<'a> {
        schema: &'a str,
        table: &'a str,
        policy: &'a str,
        permissive: bool,
        cmd: &'a str,
        roles: &'a [&'a str],
        using: Option<&'a str>,
        with_check: Option<&'a str>,
    }

    fn policy_row(spec: &PolicyRowSpec<'_>) -> Row {
        let mut r = Row::new()
            .with("schemaname", Value::Text(spec.schema.to_string()))
            .with("tablename", Value::Text(spec.table.to_string()))
            .with("policyname", Value::Text(spec.policy.to_string()))
            .with("permissive", Value::Bool(spec.permissive))
            .with("cmd", Value::Text(spec.cmd.to_string()))
            .with(
                "roles",
                Value::TextArray(spec.roles.iter().map(|s| (*s).to_string()).collect()),
            );
        if let Some(u) = spec.using {
            r.insert("using_text", Value::Text(u.to_string()));
        } else {
            r.insert("using_text", Value::Null);
        }
        if let Some(w) = spec.with_check {
            r.insert("with_check_text", Value::Text(w.to_string()));
        } else {
            r.insert("with_check_text", Value::Null);
        }
        r
    }

    fn make_table(schema: &str, name: &str) -> Table {
        use crate::identifier::Identifier;
        Table {
            qname: QualifiedName::new(
                Identifier::from_unquoted(schema).unwrap(),
                Identifier::from_unquoted(name).unwrap(),
            ),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        }
    }

    #[test]
    fn attaches_policy_to_matching_table() {
        let mut tables = vec![make_table("app", "docs")];
        let rows = vec![policy_row(&PolicyRowSpec {
            schema: "app",
            table: "docs",
            policy: "author_only",
            permissive: true,
            cmd: "ALL",
            roles: &["public"],
            using: Some("(author = current_user)"),
            with_check: None,
        })];
        attach_policies(&rows, &mut tables).unwrap();
        assert_eq!(tables[0].policies.len(), 1);
        let p = &tables[0].policies[0];
        assert_eq!(p.name.as_str(), "author_only");
        assert!(p.permissive);
        assert_eq!(p.command, PolicyCommand::All);
        assert!(matches!(p.roles[0], GrantTarget::Public));
        assert!(p.using.is_some());
        assert!(p.with_check.is_none());
    }

    #[test]
    fn silently_drops_policy_for_unmanaged_table() {
        let mut tables = vec![make_table("app", "docs")];
        let rows = vec![policy_row(&PolicyRowSpec {
            schema: "app",
            table: "other",
            policy: "pol",
            permissive: true,
            cmd: "SELECT",
            roles: &["public"],
            using: None,
            with_check: None,
        })];
        attach_policies(&rows, &mut tables).unwrap();
        // "other" table is not in managed set; "docs" must stay policy-free.
        assert!(tables[0].policies.is_empty());
    }

    #[test]
    fn rejects_unknown_command() {
        let mut tables = vec![make_table("app", "docs")];
        let rows = vec![policy_row(&PolicyRowSpec {
            schema: "app",
            table: "docs",
            policy: "pol",
            permissive: true,
            cmd: "BOGUS",
            roles: &["public"],
            using: None,
            with_check: None,
        })];
        assert!(attach_policies(&rows, &mut tables).is_err());
    }
}
