//! `ALTER <objkind> name OWNER TO role` — ownership assignment.
//!
//! Handles both the standalone [`AlterOwnerStmt`] node (for schemas, functions,
//! procedures, types, sequences) and the `AlterTableStmt::AT_ChangeOwner`
//! sub-command for relation-family objects (tables, views, materialized views).
//!
//! Unsupported object kinds (DATABASE, TABLESPACE, etc.) raise
//! [`ParseError::Structural`].

use pg_query::NodeEnum;
use pg_query::protobuf::{AlterOwnerStmt, ObjectType, RoleSpecType};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Apply a standalone `ALTER <kind> name OWNER TO role` statement.
pub(crate) fn apply(
    s: &AlterOwnerStmt,
    cat: &mut Catalog,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    let objtype = ObjectType::try_from(s.object_type).unwrap_or(ObjectType::Undefined);
    let new_owner = extract_new_owner(s, loc)?;

    match objtype {
        // Relation-family objects: use `relation` field (a RangeVar).
        ObjectType::ObjectTable | ObjectType::ObjectView | ObjectType::ObjectMatview => {
            let rv = s.relation.as_ref().ok_or_else(|| ParseError::Structural {
                location: loc.clone(),
                message: "ALTER OWNER missing relation".into(),
            })?;
            let qname = shared::resolve_qname(rv, None, loc)?;
            set_owner_for_relation(cat, &qname, objtype, new_owner, loc)
        }
        ObjectType::ObjectSequence => {
            let rv = s.relation.as_ref().ok_or_else(|| ParseError::Structural {
                location: loc.clone(),
                message: "ALTER SEQUENCE OWNER missing relation".into(),
            })?;
            let qname = shared::resolve_qname(rv, None, loc)?;
            let seq = cat
                .sequences
                .iter_mut()
                .find(|sq| sq.qname == qname)
                .ok_or_else(|| missing(loc, "sequence", &qname.to_string()))?;
            seq.owner = Some(new_owner);
            Ok(())
        }
        ObjectType::ObjectSchema => {
            let schema_name = schema_name_from_object(s, loc)?;
            let schema = cat
                .schemas
                .iter_mut()
                .find(|sc| sc.name == schema_name)
                .ok_or_else(|| missing(loc, "schema", schema_name.as_str()))?;
            schema.owner = Some(new_owner);
            Ok(())
        }
        ObjectType::ObjectFunction => {
            let qname = qname_from_object(s, loc)?;
            let func = cat
                .functions
                .iter_mut()
                .find(|f| f.qname == qname)
                .ok_or_else(|| missing(loc, "function", &qname.to_string()))?;
            func.owner = Some(new_owner);
            Ok(())
        }
        ObjectType::ObjectProcedure | ObjectType::ObjectRoutine => {
            let qname = qname_from_object(s, loc)?;
            let proc = cat
                .procedures
                .iter_mut()
                .find(|p| p.qname == qname)
                .ok_or_else(|| missing(loc, "procedure", &qname.to_string()))?;
            proc.owner = Some(new_owner);
            Ok(())
        }
        ObjectType::ObjectType => {
            let qname = type_qname_from_object(s, loc)?;
            let ty = cat
                .types
                .iter_mut()
                .find(|t| t.qname == qname)
                .ok_or_else(|| missing(loc, "type", &qname.to_string()))?;
            ty.owner = Some(new_owner);
            Ok(())
        }
        ObjectType::ObjectDatabase
        | ObjectType::ObjectTablespace
        | ObjectType::ObjectLanguage
        | ObjectType::ObjectForeignServer
        | ObjectType::ObjectFdw => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "ALTER {objtype:?} OWNER TO is not managed by pgevolve; \
                 only TABLE, VIEW, MATERIALIZED VIEW, SEQUENCE, SCHEMA, \
                 FUNCTION, PROCEDURE, and TYPE ownership is supported"
            ),
        }),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "unsupported ALTER OWNER object type {other:?} — pgevolve does not manage \
                 ownership for this object kind"
            ),
        }),
    }
}

/// Apply ownership to a relation-family object (TABLE, VIEW, or MV) by qname.
///
/// Also called from `alter_table_stmt` for the `AT_ChangeOwner` sub-command.
pub(crate) fn set_owner_for_relation(
    cat: &mut Catalog,
    qname: &QualifiedName,
    objtype: ObjectType,
    new_owner: Identifier,
    loc: &SourceLocation,
) -> Result<(), ParseError> {
    if let Some(tbl) = cat.tables.iter_mut().find(|t| &t.qname == qname) {
        tbl.owner = Some(new_owner);
        return Ok(());
    }
    if let Some(view) = cat.views.iter_mut().find(|v| &v.qname == qname) {
        view.owner = Some(new_owner);
        return Ok(());
    }
    if let Some(mv) = cat
        .materialized_views
        .iter_mut()
        .find(|m| &m.qname == qname)
    {
        mv.owner = Some(new_owner);
        return Ok(());
    }
    let kind = match objtype {
        ObjectType::ObjectView => "view",
        ObjectType::ObjectMatview => "materialized view",
        _ => "table",
    };
    Err(missing(loc, kind, &qname.to_string()))
}

// ─── Field extraction helpers ─────────────────────────────────────────────────

/// Extract the `newowner` role name from an `AlterOwnerStmt`.
pub(crate) fn extract_new_owner(
    s: &AlterOwnerStmt,
    loc: &SourceLocation,
) -> Result<Identifier, ParseError> {
    let rolespec = s.newowner.as_ref().ok_or_else(|| ParseError::Structural {
        location: loc.clone(),
        message: "ALTER OWNER missing OWNER TO clause".into(),
    })?;
    let roletype = RoleSpecType::try_from(rolespec.roletype).unwrap_or(RoleSpecType::Undefined);
    if roletype == RoleSpecType::RolespecPublic {
        return Err(ParseError::Structural {
            location: loc.clone(),
            message: "ALTER OWNER TO PUBLIC is not valid — PUBLIC is not a role name".into(),
        });
    }
    shared::ident(&rolespec.rolename, loc)
}

/// Extract a schema name from `s.object` (a bare String node).
fn schema_name_from_object(
    s: &AlterOwnerStmt,
    loc: &SourceLocation,
) -> Result<Identifier, ParseError> {
    let node = s
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: "ALTER SCHEMA OWNER missing object reference".into(),
        })?;
    match node {
        NodeEnum::String(s) => shared::ident(&s.sval, loc),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected String node for schema name in ALTER SCHEMA OWNER, got {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Extract a `QualifiedName` from `s.object` (an `ObjectWithArgs` or `List` node
/// for functions/procedures).
fn qname_from_object(
    s: &AlterOwnerStmt,
    loc: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let node = s
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: "ALTER FUNCTION/PROCEDURE OWNER missing object reference".into(),
        })?;
    match node {
        NodeEnum::ObjectWithArgs(owa) => shared::qname_from_string_list(&owa.objname, None, loc),
        NodeEnum::List(list) => shared::qname_from_string_list(&list.items, None, loc),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected ObjectWithArgs or List node for function/procedure name \
                 in ALTER OWNER, got {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

/// Extract a `QualifiedName` from `s.object` for type objects (a List of String nodes).
fn type_qname_from_object(
    s: &AlterOwnerStmt,
    loc: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let node = s
        .object
        .as_ref()
        .and_then(|o| o.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: loc.clone(),
            message: "ALTER TYPE OWNER missing object reference".into(),
        })?;
    match node {
        NodeEnum::TypeName(tn) => shared::qname_from_string_list(&tn.names, None, loc),
        NodeEnum::List(list) => shared::qname_from_string_list(&list.items, None, loc),
        other => Err(ParseError::Structural {
            location: loc.clone(),
            message: format!(
                "expected TypeName or List node for type name in ALTER TYPE OWNER, got {:?}",
                std::mem::discriminant(other)
            ),
        }),
    }
}

fn missing(loc: &SourceLocation, kind: &str, name: &str) -> ParseError {
    ParseError::Structural {
        location: loc.clone(),
        message: format!("ALTER OWNER references {kind} {name} which is not defined in source"),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::schema::Schema;
    use crate::ir::sequence::Sequence;
    use crate::ir::table::Table;
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
            columns: vec![Column {
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
            }],
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
        c
    }

    fn parse_alter_owner(sql: &str) -> AlterOwnerStmt {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::AlterOwnerStmt(s) = stmt else {
            panic!("not AlterOwnerStmt")
        };
        *s
    }

    #[test]
    fn schema_owner_changes() {
        let mut cat = seed_catalog();
        let s = parse_alter_owner("ALTER SCHEMA app OWNER TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        assert_eq!(cat.schemas[0].owner, Some(id("alice")));
    }

    #[test]
    fn table_owner_changes() {
        // ALTER TABLE ... OWNER TO arrives as AlterTableStmt (AtChangeOwner sub-
        // command), not AlterOwnerStmt. Test the shared helper `set_owner_for_relation`
        // directly, which is what alter_table_stmt calls.
        let mut cat = seed_catalog();
        set_owner_for_relation(
            &mut cat,
            &qn("app", "users"),
            ObjectType::ObjectTable,
            id("alice"),
            &loc(),
        )
        .unwrap();
        assert_eq!(cat.tables[0].owner, Some(id("alice")));
    }

    #[test]
    fn function_owner_with_signature() {
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
        let s = parse_alter_owner("ALTER FUNCTION app.double(integer) OWNER TO alice;");
        apply(&s, &mut cat, &loc()).unwrap();
        assert_eq!(cat.functions[0].owner, Some(id("alice")));
    }

    #[test]
    fn unknown_object_errors() {
        // Use ALTER SCHEMA for a name that doesn't exist — AlterOwnerStmt path.
        let mut cat = seed_catalog();
        let s = parse_alter_owner("ALTER SCHEMA does_not_exist OWNER TO alice;");
        let err = apply(&s, &mut cat, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn unsupported_object_type_errors() {
        let mut cat = seed_catalog();
        // DATABASE is not managed.
        let s = parse_alter_owner("ALTER DATABASE mydb OWNER TO alice;");
        let err = apply(&s, &mut cat, &loc()).unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { ref message, .. } if {
                // Either "not managed" or "unsupported" — both are correct rejections.
                message.contains("not managed") || message.contains("unsupported")
            })
        );
    }
}
