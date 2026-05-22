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
    fn all_managed_privilege_letters() {
        // r=Select, w=Update, a=Insert, d=Delete, D=Truncate,
        // x=References, t=Trigger, X=Execute, U=Usage, C=Create
        let g = decode_one("alice=rwadDxtXUC/owner").unwrap();
        assert_eq!(g.len(), 10, "all 10 managed privilege letters");
    }

    #[test]
    fn unmanaged_with_grant_option_skipped_cleanly() {
        // 'T' is TEMPORARY (unmanaged), 'T*' should skip the '*' too.
        let g = decode_one("alice=T*r/owner").unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].privilege, Privilege::Select);
    }
}
