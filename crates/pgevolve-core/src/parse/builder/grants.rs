//! `GRANT priv ON obj TO grantee` — object-level grants.
//!
//! Only `GRANT` (not `REVOKE`) is accepted in source SQL; revocations come from
//! diff. `GRANT ALL` expands to the explicit privilege list applicable to the
//! object kind. Column-level grants (`GRANT SELECT (col) ON TABLE`) are
//! supported for tables, views, and materialized views.
//!
//! Unmanaged object kinds (DATABASE, TABLESPACE, LANGUAGE, FOREIGN TABLE,
//! LARGE OBJECT) raise [`ParseError::Structural`].

use pg_query::NodeEnum;
use pg_query::protobuf::{GrantStmt, GrantTargetType, ObjectType, RoleSpecType};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::grant::{Grant, GrantTarget, Privilege};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply a `GRANT` statement to the catalog by pushing [`Grant`] entries onto
/// the matching object's `grants` field.
///
/// REVOKE → [`ParseError::Structural`]. Unmanaged object kinds → error.
/// Non-OBJECT target types (ALL IN SCHEMA, DEFAULTS) → error (DEFAULTS arrives
/// via [`super::default_privileges`] instead).
#[allow(clippy::too_many_lines)]
pub(crate) fn apply(
    s: &GrantStmt,
    cat: &mut Catalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    // Reject REVOKE.
    if !s.is_grant {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "REVOKE in source DDL is not supported — revocations are produced by diff; \
                      remove the REVOKE statement from your source files"
                .into(),
        });
    }

    // Reject GRANTED BY — too complex for v0.3.1; the grantor field must be
    // empty (the zero-value RoleSpec produced by protobuf when the clause is
    // absent has roletype == Undefined and rolename == "").
    if let Some(ref grantor) = s.grantor {
        let roletype = pg_query::protobuf::RoleSpecType::try_from(grantor.roletype)
            .unwrap_or(RoleSpecType::Undefined);
        if roletype != RoleSpecType::Undefined || !grantor.rolename.is_empty() {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: "GRANT ... GRANTED BY is not supported in source DDL (v0.3.1)".into(),
            });
        }
    }

    // Only OBJECT targets (ACL_TARGET_OBJECT). Reject ALL IN SCHEMA and
    // DEFAULTS (the latter arrives via AlterDefaultPrivilegesStmt instead).
    let targtype = GrantTargetType::try_from(s.targtype).unwrap_or(GrantTargetType::Undefined);
    if !matches!(targtype, GrantTargetType::AclTargetObject) {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "only OBJECT-level GRANT is supported in source DDL; \
                 got target type {targtype:?} (ALL IN SCHEMA and DEFAULT PRIVILEGES \
                 are handled separately)"
            ),
        });
    }

    let objtype = ObjectType::try_from(s.objtype).unwrap_or(ObjectType::Undefined);

    // Privileges — empty list means GRANT ALL; expand per object kind.
    let privs = decode_privileges(s, objtype, loc)?;

    // Grantees.
    let grantees = decode_grantees(&s.grantees, loc)?;

    // Dispatch by object type.
    match objtype {
        ObjectType::ObjectTable | ObjectType::ObjectView | ObjectType::ObjectMatview => {
            for obj_node in &s.objects {
                let rv = range_var_from_node(obj_node, loc, "table/view/MV")?;
                let qname = shared::resolve_qname(rv, None, loc)?;
                attach_relation_grants(
                    cat,
                    &qname,
                    objtype,
                    &grantees,
                    &privs,
                    s.grant_option,
                    loc,
                )?;
            }
        }
        ObjectType::ObjectSequence => {
            for obj_node in &s.objects {
                let rv = range_var_from_node(obj_node, loc, "sequence")?;
                let qname = shared::resolve_qname(rv, None, loc)?;
                let seq = cat
                    .sequences
                    .iter_mut()
                    .find(|sq| sq.qname == qname)
                    .ok_or_else(|| missing(loc, "sequence", &qname.to_string()))?;
                for grantee in &grantees {
                    for pc in &privs {
                        seq.grants.push(Grant {
                            grantee: grantee.clone(),
                            privilege: pc.privilege,
                            with_grant_option: s.grant_option,
                            columns: None,
                        });
                    }
                }
            }
        }
        ObjectType::ObjectSchema => {
            for obj_node in &s.objects {
                let schema_name = schema_name_from_node(obj_node, loc)?;
                let schema = cat
                    .schemas
                    .iter_mut()
                    .find(|sc| sc.name == schema_name)
                    .ok_or_else(|| missing(loc, "schema", schema_name.as_str()))?;
                for grantee in &grantees {
                    for pc in &privs {
                        schema.grants.push(Grant {
                            grantee: grantee.clone(),
                            privilege: pc.privilege,
                            with_grant_option: s.grant_option,
                            columns: None,
                        });
                    }
                }
            }
        }
        ObjectType::ObjectFunction => {
            for obj_node in &s.objects {
                let owa = obj_with_args_from_node(obj_node, loc, "function")?;
                let qname = shared::qname_from_string_list(&owa.objname, None, loc)?;
                let func = cat
                    .functions
                    .iter_mut()
                    .find(|f| f.qname == qname)
                    .ok_or_else(|| missing(loc, "function", &qname.to_string()))?;
                for grantee in &grantees {
                    for pc in &privs {
                        func.grants.push(Grant {
                            grantee: grantee.clone(),
                            privilege: pc.privilege,
                            with_grant_option: s.grant_option,
                            columns: None,
                        });
                    }
                }
            }
        }
        ObjectType::ObjectProcedure | ObjectType::ObjectRoutine => {
            for obj_node in &s.objects {
                let owa = obj_with_args_from_node(obj_node, loc, "procedure")?;
                let qname = shared::qname_from_string_list(&owa.objname, None, loc)?;
                let proc = cat
                    .procedures
                    .iter_mut()
                    .find(|p| p.qname == qname)
                    .ok_or_else(|| missing(loc, "procedure", &qname.to_string()))?;
                for grantee in &grantees {
                    for pc in &privs {
                        proc.grants.push(Grant {
                            grantee: grantee.clone(),
                            privilege: pc.privilege,
                            with_grant_option: s.grant_option,
                            columns: None,
                        });
                    }
                }
            }
        }
        ObjectType::ObjectType => {
            for obj_node in &s.objects {
                let qname = type_qname_from_node(obj_node, loc)?;
                let ty = cat
                    .types
                    .iter_mut()
                    .find(|t| t.qname == qname)
                    .ok_or_else(|| missing(loc, "type", &qname.to_string()))?;
                for grantee in &grantees {
                    for pc in &privs {
                        ty.grants.push(Grant {
                            grantee: grantee.clone(),
                            privilege: pc.privilege,
                            with_grant_option: s.grant_option,
                            columns: None,
                        });
                    }
                }
            }
        }
        ObjectType::ObjectDatabase
        | ObjectType::ObjectTablespace
        | ObjectType::ObjectLanguage
        | ObjectType::ObjectForeignTable
        | ObjectType::ObjectLargeobject => {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "GRANT on {objtype:?} is not managed by pgevolve; \
                     only TABLE, VIEW, MATERIALIZED VIEW, SEQUENCE, SCHEMA, \
                     FUNCTION, PROCEDURE, and TYPE grants are supported"
                ),
            });
        }
        other => {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "unsupported GRANT object type {other:?} — pgevolve does not manage \
                     grants on this object kind"
                ),
            });
        }
    }
    Ok(())
}

// ─── Privilege decoding ──────────────────────────────────────────────────────

/// Decode the `privileges` list from a `GrantStmt`.
///
/// An empty list means `GRANT ALL` — expand to the full set of privileges
/// applicable to `objtype`. Per-privilege `cols` are decoded and attached to
/// the grant (column-level grants).
///
/// Returns a flat list of `(privilege, columns)` pairs — one per explicit or
/// expanded privilege entry.
fn decode_privileges(
    s: &GrantStmt,
    objtype: ObjectType,
    loc: &SourceLocation,
) -> Result<Vec<PrivilegeWithCols>, ParseError> {
    if s.privileges.is_empty() {
        // GRANT ALL — expand per object type.
        let all = all_privs_for(objtype, loc)?;
        return Ok(all
            .into_iter()
            .map(|p| PrivilegeWithCols {
                privilege: p,
                columns: None,
            })
            .collect());
    }
    let mut out = Vec::new();
    for node in &s.privileges {
        let Some(NodeEnum::AccessPriv(ap)) = node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "expected AccessPriv node in GRANT privileges list, got {:?}",
                    node.node.as_ref().map(std::mem::discriminant)
                ),
            });
        };
        // Empty priv_name in AccessPriv means ALL (shouldn't happen when the
        // list is non-empty, but handle gracefully).
        if ap.priv_name.is_empty() {
            let all = all_privs_for(objtype, loc)?;
            let cols = decode_columns(&ap.cols, loc)?;
            for p in all {
                out.push(PrivilegeWithCols {
                    privilege: p,
                    columns: cols.clone(),
                });
            }
            continue;
        }
        let pv = priv_from_keyword(&ap.priv_name, loc)?;
        let cols = decode_columns(&ap.cols, loc)?;
        out.push(PrivilegeWithCols {
            privilege: pv,
            columns: cols,
        });
    }
    Ok(out)
}

/// Column names from an `AccessPriv.cols` list.
fn decode_columns(
    nodes: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<Option<Vec<Identifier>>, ParseError> {
    if nodes.is_empty() {
        return Ok(None);
    }
    let mut cols = Vec::with_capacity(nodes.len());
    for n in nodes {
        let Some(NodeEnum::String(s)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "expected String node for column name in GRANT privileges, got {:?}",
                    n.node.as_ref().map(std::mem::discriminant)
                ),
            });
        };
        cols.push(shared::ident(&s.sval, loc)?);
    }
    Ok(Some(cols))
}

/// Parse a SQL privilege keyword into [`Privilege`].
fn priv_from_keyword(kw: &str, loc: &SourceLocation) -> Result<Privilege, ParseError> {
    match kw.to_ascii_uppercase().as_str() {
        "SELECT" => Ok(Privilege::Select),
        "INSERT" => Ok(Privilege::Insert),
        "UPDATE" => Ok(Privilege::Update),
        "DELETE" => Ok(Privilege::Delete),
        "TRUNCATE" => Ok(Privilege::Truncate),
        "REFERENCES" => Ok(Privilege::References),
        "TRIGGER" => Ok(Privilege::Trigger),
        "USAGE" => Ok(Privilege::Usage),
        "EXECUTE" => Ok(Privilege::Execute),
        "CREATE" => Ok(Privilege::Create),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("unknown privilege keyword '{other}'"),
        }),
    }
}

/// The complete set of privileges applicable to an object kind.
///
/// Used when expanding `GRANT ALL`.
fn all_privs_for(objtype: ObjectType, loc: &SourceLocation) -> Result<Vec<Privilege>, ParseError> {
    match objtype {
        ObjectType::ObjectTable | ObjectType::ObjectView | ObjectType::ObjectMatview => Ok(vec![
            Privilege::Select,
            Privilege::Insert,
            Privilege::Update,
            Privilege::Delete,
            Privilege::Truncate,
            Privilege::References,
            Privilege::Trigger,
        ]),
        ObjectType::ObjectSequence => {
            Ok(vec![Privilege::Usage, Privilege::Select, Privilege::Update])
        }
        ObjectType::ObjectSchema => Ok(vec![Privilege::Usage, Privilege::Create]),
        ObjectType::ObjectFunction | ObjectType::ObjectProcedure | ObjectType::ObjectRoutine => {
            Ok(vec![Privilege::Execute])
        }
        ObjectType::ObjectType => Ok(vec![Privilege::Usage]),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!("GRANT ALL on {other:?}: no known privilege expansion for this type"),
        }),
    }
}

// ─── Grantee decoding ────────────────────────────────────────────────────────

/// Decode the `grantees` list from a `GrantStmt` into [`GrantTarget`] values.
fn decode_grantees(
    nodes: &[pg_query::protobuf::Node],
    loc: &SourceLocation,
) -> Result<Vec<GrantTarget>, ParseError> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let Some(NodeEnum::RoleSpec(rs)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: loc.clone(),
                message: format!(
                    "expected RoleSpec node in GRANT grantees list, got {:?}",
                    n.node.as_ref().map(std::mem::discriminant)
                ),
            });
        };
        let roletype = RoleSpecType::try_from(rs.roletype).unwrap_or(RoleSpecType::Undefined);
        let target = if roletype == RoleSpecType::RolespecPublic {
            GrantTarget::Public
        } else {
            let id = shared::ident(&rs.rolename, loc)?;
            GrantTarget::Role(id)
        };
        out.push(target);
    }
    Ok(out)
}

// ─── Object extraction helpers ───────────────────────────────────────────────

/// Extract a `RangeVar` reference from a `Node` (for table/view/MV/sequence).
fn range_var_from_node<'a>(
    node: &'a pg_query::protobuf::Node,
    loc: &SourceLocation,
    kind: &'static str,
) -> Result<&'a pg_query::protobuf::RangeVar, ParseError> {
    match node.node.as_ref() {
        Some(NodeEnum::RangeVar(rv)) => Ok(rv),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected RangeVar for {kind} in GRANT objects list, got {:?}",
                other.map(std::mem::discriminant)
            ),
        }),
    }
}

/// Extract an `ObjectWithArgs` from a `Node` (for functions/procedures).
fn obj_with_args_from_node<'a>(
    node: &'a pg_query::protobuf::Node,
    loc: &SourceLocation,
    kind: &'static str,
) -> Result<&'a pg_query::protobuf::ObjectWithArgs, ParseError> {
    match node.node.as_ref() {
        Some(NodeEnum::ObjectWithArgs(owa)) => Ok(owa),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected ObjectWithArgs for {kind} in GRANT objects list, got {:?}",
                other.map(std::mem::discriminant)
            ),
        }),
    }
}

/// Extract a schema `Identifier` from a `Node` (schemas are bare String nodes).
fn schema_name_from_node(
    node: &pg_query::protobuf::Node,
    loc: &SourceLocation,
) -> Result<Identifier, ParseError> {
    match node.node.as_ref() {
        Some(NodeEnum::String(s)) => shared::ident(&s.sval, loc),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected String node for schema name in GRANT objects list, got {:?}",
                other.map(std::mem::discriminant)
            ),
        }),
    }
}

/// Extract a `QualifiedName` from a `Node` for type objects.
///
/// `GRANT ... ON TYPE schema.name` arrives as a `TypeName` node.
fn type_qname_from_node(
    node: &pg_query::protobuf::Node,
    loc: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    match node.node.as_ref() {
        Some(NodeEnum::TypeName(tn)) => shared::qname_from_string_list(&tn.names, None, loc),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected TypeName node for type in GRANT objects list, got {:?}",
                other.map(std::mem::discriminant)
            ),
        }),
    }
}

// ─── Relation grant helper ───────────────────────────────────────────────────

/// A privilege with optional column restriction (for column-level grants).
struct PrivilegeWithCols {
    privilege: Privilege,
    columns: Option<Vec<Identifier>>,
}

/// Attach grants to a table, view, or materialized view by qname.
///
/// Tries each collection in turn (table → view → MV). Errors if the qname
/// does not resolve to any of them.
#[allow(clippy::too_many_arguments)]
fn attach_relation_grants(
    cat: &mut Catalog,
    qname: &QualifiedName,
    objtype: ObjectType,
    grantees: &[GrantTarget],
    privs: &[PrivilegeWithCols],
    grant_option: bool,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    // Tables.
    if let Some(tbl) = cat.tables.iter_mut().find(|t| &t.qname == qname) {
        for grantee in grantees {
            for pc in privs {
                tbl.grants.push(Grant {
                    grantee: grantee.clone(),
                    privilege: pc.privilege,
                    with_grant_option: grant_option,
                    columns: pc.columns.clone(),
                });
            }
        }
        return Ok(());
    }
    // Views.
    if let Some(view) = cat.views.iter_mut().find(|v| &v.qname == qname) {
        for grantee in grantees {
            for pc in privs {
                view.grants.push(Grant {
                    grantee: grantee.clone(),
                    privilege: pc.privilege,
                    with_grant_option: grant_option,
                    columns: pc.columns.clone(),
                });
            }
        }
        return Ok(());
    }
    // Materialized views.
    if let Some(mv) = cat
        .materialized_views
        .iter_mut()
        .find(|m| &m.qname == qname)
    {
        for grantee in grantees {
            for pc in privs {
                mv.grants.push(Grant {
                    grantee: grantee.clone(),
                    privilege: pc.privilege,
                    with_grant_option: grant_option,
                    columns: pc.columns.clone(),
                });
            }
        }
        return Ok(());
    }
    let kind = match objtype {
        ObjectType::ObjectView => "view",
        ObjectType::ObjectMatview => "materialized view",
        _ => "table",
    };
    Err(missing(loc, kind, &qname.to_string()))
}

fn missing(loc: &SourceLocation, kind: &str, name: &str) -> ParseError {
    ParseError::Structural {
        location: loc.clone(),
        message: format!("GRANT references {kind} {name} which is not defined in source"),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::schema::Schema;
    use crate::ir::sequence::Sequence;
    use crate::ir::table::Table;
    use crate::ir::view::View;
    use crate::parse::normalize_body::NormalizedBody;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn seed_catalog() -> Catalog {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                Column {
                    name: id("id"),
                    ty: ColumnType::Integer,
                    nullable: false,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    storage: None,
                    compression: None,
                    comment: None,
                },
                Column {
                    name: id("email"),
                    ty: ColumnType::Text,
                    nullable: true,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    storage: None,
                    compression: None,
                    comment: None,
                },
            ],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        c.sequences.push(Sequence {
            qname: qn("app", "seq1"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
            owner: None,
            grants: vec![],
        });
        c.views.push(View {
            qname: qn("app", "v1"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            raw_body: "SELECT 1".into(),
            comment: None,
            owner: None,
            grants: vec![],
        });
        c
    }

    fn parse_grant(sql: &str) -> GrantStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::GrantStmt(s) = stmt else {
            panic!("not GrantStmt, got something else")
        };
        s
    }

    #[test]
    fn basic_table_grant_select() {
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT SELECT ON TABLE app.users TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        let grants = &cat.tables[0].grants;
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].privilege, Privilege::Select);
        assert_eq!(grants[0].grantee, GrantTarget::Role(id("alice")));
        assert!(!grants[0].with_grant_option);
        assert!(grants[0].columns.is_none());
    }

    #[test]
    fn grant_all_on_table_expands() {
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT ALL ON TABLE app.users TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        // SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER = 7
        assert_eq!(cat.tables[0].grants.len(), 7);
        let privs: Vec<Privilege> = cat.tables[0].grants.iter().map(|g| g.privilege).collect();
        assert!(privs.contains(&Privilege::Select));
        assert!(privs.contains(&Privilege::Insert));
        assert!(privs.contains(&Privilege::Update));
        assert!(privs.contains(&Privilege::Delete));
        assert!(privs.contains(&Privilege::Truncate));
        assert!(privs.contains(&Privilege::References));
        assert!(privs.contains(&Privilege::Trigger));
    }

    #[test]
    fn column_level_grant() {
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT SELECT (id, email) ON TABLE app.users TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        let grants = &cat.tables[0].grants;
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].privilege, Privilege::Select);
        let cols = grants[0].columns.as_ref().expect("should have columns");
        assert_eq!(cols.len(), 2);
        assert!(cols.contains(&id("id")));
        assert!(cols.contains(&id("email")));
    }

    #[test]
    fn grant_to_public() {
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT SELECT ON TABLE app.users TO PUBLIC;");
        apply(&s, &mut cat, &loc()).unwrap();
        let grants = &cat.tables[0].grants;
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].grantee, GrantTarget::Public);
    }

    #[test]
    fn multi_privilege_expansion() {
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT SELECT, INSERT ON TABLE app.users TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        // 2 privileges × 1 grantee = 2 entries
        assert_eq!(cat.tables[0].grants.len(), 2);
        let privs: Vec<Privilege> = cat.tables[0].grants.iter().map(|g| g.privilege).collect();
        assert!(privs.contains(&Privilege::Select));
        assert!(privs.contains(&Privilege::Insert));
    }

    #[test]
    fn multi_grantee_expansion() {
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT SELECT ON TABLE app.users TO alice, bob;");
        apply(&s, &mut cat, &loc()).unwrap();
        // 1 privilege × 2 grantees = 2 entries
        assert_eq!(cat.tables[0].grants.len(), 2);
        let grantees: Vec<GrantTarget> = cat.tables[0]
            .grants
            .iter()
            .map(|g| g.grantee.clone())
            .collect();
        assert!(grantees.contains(&GrantTarget::Role(id("alice"))));
        assert!(grantees.contains(&GrantTarget::Role(id("bob"))));
    }

    #[test]
    fn schema_grant_usage() {
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT USAGE ON SCHEMA app TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        let grants = &cat.schemas[0].grants;
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].privilege, Privilege::Usage);
        assert_eq!(grants[0].grantee, GrantTarget::Role(id("alice")));
    }

    #[test]
    fn function_grant_with_signature_extraction() {
        use crate::ir::function::{
            ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
            ReturnType, SecurityMode, Volatility,
        };
        let mut cat = seed_catalog();
        let args = vec![FunctionArg {
            name: None,
            mode: ArgMode::In,
            ty: ColumnType::Integer,
            default: None,
        }];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        cat.functions.push(Function {
            qname: qn("app", "double"),
            args,
            arg_types_normalized,
            return_type: ReturnType::Scalar {
                ty: ColumnType::Integer,
            },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::from_sql("SELECT $1 * 2").unwrap(),
            body_dependencies: vec![],
            volatility: Volatility::Immutable,
            strict: true,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Safe,
            leakproof: false,
            cost: Some(1.0),
            rows: None,
            comment: None,
            owner: None,
            grants: vec![],
        });
        let s = parse_grant("GRANT EXECUTE ON FUNCTION app.double(integer) TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        let grants = &cat.functions[0].grants;
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].privilege, Privilege::Execute);
        assert_eq!(grants[0].grantee, GrantTarget::Role(id("alice")));
    }

    #[test]
    fn revoke_rejected() {
        let mut cat = seed_catalog();
        let s = parse_grant("REVOKE SELECT ON TABLE app.users FROM alice;");
        let err = apply(&s, &mut cat, &loc()).unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if message.contains("REVOKE")),
            "expected REVOKE error, got: {err:?}"
        );
    }

    #[test]
    fn grant_on_unmanaged_objtype_rejected() {
        // DATABASE grants are not managed by pgevolve.
        let mut cat = seed_catalog();
        let s = parse_grant("GRANT CONNECT ON DATABASE mydb TO alice;");
        let err = apply(&s, &mut cat, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }
}
