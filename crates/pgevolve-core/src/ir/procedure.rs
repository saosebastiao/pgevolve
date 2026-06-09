//! User-defined procedures (SQL or PL/pgSQL).

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};
use crate::ir::function::{FunctionArg, FunctionLanguage, SecurityMode};
use crate::parse::normalize_body::NormalizedBody;
use crate::plan::edges::DepEdge;

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
    /// Dependency edges extracted from the procedure body AST.
    ///
    /// Filled by the T4 PL/pgSQL body parser. Empty until that pass runs.
    #[serde(default)]
    pub body_dependencies: Vec<DepEdge>,
    /// Security context (INVOKER or DEFINER).
    pub security: SecurityMode,
    /// Parser-detected COMMIT/ROLLBACK in body. Drives transactional=OutsideTransaction at planner time.
    pub commits_in_body: bool,
    /// Optional `COMMENT ON PROCEDURE` text.
    pub comment: Option<String>,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER PROCEDURE ... OWNER TO role`.
    pub owner: Option<crate::identifier::Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    pub grants: Vec<crate::ir::grant::Grant>,
}

impl Equiv for Procedure {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        let Self {
            qname: _,
            args: _,
            language: _,
            body: _,
            body_dependencies: _,
            security: _,
            commits_in_body: _,
            comment: _,
            owner: _,
            grants: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("qname", &self.qname, &other.qname));
        out.extend(field_difference(
            "args",
            &format!("{:?}", self.args),
            &format!("{:?}", other.args),
        ));
        out.extend(field_difference(
            "language",
            &format!("{:?}", self.language),
            &format!("{:?}", other.language),
        ));
        out.extend(field_difference(
            "body",
            &format!("{:?}", self.body),
            &format!("{:?}", other.body),
        ));
        out.extend(field_difference(
            "body_dependencies",
            &format!("{:?}", self.body_dependencies),
            &format!("{:?}", other.body_dependencies),
        ));
        out.extend(field_difference(
            "security",
            &format!("{:?}", self.security),
            &format!("{:?}", other.security),
        ));
        out.extend(field_difference(
            "commits_in_body",
            &self.commits_in_body,
            &other.commits_in_body,
        ));
        out.extend(field_difference(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
            "grants",
            &format!("{:?}", self.grants),
            &format!("{:?}", other.grants),
        ));
        out
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
            body_dependencies: vec![],
            security: SecurityMode::Invoker,
            commits_in_body: false,
            comment: None,
            owner: None,
            grants: Vec::new(),
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
    fn procedure_diff_reports_per_field_changes() {
        use crate::ir::eq::Equiv;

        let a = sample_procedure();
        let mut b = sample_procedure();
        b.language = FunctionLanguage::Sql;
        b.comment = Some("changed".into());

        let d = a.differences(&b);
        let paths: Vec<&str> = d.iter().map(|x| x.path.as_str()).collect();
        assert!(
            paths.contains(&"language"),
            "expected 'language' in {paths:?}"
        );
        assert!(
            paths.contains(&"comment"),
            "expected 'comment' in {paths:?}"
        );
        // Old behavior was a single empty-path entry; new behavior must emit
        // exactly the two changed fields, no more.
        assert_eq!(d.len(), 2, "expected exactly two field diffs, got {d:?}");
    }

    #[test]
    fn catalog_rejects_duplicate_procedure_qname() {
        use crate::ir::IrError;
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(ident("app")));
        c.procedures.push(sample_procedure());
        c.procedures.push(sample_procedure());
        let r = c.canonicalize();
        assert!(matches!(
            r,
            Err(IrError::DuplicateObject {
                kind: "procedure",
                ..
            })
        ));
        assert!(r.unwrap_err().to_string().contains("app.do_thing"));
    }

    #[test]
    fn owner_change_diffs() {
        use crate::ir::eq::Equiv;
        let mut b = sample_procedure();
        b.owner = Some(ident("new_owner"));
        assert!(
            sample_procedure()
                .differences(&b)
                .iter()
                .any(|x| x.path == "owner")
        );
    }

    #[test]
    fn grants_change_diffs() {
        use crate::ir::eq::Equiv;
        let mut b = sample_procedure();
        b.grants.push(crate::ir::grant::Grant {
            grantee: crate::ir::grant::GrantTarget::Public,
            privilege: crate::ir::grant::Privilege::Execute,
            with_grant_option: false,
            columns: None,
        });
        assert!(
            sample_procedure()
                .differences(&b)
                .iter()
                .any(|x| x.path == "grants")
        );
    }
}
