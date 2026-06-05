//! Classify a top-level `pg_query` statement node into the v0.1 whitelist.
//!
//! Anything outside the whitelist is rejected with [`ParseError::UnsupportedObjectKind`]
//! so that source-loading fails loudly instead of silently dropping unsupported DDL.

use pg_query::NodeEnum;
use pg_query::protobuf;
use pg_query::protobuf::ObjectType;

use crate::parse::error::{ParseError, SourceLocation};

/// One supported top-level statement kind. Each variant carries the typed protobuf
/// message that the corresponding builder will consume.
#[derive(Debug, Clone)]
pub enum Statement {
    /// `CREATE SCHEMA ...`.
    CreateSchema(protobuf::CreateSchemaStmt),
    /// `CREATE TABLE ...`.
    CreateTable(protobuf::CreateStmt),
    /// `CREATE SEQUENCE ...`.
    CreateSequence(protobuf::CreateSeqStmt),
    /// `CREATE [UNIQUE] INDEX ...`.
    CreateIndex(protobuf::IndexStmt),
    /// `ALTER TABLE ...` — only `ADD CONSTRAINT FOREIGN KEY` is allowed in source DDL.
    AlterTable(protobuf::AlterTableStmt),
    /// `COMMENT ON ...`.
    Comment(protobuf::CommentStmt),
    /// `CREATE [OR REPLACE] VIEW ...`.
    CreateView(protobuf::ViewStmt),
    /// `CREATE MATERIALIZED VIEW ...`.
    CreateMaterializedView(protobuf::CreateTableAsStmt),
    /// `CREATE TYPE ... AS ENUM (...)`.
    CreateEnum(protobuf::CreateEnumStmt),
    /// `CREATE DOMAIN ... AS ...`.
    CreateDomain(protobuf::CreateDomainStmt),
    /// `CREATE TYPE ... AS (...)` (composite).
    CreateCompositeType(protobuf::CompositeTypeStmt),
    /// `CREATE TYPE ... AS RANGE (...)`.
    CreateRange(protobuf::CreateRangeStmt),
    /// `CREATE [OR REPLACE] FUNCTION ...`.
    CreateFunction(protobuf::CreateFunctionStmt),
    /// `CREATE [OR REPLACE] PROCEDURE ...`.
    CreateProcedure(protobuf::CreateFunctionStmt),
    /// `CREATE EXTENSION ...`.
    CreateExtension(protobuf::CreateExtensionStmt),
    /// `CREATE [CONSTRAINT] TRIGGER ...`.
    CreateTrigger(protobuf::CreateTrigStmt),
    /// `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...`.
    AlterTableAttachPartition(protobuf::AlterTableStmt),
    /// `GRANT priv ON obj TO grantee` — object-level grant (not REVOKE, not role grant).
    Grant(protobuf::GrantStmt),
    /// `ALTER <kind> name OWNER TO role`.
    AlterOwner(protobuf::AlterOwnerStmt),
    /// `ALTER DEFAULT PRIVILEGES ... GRANT ...`.
    AlterDefaultPrivileges(protobuf::AlterDefaultPrivilegesStmt),
    /// `CREATE POLICY name ON table ...`.
    CreatePolicy(protobuf::CreatePolicyStmt),
    /// `CREATE PUBLICATION name ...`.
    CreatePublication(protobuf::CreatePublicationStmt),
    /// `ALTER PUBLICATION name ...`.
    AlterPublication(protobuf::AlterPublicationStmt),
    /// `CREATE SUBSCRIPTION name CONNECTION ... PUBLICATION ...`.
    CreateSubscription(protobuf::CreateSubscriptionStmt),
    /// `ALTER SUBSCRIPTION name ...`.
    AlterSubscription(protobuf::AlterSubscriptionStmt),
    /// `CREATE STATISTICS name … ON … FROM …`.
    CreateStatistics(protobuf::CreateStatsStmt),
    /// `ALTER STATISTICS name SET STATISTICS n`.
    AlterStatistics(protobuf::AlterStatsStmt),
    /// `CREATE COLLATION qname (provider = …, locale = …, …)`.
    CreateCollation(protobuf::DefineStmt),
    /// `CREATE EVENT TRIGGER name ON event …`.
    CreateEventTrigger(protobuf::CreateEventTrigStmt),
    /// `ALTER EVENT TRIGGER name {ENABLE|DISABLE|ENABLE REPLICA|ENABLE ALWAYS}`.
    AlterEventTrigger(protobuf::AlterEventTrigStmt),
}

impl Statement {
    /// Classify a node into the supported whitelist, or return
    /// [`ParseError::UnsupportedObjectKind`] for anything else.
    #[allow(clippy::too_many_lines)] // one arm per supported `NodeEnum` variant; the dispatch table is the function.
    pub fn classify(node: NodeEnum, location: SourceLocation) -> Result<Self, ParseError> {
        match node {
            NodeEnum::CreateSchemaStmt(s) => Ok(Self::CreateSchema(s)),
            NodeEnum::CreateStmt(s) => Ok(Self::CreateTable(s)),
            NodeEnum::CreateSeqStmt(s) => Ok(Self::CreateSequence(s)),
            NodeEnum::IndexStmt(s) => Ok(Self::CreateIndex(*s)),
            NodeEnum::AlterTableStmt(s) => {
                if is_attach_partition_stmt(&s) {
                    Ok(Self::AlterTableAttachPartition(s))
                } else {
                    Ok(Self::AlterTable(s))
                }
            }
            NodeEnum::CommentStmt(s) => Ok(Self::Comment(*s)),
            NodeEnum::ViewStmt(s) => Ok(Self::CreateView(*s)),
            NodeEnum::CreateEnumStmt(s) => Ok(Self::CreateEnum(s)),
            NodeEnum::CreateDomainStmt(s) => Ok(Self::CreateDomain(*s)),
            NodeEnum::CompositeTypeStmt(s) => Ok(Self::CreateCompositeType(s)),
            NodeEnum::CreateRangeStmt(s) => Ok(Self::CreateRange(s)),
            NodeEnum::CreateFunctionStmt(s) => {
                if s.is_procedure {
                    Ok(Self::CreateProcedure(*s))
                } else {
                    Ok(Self::CreateFunction(*s))
                }
            }
            NodeEnum::CreateExtensionStmt(s) => Ok(Self::CreateExtension(s)),
            NodeEnum::CreateTrigStmt(s) => Ok(Self::CreateTrigger(*s)),
            NodeEnum::GrantStmt(s) => Ok(Self::Grant(s)),
            NodeEnum::AlterOwnerStmt(s) => Ok(Self::AlterOwner(*s)),
            NodeEnum::AlterDefaultPrivilegesStmt(s) => Ok(Self::AlterDefaultPrivileges(s)),
            NodeEnum::CreatePolicyStmt(s) => Ok(Self::CreatePolicy(*s)),
            NodeEnum::CreatePublicationStmt(s) => Ok(Self::CreatePublication(s)),
            NodeEnum::AlterPublicationStmt(s) => Ok(Self::AlterPublication(s)),
            NodeEnum::CreateSubscriptionStmt(s) => Ok(Self::CreateSubscription(s)),
            NodeEnum::AlterSubscriptionStmt(s) => Ok(Self::AlterSubscription(s)),
            NodeEnum::CreateStatsStmt(s) => Ok(Self::CreateStatistics(s)),
            NodeEnum::AlterStatsStmt(s) => Ok(Self::AlterStatistics(*s)),
            NodeEnum::CreateEventTrigStmt(s) => Ok(Self::CreateEventTrigger(s)),
            NodeEnum::AlterEventTrigStmt(s) => Ok(Self::AlterEventTrigger(s)),
            NodeEnum::DefineStmt(s) => {
                let kind = ObjectType::try_from(s.kind).unwrap_or(ObjectType::Undefined);
                if matches!(kind, ObjectType::ObjectCollation) {
                    Ok(Self::CreateCollation(s))
                } else {
                    Err(unsupported(location, "CREATE AGGREGATE/OPERATOR/etc."))
                }
            }
            NodeEnum::RenameStmt(s) => {
                use pg_query::protobuf::ObjectType;
                let rename_type =
                    ObjectType::try_from(s.rename_type).unwrap_or(ObjectType::Undefined);
                if matches!(rename_type, ObjectType::ObjectPublication) {
                    return Err(ParseError::Structural {
                        location,
                        message: "ALTER PUBLICATION … RENAME TO is not supported in pgevolve \
                                  (pgevolve never models renames)"
                            .into(),
                    });
                }
                if matches!(rename_type, ObjectType::ObjectSubscription) {
                    // Recover the subscription name for the error (subname is in
                    // RenameStmt.relation for subscriptions).
                    let sub_name = s
                        .relation
                        .as_ref()
                        .map_or_else(|| String::from("<unknown>"), |r| r.relname.clone());
                    let id = crate::identifier::Identifier::from_unquoted(&sub_name)
                        .unwrap_or_else(|_| {
                            crate::identifier::Identifier::from_unquoted("subscription")
                                .expect("static identifier")
                        });
                    return Err(ParseError::SubscriptionRenameNotSupported(id, location));
                }
                if matches!(rename_type, ObjectType::ObjectEventTrigger) {
                    // RenameStmt for an event trigger encodes the (old) name in
                    // `s.object` as a bare String node.
                    let name = s
                        .object
                        .as_ref()
                        .and_then(|o| o.node.as_ref())
                        .and_then(|n| match n {
                            NodeEnum::String(st) => Some(st.sval.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| String::from("<unknown>"));
                    let id =
                        crate::identifier::Identifier::from_unquoted(&name).unwrap_or_else(|_| {
                            crate::identifier::Identifier::from_unquoted("event_trigger")
                                .expect("static identifier")
                        });
                    return Err(ParseError::EventTriggerRenameNotSupported(id, location));
                }
                if matches!(rename_type, ObjectType::ObjectStatisticExt) {
                    // RenameStmt for statistics encodes the name in `s.subname` (the
                    // new name) and `s.relation` (the old qualified name).
                    let schema = s
                        .relation
                        .as_ref()
                        .map_or_else(String::new, |r| r.schemaname.clone());
                    let name = s
                        .relation
                        .as_ref()
                        .map_or_else(|| String::from("<unknown>"), |r| r.relname.clone());
                    let schema_id = crate::identifier::Identifier::from_unquoted(&schema)
                        .unwrap_or_else(|_| {
                            crate::identifier::Identifier::from_unquoted("unknown")
                                .expect("static identifier")
                        });
                    let name_id = crate::identifier::Identifier::from_unquoted(&name)
                        .unwrap_or_else(|_| {
                            crate::identifier::Identifier::from_unquoted("statistic")
                                .expect("static identifier")
                        });
                    let qname = crate::identifier::QualifiedName::new(schema_id, name_id);
                    return Err(ParseError::StatisticRenameNotSupported(qname, location));
                }
                Err(unsupported(location, "RENAME"))
            }
            NodeEnum::AlterPolicyStmt(_) => Err(ParseError::Structural {
                location,
                message: "ALTER POLICY in source is not supported — policy modifications \
                          happen via diff; use CREATE POLICY in source"
                    .into(),
            }),
            NodeEnum::DropStmt(s) => {
                let kind = ObjectType::try_from(s.remove_type).unwrap_or(ObjectType::Undefined);
                if matches!(kind, ObjectType::ObjectPolicy) {
                    return Err(ParseError::Structural {
                        location,
                        message: "DROP POLICY in source is not supported — drops happen \
                                  via diff"
                            .into(),
                    });
                }
                Err(unsupported(location, "DROP"))
            }
            NodeEnum::AlterExtensionStmt(_) => Err(ParseError::Structural {
                location,
                message: "ALTER EXTENSION is not supported in source files — \
                          declare the desired state via CREATE EXTENSION"
                    .into(),
            }),
            NodeEnum::CreateTableAsStmt(s) => {
                // `CREATE TABLE ... AS SELECT` and `CREATE MATERIALIZED VIEW`
                // both arrive as CreateTableAsStmt. Route by the objtype field.
                match ObjectType::try_from(s.objtype) {
                    Ok(ObjectType::ObjectMatview) => Ok(Self::CreateMaterializedView(*s)),
                    _ => Err(unsupported(location, "CREATE TABLE AS SELECT")),
                }
            }
            other => Err(unsupported(location, friendly_kind(&other))),
        }
    }
}

/// Returns `true` when the `AlterTableStmt` is exactly one `ATTACH PARTITION`
/// sub-command so the classifier can route it to the dedicated variant.
fn is_attach_partition_stmt(stmt: &protobuf::AlterTableStmt) -> bool {
    use pg_query::protobuf::AlterTableType;
    if stmt.cmds.len() != 1 {
        return false;
    }
    match &stmt.cmds[0].node {
        Some(NodeEnum::AlterTableCmd(c)) => AlterTableType::try_from(c.subtype)
            .ok()
            .is_some_and(|t| matches!(t, AlterTableType::AtAttachPartition)),
        _ => false,
    }
}

const fn unsupported(location: SourceLocation, kind: &'static str) -> ParseError {
    ParseError::UnsupportedObjectKind { location, kind }
}

/// Translate non-whitelisted node kinds to a human-readable label for diagnostics.
#[allow(clippy::match_same_arms)]
const fn friendly_kind(node: &NodeEnum) -> &'static str {
    match node {
        NodeEnum::ViewStmt(_) => "CREATE VIEW", // never reached — routed above
        NodeEnum::CreateEnumStmt(_) => "CREATE TYPE ... AS ENUM", // never reached — routed above
        NodeEnum::CreateFunctionStmt(_) => "CREATE FUNCTION/PROCEDURE", // never reached — routed above
        NodeEnum::CreateTrigStmt(_) => "CREATE TRIGGER", // never reached — routed above
        NodeEnum::CreateRangeStmt(_) => "CREATE TYPE ... AS RANGE", // never reached — routed above
        NodeEnum::CreateExtensionStmt(_) => "CREATE EXTENSION", // never reached — routed above
        NodeEnum::CreatePolicyStmt(_) => "CREATE POLICY", // never reached — routed above
        NodeEnum::CreateForeignTableStmt(_) => "CREATE FOREIGN TABLE",
        NodeEnum::CreateFdwStmt(_) => "CREATE FOREIGN DATA WRAPPER",
        NodeEnum::CreateForeignServerStmt(_) => "CREATE FOREIGN SERVER",
        NodeEnum::CreateRoleStmt(_) => "CREATE ROLE",
        NodeEnum::GrantStmt(_) => "GRANT/REVOKE", // never reached — routed above
        NodeEnum::GrantRoleStmt(_) => "GRANT/REVOKE ROLE",
        NodeEnum::CreateTableAsStmt(_) => "CREATE TABLE AS",
        NodeEnum::CreateAmStmt(_) => "CREATE ACCESS METHOD",
        NodeEnum::CreateEventTrigStmt(_) => "CREATE EVENT TRIGGER",
        NodeEnum::CreateOpClassStmt(_) => "CREATE OPERATOR CLASS",
        NodeEnum::CreateOpFamilyStmt(_) => "CREATE OPERATOR FAMILY",
        NodeEnum::CreatePlangStmt(_) => "CREATE LANGUAGE",
        NodeEnum::CreateStatsStmt(_) => "CREATE STATISTICS",
        NodeEnum::CreateUserMappingStmt(_) => "CREATE USER MAPPING",
        NodeEnum::CreateTableSpaceStmt(_) => "CREATE TABLESPACE",
        NodeEnum::CreatedbStmt(_) => "CREATE DATABASE",
        NodeEnum::DefineStmt(_) => "CREATE AGGREGATE/OPERATOR/etc.",
        NodeEnum::RuleStmt(_) => "CREATE RULE",
        NodeEnum::DropStmt(_) => "DROP",
        NodeEnum::TruncateStmt(_) => "TRUNCATE",
        NodeEnum::SelectStmt(_) => "SELECT",
        NodeEnum::InsertStmt(_) => "INSERT",
        NodeEnum::UpdateStmt(_) => "UPDATE",
        NodeEnum::DeleteStmt(_) => "DELETE",
        NodeEnum::CopyStmt(_) => "COPY",
        NodeEnum::TransactionStmt(_) => "BEGIN/COMMIT/ROLLBACK",
        _ => "this statement kind",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn first_node(sql: &str) -> NodeEnum {
        let parsed = pg_query::parse(sql).expect("pg_query parses");
        parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("at least one statement")
    }

    #[test]
    fn create_table_classifies() {
        let node = first_node("CREATE TABLE app.users (id integer);");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateTable(_)));
    }

    #[test]
    fn create_schema_classifies() {
        let node = first_node("CREATE SCHEMA app;");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateSchema(_)));
    }

    #[test]
    fn create_index_classifies() {
        let node = first_node("CREATE INDEX users_email_idx ON app.users (email);");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateIndex(_)));
    }

    #[test]
    fn create_sequence_classifies() {
        let node = first_node("CREATE SEQUENCE app.seq1;");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateSequence(_)));
    }

    #[test]
    fn alter_table_classifies() {
        let node = first_node(
            "ALTER TABLE app.invoices ADD CONSTRAINT fk1 FOREIGN KEY (cid) REFERENCES app.c(id);",
        );
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::AlterTable(_)));
    }

    #[test]
    fn comment_classifies() {
        let node = first_node("COMMENT ON TABLE app.users IS 'people';");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::Comment(_)));
    }

    #[test]
    fn create_view_classifies() {
        let node = first_node("CREATE VIEW app.v AS SELECT 1;");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateView(_)));
    }

    #[test]
    fn create_materialized_view_classifies() {
        let node = first_node(
            "CREATE MATERIALIZED VIEW app.mv AS SELECT count(*) FROM app.t WITH NO DATA;",
        );
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateMaterializedView(_)));
    }

    #[test]
    fn create_table_as_select_unsupported() {
        let node = first_node("CREATE TABLE app.t2 AS SELECT 1;");
        let err = Statement::classify(node, loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnsupportedObjectKind { .. }));
    }

    #[test]
    fn create_function_classifies() {
        let node =
            first_node("CREATE FUNCTION app.f() RETURNS integer AS $$ SELECT 1; $$ LANGUAGE sql;");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateFunction(_)));
    }

    #[test]
    fn create_procedure_classifies() {
        let node =
            first_node("CREATE PROCEDURE app.p() LANGUAGE plpgsql AS $$ BEGIN NULL; END $$;");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateProcedure(_)));
    }

    #[test]
    fn create_extension_classifies() {
        let node = first_node("CREATE EXTENSION pgcrypto;");
        let stmt = Statement::classify(node, loc()).unwrap();
        assert!(matches!(stmt, Statement::CreateExtension(_)));
    }

    #[test]
    fn alter_extension_rejected() {
        let node = first_node("ALTER EXTENSION pgcrypto UPDATE;");
        let err = Statement::classify(node, loc()).unwrap_err();
        assert!(matches!(err, ParseError::Structural { .. }));
    }

    #[test]
    fn select_unsupported() {
        let node = first_node("SELECT 1;");
        let err = Statement::classify(node, loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnsupportedObjectKind { .. }));
    }
}
