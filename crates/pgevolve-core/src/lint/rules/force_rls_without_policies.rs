//! Warns when FORCE ROW LEVEL SECURITY is enabled on a table with no
//! policies. PG's behavior in that state is to deny every row — almost
//! always a configuration mistake (operator forgot to add policies).

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "force-rls-without-policies";

pub fn check(cat: &Catalog) -> Vec<Finding> {
    cat.tables
        .iter()
        .filter(|t| t.rls_forced && t.policies.is_empty())
        .map(|t| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!(
                "table {}: FORCE ROW LEVEL SECURITY enabled but no policies defined — all rows will be denied",
                t.qname,
            ),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::grant::GrantTarget;
    use crate::ir::policy::{Policy, PolicyCommand};
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn table(qname: QualifiedName, rls_forced: bool, policies: Vec<Policy>) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: rls_forced, // in real PG, FORCE without ENABLE is permitted; the lint checks the FORCE flag independently
            rls_forced,
            policies,
        }
    }

    fn dummy_policy() -> Policy {
        Policy {
            name: id("p"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        }
    }

    #[test]
    fn force_without_policies_fires() {
        let mut cat = Catalog::empty();
        cat.tables.push(table(qn("app", "t"), true, vec![]));
        let f = check(&cat);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, RULE_ID);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn force_with_policies_silent() {
        let mut cat = Catalog::empty();
        cat.tables
            .push(table(qn("app", "t"), true, vec![dummy_policy()]));
        assert!(check(&cat).is_empty());
    }

    #[test]
    fn no_force_no_policies_silent() {
        let mut cat = Catalog::empty();
        cat.tables.push(table(qn("app", "t"), false, vec![]));
        assert!(check(&cat).is_empty());
    }
}
