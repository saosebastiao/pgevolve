//! `CREATE SCHEMA` → [`crate::ir::schema::Schema`].

use pg_query::protobuf::CreateSchemaStmt;

use crate::ir::schema::Schema;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`Schema`] from a `CREATE SCHEMA` AST.
pub fn build_schema(
    stmt: &CreateSchemaStmt,
    location: &SourceLocation,
) -> Result<Schema, ParseError> {
    if stmt.schemaname.is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "CREATE SCHEMA requires a schema name".into(),
        });
    }
    Ok(Schema::new(shared::ident(&stmt.schemaname, location)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_query::NodeEnum;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn build(sql: &str) -> Schema {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateSchemaStmt(s) = stmt else {
            panic!()
        };
        build_schema(&s, &loc()).expect("builds")
    }

    #[test]
    fn bare_schema() {
        let s = build("CREATE SCHEMA app;");
        assert_eq!(s.name.as_str(), "app");
    }

    #[test]
    fn case_folded() {
        let s = build("CREATE SCHEMA Billing;");
        assert_eq!(s.name.as_str(), "billing");
    }
}
