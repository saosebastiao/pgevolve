//! User-defined procedures (SQL or PL/pgSQL).

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::difference::Difference;
use crate::ir::eq::Diff;
use crate::ir::function::{FunctionArg, FunctionLanguage, SecurityMode};
use crate::parse::normalize_body::NormalizedBody;

/// A user-defined procedure (`CREATE PROCEDURE`).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Procedure {
    /// Schema-qualified procedure name.
    pub qname: QualifiedName,
    /// Declared argument list.
    pub args: Vec<FunctionArg>,
    /// Implementation language.
    pub language: FunctionLanguage,
    /// Canonicalized procedure body.
    pub body: NormalizedBody,
    /// Security context (INVOKER or DEFINER).
    pub security: SecurityMode,
    /// Parser-detected COMMIT/ROLLBACK in body. Drives transactional=OutsideTransaction at planner time.
    pub commits_in_body: bool,
    /// Optional `COMMENT ON PROCEDURE` text.
    pub comment: Option<String>,
}

impl Diff for Procedure {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        if self == other {
            Vec::new()
        } else {
            vec![Difference::new(
                "",
                format!("{self:?}"),
                format!("{other:?}"),
            )]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;

    fn ident(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(ident(schema), ident(name))
    }

    fn sample_procedure() -> Procedure {
        Procedure {
            qname: qname("app", "do_thing"),
            args: vec![],
            language: FunctionLanguage::PlPgSql,
            body: NormalizedBody::empty(),
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
        }
    }

    #[test]
    fn procedure_serde_round_trip() {
        let p = sample_procedure();
        let json = serde_json::to_string(&p).unwrap();
        let back: Procedure = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn catalog_rejects_duplicate_procedure_qname() {
        use crate::ir::IrError;
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        c.procedures.push(sample_procedure());
        c.procedures.push(sample_procedure());
        let r = c.canonicalize();
        assert!(matches!(r, Err(IrError::InvalidIdentifier(_))));
        assert!(r.unwrap_err().to_string().contains("app.do_thing"));
    }
}
