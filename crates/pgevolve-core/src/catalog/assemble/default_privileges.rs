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
        let grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;
        // Note: default-priv rules don't have a per-rule "owner" to strip
        // against, so no owner-self-grant filtering here.

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
}
