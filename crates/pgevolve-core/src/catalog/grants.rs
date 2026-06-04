//! Decode PG `aclitem` text into `Grant` structs.
//!
//! `aclitem` text form: `grantee=privileges/grantor`. Empty grantee means
//! PUBLIC. Privilege letters: `r`=Select, `w`=Update, `a`=Insert, `d`=Delete,
//! `D`=Truncate, `x`=References, `t`=Trigger, `X`=Execute, `U`=Usage,
//! `C`=Create. An asterisk after a letter marks `WITH GRANT OPTION`
//! (e.g., `r*` = SELECT WITH GRANT OPTION).

use crate::catalog::error::CatalogError;
use crate::identifier::Identifier;
use crate::ir::grant::{Grant, GrantTarget, Privilege};

/// Strip PG-implicit PUBLIC grants for object types that carry them by default.
///
/// Postgres automatically grants:
/// - `USAGE` on `TYPES` to `PUBLIC`
/// - `EXECUTE` on `FUNCTIONS` (and `PROCEDURES`) to `PUBLIC`
///
/// These entries always appear in `pg_default_acl.defaclacl` when any explicit
/// grant exists on such a rule, regardless of user intent. They cannot be
/// revoked by users (PG re-adds them). Carrying them into source or live IR
/// would cause a permanent divergence: the source author never writes them, but
/// the live catalog reader always sees them.
///
/// Only the plain (non–WGO) form is implicit: `WITH GRANT OPTION` variants
/// (`Public/Usage wgo=true`, `Public/Execute wgo=true`) are always user-declared
/// and must be preserved.
///
/// This stripping is symmetrical: both the catalog reader and the source parser
/// apply it so that source IR and live IR remain in the same canonical form.
#[must_use]
pub fn strip_public_implicit_grants(
    grants: Vec<Grant>,
    object_type: crate::ir::default_privileges::DefaultPrivObjectType,
) -> Vec<Grant> {
    use crate::ir::default_privileges::DefaultPrivObjectType;
    grants
        .into_iter()
        .filter(|g| {
            if g.with_grant_option {
                return true; // WGO variants are always user-declared.
            }
            match (&g.grantee, g.privilege, object_type) {
                // PG-implicit PUBLIC grants: Usage on TYPES, Execute on FUNCTIONS
                // (PROCEDURES map to Functions in pgevolve). Both are always
                // present in pg_default_acl.defaclacl and are never user-revocable.
                (GrantTarget::Public, Privilege::Usage, DefaultPrivObjectType::Types)
                | (GrantTarget::Public, Privilege::Execute, DefaultPrivObjectType::Functions) => {
                    false
                }
                _ => true,
            }
        })
        .collect()
}

/// Strip grants whose grantee equals the object owner.
///
/// PG's `relacl` materializes owner self-grants (e.g. `app_owner=arwdDxt/app_owner`)
/// whenever any explicit GRANT exists on an object. These are redundant with the
/// owner relationship and would cause spurious diffs if carried in our IR: source
/// authors who write only `ALTER TABLE t OWNER TO app_owner;` (no explicit grants)
/// would see the plan demand `REVOKE` against the owner, and the `revoke-from-owner`
/// lint (Stage 11) would then error the plan.
///
/// `PUBLIC` grants are never considered owner self-grants and are always kept.
#[must_use]
pub fn strip_owner_self_grants(grants: Vec<Grant>, owner: &Identifier) -> Vec<Grant> {
    grants
        .into_iter()
        .filter(|g| match &g.grantee {
            GrantTarget::Role(name) => name != owner,
            GrantTarget::Public => true,
        })
        .collect()
}

/// Decode an array of aclitem strings into `Grant` entries.
///
/// `columns: None` for object-level; caller is responsible for marking
/// column-level grants with `Some(vec![colname])` when decoding
/// `pg_attribute.attacl`.
pub fn decode_aclitem_array(items: &[String]) -> Result<Vec<Grant>, CatalogError> {
    let mut out = Vec::with_capacity(items.len());
    for raw in items {
        out.extend(decode_one(raw)?);
    }
    Ok(out)
}

fn decode_one(raw: &str) -> Result<Vec<Grant>, CatalogError> {
    let body = raw
        .split('/')
        .next()
        .ok_or_else(|| CatalogError::BadColumnType {
            query: crate::catalog::CatalogQuery::Schemas,
            column: "acl".to_string(),
            message: format!("malformed aclitem {raw:?}"),
        })?;
    let (grantee_str, privs) = body
        .split_once('=')
        .ok_or_else(|| CatalogError::BadColumnType {
            query: crate::catalog::CatalogQuery::Schemas,
            column: "acl".to_string(),
            message: format!("malformed aclitem {raw:?}"),
        })?;

    let grantee = if grantee_str.is_empty() {
        GrantTarget::Public
    } else {
        // Strip a single pair of surrounding double-quotes for quoted role names.
        let trimmed = grantee_str
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(grantee_str);
        GrantTarget::Role(Identifier::from_unquoted(trimmed).map_err(|e| {
            CatalogError::BadColumnType {
                query: crate::catalog::CatalogQuery::Schemas,
                column: "acl".to_string(),
                message: format!("aclitem grantee {grantee_str:?}: {e}"),
            }
        })?)
    };

    let mut out = Vec::new();
    let mut chars = privs.chars().peekable();
    while let Some(c) = chars.next() {
        let parsed_privilege = match c {
            'r' => Privilege::Select,
            'w' => Privilege::Update,
            'a' => Privilege::Insert,
            'd' => Privilege::Delete,
            'D' => Privilege::Truncate,
            'x' => Privilege::References,
            't' => Privilege::Trigger,
            'X' => Privilege::Execute,
            'U' => Privilege::Usage,
            'C' => Privilege::Create,
            // Privilege letters pgevolve doesn't manage at this layer:
            //   'T' (TEMPORARY on database)
            //   'c' (CONNECT on database)
            //   's' (SET on parameter)
            //   'A' (ALTER SYSTEM on parameter)
            // Silently skip and consume any trailing '*'.
            _ => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                }
                continue;
            }
        };
        let with_grant_option = chars.peek() == Some(&'*');
        if with_grant_option {
            chars.next();
        }
        out.push(Grant {
            grantee: grantee.clone(),
            privilege: parsed_privilege,
            with_grant_option,
            columns: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_select() {
        let g = decode_one("=r/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert!(matches!(g[0].grantee, GrantTarget::Public));
        assert_eq!(g[0].privilege, Privilege::Select);
        assert!(!g[0].with_grant_option);
    }

    #[test]
    fn role_multiple_privileges() {
        let g = decode_one("alice=arwd/owner").unwrap();
        assert_eq!(g.len(), 4);
        let privs: Vec<Privilege> = g.iter().map(|x| x.privilege).collect();
        assert!(privs.contains(&Privilege::Insert));
        assert!(privs.contains(&Privilege::Select));
        assert!(privs.contains(&Privilege::Update));
        assert!(privs.contains(&Privilege::Delete));
    }

    #[test]
    fn with_grant_option_flag() {
        let g = decode_one("alice=r*/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert!(g[0].with_grant_option);
    }

    #[test]
    fn unmanaged_privileges_skipped() {
        let g = decode_one("alice=Tr/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].privilege, Privilege::Select);
    }

    #[test]
    fn malformed_aclitem_errors() {
        assert!(decode_one("no_equals_sign").is_err());
    }

    #[test]
    fn array_decode_combines() {
        let arr = vec!["alice=r/o".to_string(), "=a/o".to_string()];
        let g = decode_aclitem_array(&arr).unwrap();
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn strip_owner_self_grants_removes_owner_entries() {
        let owner = Identifier::from_unquoted("app_owner").unwrap();
        let grants = vec![
            Grant {
                grantee: GrantTarget::Role(Identifier::from_unquoted("app_owner").unwrap()),
                privilege: Privilege::Select,
                with_grant_option: false,
                columns: None,
            },
            Grant {
                grantee: GrantTarget::Role(Identifier::from_unquoted("readers").unwrap()),
                privilege: Privilege::Select,
                with_grant_option: false,
                columns: None,
            },
        ];
        let filtered = strip_owner_self_grants(grants, &owner);
        assert_eq!(filtered.len(), 1);
        assert!(matches!(&filtered[0].grantee, GrantTarget::Role(r) if r.as_str() == "readers"));
    }

    #[test]
    fn strip_owner_self_grants_keeps_public() {
        let owner = Identifier::from_unquoted("app_owner").unwrap();
        let grants = vec![Grant {
            grantee: GrantTarget::Public,
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: None,
        }];
        let filtered = strip_owner_self_grants(grants, &owner);
        assert_eq!(filtered.len(), 1, "PUBLIC grants are not owner self-grants");
    }

    #[test]
    fn all_managed_privilege_letters() {
        // r=Select, w=Update, a=Insert, d=Delete, D=Truncate,
        // x=References, t=Trigger, X=Execute, U=Usage, C=Create
        let g = decode_one("alice=rwadDxtXUC/owner").unwrap();
        assert_eq!(g.len(), 10, "all 10 managed privilege letters");
    }

    #[test]
    fn strip_public_implicit_grants_removes_types_usage() {
        use crate::ir::default_privileges::DefaultPrivObjectType;
        let grants = vec![
            Grant {
                grantee: GrantTarget::Public,
                privilege: Privilege::Usage,
                with_grant_option: false,
                columns: None,
            },
            Grant {
                grantee: GrantTarget::Role(Identifier::from_unquoted("readers").unwrap()),
                privilege: Privilege::Usage,
                with_grant_option: false,
                columns: None,
            },
        ];
        let filtered = strip_public_implicit_grants(grants, DefaultPrivObjectType::Types);
        assert_eq!(filtered.len(), 1);
        assert!(matches!(&filtered[0].grantee, GrantTarget::Role(r) if r.as_str() == "readers"));
    }

    #[test]
    fn strip_public_implicit_grants_removes_functions_execute() {
        use crate::ir::default_privileges::DefaultPrivObjectType;
        let grants = vec![
            Grant {
                grantee: GrantTarget::Public,
                privilege: Privilege::Execute,
                with_grant_option: false,
                columns: None,
            },
            Grant {
                grantee: GrantTarget::Role(Identifier::from_unquoted("app").unwrap()),
                privilege: Privilege::Execute,
                with_grant_option: false,
                columns: None,
            },
        ];
        let filtered = strip_public_implicit_grants(grants, DefaultPrivObjectType::Functions);
        assert_eq!(filtered.len(), 1);
        assert!(matches!(&filtered[0].grantee, GrantTarget::Role(r) if r.as_str() == "app"));
    }

    #[test]
    fn strip_public_implicit_grants_keeps_wgo_variants() {
        use crate::ir::default_privileges::DefaultPrivObjectType;
        // Public/Usage WGO on TYPES must NOT be stripped.
        let grants = vec![Grant {
            grantee: GrantTarget::Public,
            privilege: Privilege::Usage,
            with_grant_option: true, // WGO — user-declared, never implicit
            columns: None,
        }];
        let filtered = strip_public_implicit_grants(grants, DefaultPrivObjectType::Types);
        assert_eq!(filtered.len(), 1, "WGO Public/Usage on TYPES must be kept");
    }

    #[test]
    fn strip_public_implicit_grants_leaves_tables_untouched() {
        use crate::ir::default_privileges::DefaultPrivObjectType;
        // Tables have no implicit PUBLIC grant — nothing should be stripped.
        let grants = vec![Grant {
            grantee: GrantTarget::Public,
            privilege: Privilege::Select,
            with_grant_option: false,
            columns: None,
        }];
        let filtered = strip_public_implicit_grants(grants, DefaultPrivObjectType::Tables);
        assert_eq!(filtered.len(), 1, "Tables have no implicit PUBLIC grants");
    }

    #[test]
    fn unmanaged_with_grant_option_skipped_cleanly() {
        // 'T' is TEMPORARY (unmanaged), 'T*' should skip the '*' too.
        let g = decode_one("alice=T*r/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].privilege, Privilege::Select);
    }
}
