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
}

impl Statement {
    /// Classify a node into the supported whitelist, or return
    /// [`ParseError::UnsupportedObjectKind`] for anything else.
    pub fn classify(node: NodeEnum, location: SourceLocation) -> Result<Self, ParseError> {
        match node {
            NodeEnum::CreateSchemaStmt(s) => Ok(Self::CreateSchema(s)),
            NodeEnum::CreateStmt(s) => Ok(Self::CreateTable(s)),
            NodeEnum::CreateSeqStmt(s) => Ok(Self::CreateSequence(s)),
            NodeEnum::IndexStmt(s) => Ok(Self::CreateIndex(*s)),
            NodeEnum::AlterTableStmt(s) => Ok(Self::AlterTable(s)),
            NodeEnum::CommentStmt(s) => Ok(Self::Comment(*s)),
            NodeEnum::ViewStmt(s) => Ok(Self::CreateView(*s)),
            NodeEnum::CreateEnumStmt(s) => Ok(Self::CreateEnum(s)),
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

const fn unsupported(location: SourceLocation, kind: &'static str) -> ParseError {
    ParseError::UnsupportedObjectKind { location, kind }
}

/// Translate non-whitelisted node kinds to a human-readable label for diagnostics.
#[allow(clippy::match_same_arms)]
const fn friendly_kind(node: &NodeEnum) -> &'static str {
    match node {
        NodeEnum::ViewStmt(_) => "CREATE VIEW", // never reached — routed above
        NodeEnum::CreateEnumStmt(_) => "CREATE TYPE ... AS ENUM", // never reached — routed above
        NodeEnum::CreateFunctionStmt(_) => "CREATE FUNCTION/PROCEDURE",
        NodeEnum::CreateTrigStmt(_) => "CREATE TRIGGER",
        NodeEnum::CreateRangeStmt(_) => "CREATE TYPE ... AS RANGE",
        NodeEnum::CompositeTypeStmt(_) => "CREATE TYPE ... AS (...)",
        NodeEnum::CreateDomainStmt(_) => "CREATE DOMAIN",
        NodeEnum::CreateExtensionStmt(_) => "CREATE EXTENSION",
        NodeEnum::CreatePolicyStmt(_) => "CREATE POLICY",
        NodeEnum::CreateForeignTableStmt(_) => "CREATE FOREIGN TABLE",
        NodeEnum::CreateFdwStmt(_) => "CREATE FOREIGN DATA WRAPPER",
        NodeEnum::CreateForeignServerStmt(_) => "CREATE FOREIGN SERVER",
        NodeEnum::CreateRoleStmt(_) => "CREATE ROLE",
        NodeEnum::GrantStmt(_) => "GRANT/REVOKE",
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
    fn create_function_unsupported() {
        let node =
            first_node("CREATE FUNCTION app.f() RETURNS integer AS $$ SELECT 1; $$ LANGUAGE sql;");
        let err = Statement::classify(node, loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnsupportedObjectKind { .. }));
    }

    #[test]
    fn create_extension_unsupported() {
        let node = first_node("CREATE EXTENSION pgcrypto;");
        let err = Statement::classify(node, loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnsupportedObjectKind { .. }));
    }

    #[test]
    fn select_unsupported() {
        let node = first_node("SELECT 1;");
        let err = Statement::classify(node, loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnsupportedObjectKind { .. }));
    }
}
