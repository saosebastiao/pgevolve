//! Source-side parser for `CREATE DOMAIN`.
//!
//! Produces a [`UserType`] with `kind = UserTypeKind::Domain { ... }`.
//! Supported constraints: `NOT NULL`, `DEFAULT`, and named `CHECK`.
//! Unnamed `CHECK` constraints are rejected — explicit names are required for
//! stable diffing (mirrors the v0.1 table-check policy).
//! Collation is not modelled in v0.2.

use std::collections::BTreeSet;

use pg_query::NodeEnum;
use pg_query::protobuf::{ConstrType, CreateDomainStmt};

use crate::identifier::Identifier;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::user_type::{DomainCheck, UserType, UserTypeKind};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_expr;

/// Build a [`UserType`] with `kind = Domain` from a `CREATE DOMAIN` AST node.
///
/// * `default_schema` — filled in when the source omits the schema prefix
///   (e.g. a `-- @pgevolve schema=app` directive is in effect).
/// * Unnamed `CHECK` constraints are rejected with a clear error.
/// * Duplicate `CHECK` names within the same domain are rejected.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_domain(
    stmt: &CreateDomainStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<UserType, ParseError> {
    let qname = shared::qname_from_string_list(&stmt.domainname, default_schema, location)?;

    let type_name = stmt
        .type_name
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("CREATE DOMAIN {qname}: missing base type"),
        })?;
    let base = shared::type_name_to_column_type(type_name, location).map_err(|e| {
        // Rethrow with domain-qualified context if the message doesn't already
        // include the domain name.
        match e {
            ParseError::Structural {
                location: loc,
                message,
            } => ParseError::Structural {
                location: loc,
                message: format!("CREATE DOMAIN {qname}: {message}"),
            },
            other => other,
        }
    })?;

    let mut nullable = true;
    let mut default: Option<NormalizedExpr> = None;
    let mut checks: Vec<DomainCheck> = Vec::new();
    let mut seen_check_names: BTreeSet<String> = BTreeSet::new();

    for con_node in &stmt.constraints {
        let Some(NodeEnum::Constraint(c)) = con_node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("CREATE DOMAIN {qname}: unexpected constraint node"),
            });
        };
        let ctype = ConstrType::try_from(c.contype).map_err(|_| ParseError::Structural {
            location: location.clone(),
            message: format!(
                "CREATE DOMAIN {qname}: invalid constraint type code {}",
                c.contype
            ),
        })?;
        match ctype {
            ConstrType::ConstrNotnull => {
                nullable = false;
            }
            ConstrType::ConstrDefault => {
                let raw = c
                    .raw_expr
                    .as_ref()
                    .and_then(|r| r.node.as_ref())
                    .ok_or_else(|| ParseError::Structural {
                        location: location.clone(),
                        message: format!("CREATE DOMAIN {qname}: DEFAULT without expression"),
                    })?;
                default = Some(normalize_expr::from_pg_node(raw, Some(&base), location)?);
            }
            ConstrType::ConstrCheck => {
                if c.conname.is_empty() {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "CREATE DOMAIN {qname}: CHECK constraints must be explicitly named \
                             (use CONSTRAINT <name> CHECK (...))",
                        ),
                    });
                }
                let name =
                    Identifier::from_unquoted(&c.conname).map_err(|e| ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "CREATE DOMAIN {qname}: invalid CHECK name {:?} — {e}",
                            c.conname
                        ),
                    })?;
                if !seen_check_names.insert(name.as_str().to_string()) {
                    return Err(ParseError::Structural {
                        location: location.clone(),
                        message: format!(
                            "CREATE DOMAIN {qname}: duplicate CHECK name {:?}",
                            name.as_str(),
                        ),
                    });
                }
                let raw = c
                    .raw_expr
                    .as_ref()
                    .and_then(|r| r.node.as_ref())
                    .ok_or_else(|| ParseError::Structural {
                        location: location.clone(),
                        message: format!("CREATE DOMAIN {qname}: CHECK without expression"),
                    })?;
                let expression = normalize_expr::from_pg_node(raw, None, location)?;
                checks.push(DomainCheck { name, expression });
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!(
                        "CREATE DOMAIN {qname}: unsupported constraint type {other:?} \
                         (v0.2 supports NOT NULL, DEFAULT, CHECK)",
                    ),
                });
            }
        }
    }

    Ok(UserType {
        qname,
        kind: UserTypeKind::Domain {
            base,
            nullable,
            default,
            check_constraints: checks,
            collation: None, // v0.2 does not model domain collation
        },
        comment: None,
        owner: None,
        grants: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use pg_query::protobuf::CreateDomainStmt as PgCreateDomainStmt;

    use crate::ir::column_type::ColumnType;
    use crate::ir::user_type::UserTypeKind;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn parse_domain(sql: &str) -> PgCreateDomainStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateDomainStmt(boxed) = node else {
            panic!("expected CreateDomainStmt, got: {node:?}");
        };
        *boxed
    }

    fn build(sql: &str) -> UserType {
        let stmt = parse_domain(sql);
        build_domain(&stmt, None, &loc()).expect("build_domain")
    }

    #[test]
    fn qualified_domain_with_not_null_default_and_check() {
        let ut = build(
            "CREATE DOMAIN app.positive_int AS integer NOT NULL DEFAULT 1 \
             CONSTRAINT positive_int_check CHECK (VALUE > 0);",
        );
        assert_eq!(ut.qname.to_string(), "app.positive_int");
        let UserTypeKind::Domain {
            base,
            nullable,
            default,
            check_constraints,
            collation,
        } = ut.kind
        else {
            panic!("not domain kind");
        };
        assert_eq!(base, ColumnType::Integer);
        assert!(!nullable, "should be NOT NULL");
        assert!(default.is_some(), "should have DEFAULT");
        assert_eq!(check_constraints.len(), 1);
        assert_eq!(check_constraints[0].name.as_str(), "positive_int_check");
        assert!(
            check_constraints[0]
                .expression
                .canonical_text
                .contains("value"),
            "expression should reference VALUE (lowercased): {}",
            check_constraints[0].expression.canonical_text
        );
        assert!(collation.is_none());
        assert!(ut.comment.is_none());
    }

    #[test]
    fn bare_domain_is_nullable_no_default_no_checks() {
        let ut = build("CREATE DOMAIN app.email AS text;");
        assert_eq!(ut.qname.to_string(), "app.email");
        let UserTypeKind::Domain {
            base,
            nullable,
            default,
            check_constraints,
            collation,
        } = ut.kind
        else {
            panic!("not domain kind");
        };
        assert_eq!(base, ColumnType::Text);
        assert!(nullable, "should be nullable");
        assert!(default.is_none(), "should have no DEFAULT");
        assert!(check_constraints.is_empty(), "should have no CHECKs");
        assert!(collation.is_none());
    }

    #[test]
    fn unqualified_domain_uses_default_schema() {
        let stmt = parse_domain("CREATE DOMAIN email AS text;");
        let app = Identifier::from_unquoted("app").unwrap();
        let ut = build_domain(&stmt, Some(&app), &loc()).unwrap();
        assert_eq!(ut.qname.to_string(), "app.email");
    }

    #[test]
    fn unqualified_without_schema_errors() {
        let stmt = parse_domain("CREATE DOMAIN email AS text;");
        let err = build_domain(&stmt, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }

    #[test]
    fn unnamed_check_rejected() {
        let stmt = parse_domain("CREATE DOMAIN app.pos AS integer CHECK (VALUE > 0);");
        let err = build_domain(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(
            msg.contains("explicitly named"),
            "expected 'explicitly named' in: {msg}"
        );
    }

    #[test]
    fn duplicate_check_names_rejected() {
        let stmt = parse_domain(
            "CREATE DOMAIN app.d AS integer \
             CONSTRAINT chk CHECK (VALUE > 0) \
             CONSTRAINT chk CHECK (VALUE < 100);",
        );
        let err = build_domain(&stmt, None, &loc()).unwrap_err();
        let msg = match &err {
            ParseError::Structural { message, .. } => message.clone(),
            other => panic!("expected Structural, got {other:?}"),
        };
        assert!(msg.contains("duplicate"), "expected 'duplicate' in: {msg}");
        assert!(msg.contains("chk"), "expected constraint name in: {msg}");
    }
}
