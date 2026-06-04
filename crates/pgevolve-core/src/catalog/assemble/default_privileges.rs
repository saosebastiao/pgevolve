//! Decode `pg_default_acl` rows into [`DefaultPrivilegeRule`] entries.

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::Identifier;
use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};

/// Build [`DefaultPrivilegeRule`] entries from `pg_default_acl` rows.
///
/// Each row encodes one (`target_role`, schema, `object_type`) tuple. The `acl`
/// column is a `text[]` of `aclitem` strings, decoded via
/// [`crate::catalog::grants::decode_aclitem_array`].
///
/// No owner self-grant filtering is applied here — default-privilege rules
/// don't carry a per-rule "owner" to strip against.
pub(super) fn build_default_privileges(
    rows: &[Row],
) -> Result<Vec<DefaultPrivilegeRule>, CatalogError> {
    const Q: CatalogQuery = CatalogQuery::DefaultPrivileges;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let target_role_str = row.get_text(Q, "target_role")?;
        let target_role = Identifier::from_unquoted(&target_role_str).map_err(|e| {
            CatalogError::BadColumnType {
                query: Q,
                column: "target_role".to_string(),
                message: format!("invalid role name {target_role_str:?}: {e}"),
            }
        })?;

        let schema = row
            .get_opt_text(Q, "schema_name")?
            .map(|s| {
                Identifier::from_unquoted(&s).map_err(|e| CatalogError::BadColumnType {
                    query: Q,
                    column: "schema_name".to_string(),
                    message: format!("invalid schema name {s:?}: {e}"),
                })
            })
            .transpose()?;

        let object_type_str = row.get_text(Q, "object_type")?;
        let object_type_char =
            object_type_str
                .chars()
                .next()
                .ok_or_else(|| CatalogError::BadColumnType {
                    query: Q,
                    column: "object_type".to_string(),
                    message: "empty object_type".into(),
                })?;
        let object_type =
            DefaultPrivObjectType::from_pg_char(object_type_char).ok_or_else(|| {
                CatalogError::BadColumnType {
                    query: Q,
                    column: "object_type".to_string(),
                    message: format!("unknown defaclobjtype {object_type_char:?}"),
                }
            })?;

        let acl_strings = row.get_text_array(Q, "acl")?;
        let raw_grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;
        // Strip self-grants: `pg_default_acl.defaclacl` stores the COMPLETE
        // desired ACL for objects created by `target_role`, which includes the
        // target_role's own implicit privileges (e.g. `ops=X/ops` for EXECUTE
        // on functions). These self-grants are PG bookkeeping — they are never
        // declared in source IR and must not appear in the live catalog either.
        // Without this filter the live catalog perpetually diverges from source:
        //   source = `[readers/Execute]`
        //   live   = `[ops/Execute, readers/Execute]`
        // and the diff engine emits a spurious REVOKE that can delete the
        // entire `pg_default_acl` row, causing `present vs removed` failures.
        // (Analogous to `strip_owner_self_grants` for regular object grants.)
        let grants = crate::catalog::grants::strip_owner_self_grants(raw_grants, &target_role);

        out.push(DefaultPrivilegeRule {
            target_role,
            schema,
            object_type,
            grants,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::{Row, Value};

    fn row_for(target_role: &str, schema: Option<&str>, obj_type: &str, acl: &[&str]) -> Row {
        let acl_values: Vec<String> = acl.iter().map(|s| (*s).to_string()).collect();
        let mut r = Row::new()
            .with("target_role", Value::Text(target_role.to_string()))
            .with("object_type", Value::Text(obj_type.to_string()))
            .with("acl", Value::TextArray(acl_values));
        if let Some(s) = schema {
            r.insert("schema_name", Value::Text(s.to_string()));
        } else {
            r.insert("schema_name", Value::Null);
        }
        r
    }

    #[test]
    fn decodes_table_rule_with_schema() {
        let rows = vec![row_for(
            "app_owner",
            Some("app"),
            "r",
            &["readers=r/app_owner"],
        )];
        let rules = build_default_privileges(&rows).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].target_role.as_str(), "app_owner");
        assert_eq!(
            rules[0].schema.as_ref().map(Identifier::as_str),
            Some("app")
        );
        assert_eq!(rules[0].object_type, DefaultPrivObjectType::Tables);
        assert_eq!(rules[0].grants.len(), 1);
    }

    #[test]
    fn decodes_all_object_types() {
        for (ch, expected) in [
            ("r", DefaultPrivObjectType::Tables),
            ("S", DefaultPrivObjectType::Sequences),
            ("f", DefaultPrivObjectType::Functions),
            ("T", DefaultPrivObjectType::Types),
            ("n", DefaultPrivObjectType::Schemas),
        ] {
            let rows = vec![row_for("role1", None, ch, &[])];
            let rules = build_default_privileges(&rows).unwrap();
            assert_eq!(rules[0].object_type, expected, "char {ch:?}");
        }
    }

    #[test]
    fn schema_none_when_null() {
        let rows = vec![row_for("role1", None, "r", &[])];
        let rules = build_default_privileges(&rows).unwrap();
        assert!(rules[0].schema.is_none());
    }

    #[test]
    fn empty_acl_yields_no_grants() {
        let rows = vec![row_for("role1", Some("s"), "f", &[])];
        let rules = build_default_privileges(&rows).unwrap();
        assert!(rules[0].grants.is_empty());
    }

    #[test]
    fn unknown_object_type_errors() {
        let rows = vec![row_for("role1", None, "q", &[])];
        assert!(build_default_privileges(&rows).is_err());
    }

    /// Regression for issue #34: PG stores the `target_role`'s own privileges in
    /// `pg_default_acl.defaclacl` (e.g. `ops=X/ops` alongside `readers=X/ops`
    /// when you GRANT EXECUTE ON FUNCTIONS TO readers FOR ROLE ops). Without
    /// filtering, the live catalog perpetually diverges from source IR because
    /// source never declares these self-grants. The self-grants must be stripped
    /// at read time, just as `strip_owner_self_grants` strips them for regular
    /// object ACLs.
    #[test]
    fn strips_target_role_self_grant_from_acl() {
        // Simulate what PG stores in defaclacl after:
        //   ALTER DEFAULT PRIVILEGES FOR ROLE ops
        //     GRANT EXECUTE ON FUNCTIONS TO readers;
        // PG records: ops=X/ops (self-grant) AND readers=X/ops (explicit grant).
        let rows = vec![row_for("ops", None, "f", &["ops=X/ops", "readers=X/ops"])];
        let rules = build_default_privileges(&rows).unwrap();
        assert_eq!(rules.len(), 1);
        // The self-grant `ops=X/ops` must be stripped; only `readers/Execute` remains.
        assert_eq!(
            rules[0].grants.len(),
            1,
            "expected 1 grant (readers/Execute), got: {:?}",
            rules[0].grants
        );
        assert!(
            matches!(
                &rules[0].grants[0].grantee,
                crate::ir::grant::GrantTarget::Role(r) if r.as_str() == "readers"
            ),
            "remaining grant should be to readers, got: {:?}",
            rules[0].grants[0]
        );
    }

    /// Analogous regression test for schemas (USAGE + CREATE self-grant case).
    #[test]
    fn strips_target_role_self_grant_schemas() {
        // Simulate: ALTER DEFAULT PRIVILEGES FOR ROLE app GRANT CREATE ON SCHEMAS TO readers
        // PG stores: app=UC/app (self-grant), readers=C/app (explicit grant).
        let rows = vec![row_for("app", None, "n", &["app=UC/app", "readers=C/app"])];
        let rules = build_default_privileges(&rows).unwrap();
        assert_eq!(rules.len(), 1);
        // app=UC/app (self-grant) must be stripped; only readers/Create remains.
        assert_eq!(
            rules[0].grants.len(),
            1,
            "expected 1 grant (readers/Create), got: {:?}",
            rules[0].grants
        );
        let g = &rules[0].grants[0];
        assert!(
            matches!(&g.grantee, crate::ir::grant::GrantTarget::Role(r) if r.as_str() == "readers"),
            "remaining grant should be to readers, got: {g:?}",
        );
        assert_eq!(
            g.privilege,
            crate::ir::grant::Privilege::Create,
            "remaining grant should be Create"
        );
    }

    /// A non-self-grant from a role that happens to have the same name as the
    /// `target_role` but is a different grant (to a different grantee) must be kept.
    #[test]
    fn keeps_non_self_grants_intact() {
        // ops is the target_role. readers and ops are both grantees, but only
        // ops=X/ops is the self-grant. If ops appeared as a grantee in a
        // different privilege it would also be stripped (same name comparison).
        // This test confirms that non-self-grantees (readers) are always kept.
        let rows = vec![row_for("ops", None, "f", &["ops=X/ops", "readers=X/ops"])];
        let rules = build_default_privileges(&rows).unwrap();
        assert_eq!(rules[0].grants.len(), 1);
        assert!(
            matches!(
                &rules[0].grants[0].grantee,
                crate::ir::grant::GrantTarget::Role(r) if r.as_str() == "readers"
            ),
            "readers grant must be kept"
        );
    }
}
