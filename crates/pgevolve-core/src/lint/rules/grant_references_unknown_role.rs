//! Lint: a grant's grantee role isn't declared in the linked cluster source.
//!
//! Runs only when the caller has loaded the cluster project's role names
//! (via `[cluster].project` in pgevolve.toml). When the link is absent,
//! the rule is silently skipped — per-DB independence preserved.
//!
//! # Cluster-aware source-tree rules
//!
//! This rule is dispatched via [`crate::lint::universal::check_universal_with_cluster`].

use std::collections::BTreeSet;

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::grant::GrantTarget;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "grant-references-unknown-role";

/// Check requires `cluster_role_names` — the set of role names declared
/// in the linked cluster project's roles/*.sql. When the user hasn't set
/// `[cluster].project`, the caller passes `None`, and this rule emits
/// nothing.
#[allow(clippy::too_many_lines)] // each object family needs its own block; extracting further would hurt readability
pub fn check(cat: &Catalog, cluster_role_names: Option<&BTreeSet<Identifier>>) -> Vec<Finding> {
    let Some(cluster_roles) = cluster_role_names else {
        return Vec::new();
    };

    let mut findings = Vec::new();

    let check_grants =
        |obj_label: &str, grants: &[crate::ir::grant::Grant], out: &mut Vec<Finding>| {
            for g in grants {
                if let GrantTarget::Role(name) = &g.grantee
                    && !cluster_roles.contains(name)
                {
                    out.push(Finding {
                        rule: RULE_ID,
                        severity: Severity::Error,
                        message: format!(
                            "{obj_label}: GRANT to role {name} but that role is not \
                             declared in the linked cluster project"
                        ),
                        location: None,
                    });
                }
            }
        };

    let check_owner = |obj_label: &str, owner: Option<&Identifier>, out: &mut Vec<Finding>| {
        if let Some(o) = owner
            && !cluster_roles.contains(o)
        {
            out.push(Finding {
                rule: RULE_ID,
                severity: Severity::Error,
                message: format!(
                    "{obj_label}: OWNER role {o} is not declared in the linked \
                         cluster project"
                ),
                location: None,
            });
        }
    };

    for s in &cat.schemas {
        let label = format!("schema {}", s.name);
        check_grants(&label, &s.grants, &mut findings);
        check_owner(&label, s.owner.as_ref(), &mut findings);
    }
    for s in &cat.sequences {
        let label = format!("sequence {}", s.qname);
        check_grants(&label, &s.grants, &mut findings);
        check_owner(&label, s.owner.as_ref(), &mut findings);
    }
    for t in &cat.tables {
        let label = format!("table {}", t.qname);
        check_grants(&label, &t.grants, &mut findings);
        check_owner(&label, t.owner.as_ref(), &mut findings);
    }
    for v in &cat.views {
        let label = format!("view {}", v.qname);
        check_grants(&label, &v.grants, &mut findings);
        check_owner(&label, v.owner.as_ref(), &mut findings);
    }
    for m in &cat.materialized_views {
        let label = format!("materialized view {}", m.qname);
        check_grants(&label, &m.grants, &mut findings);
        check_owner(&label, m.owner.as_ref(), &mut findings);
    }
    for f in &cat.functions {
        let label = format!("function {}", f.qname);
        check_grants(&label, &f.grants, &mut findings);
        check_owner(&label, f.owner.as_ref(), &mut findings);
    }
    for p in &cat.procedures {
        let label = format!("procedure {}", p.qname);
        check_grants(&label, &p.grants, &mut findings);
        check_owner(&label, p.owner.as_ref(), &mut findings);
    }
    for t in &cat.types {
        let label = format!("type {}", t.qname);
        check_grants(&label, &t.grants, &mut findings);
        check_owner(&label, t.owner.as_ref(), &mut findings);
    }
    // Policies on tables — TO clause references.
    for t in &cat.tables {
        for p in &t.policies {
            for role_target in &p.roles {
                if let crate::ir::grant::GrantTarget::Role(name) = role_target
                    && !cluster_roles.contains(name)
                {
                    findings.push(Finding {
                        rule: RULE_ID,
                        severity: Severity::Error,
                        message: format!(
                            "policy {} on table {}: TO clause references role {} which is not declared in the linked cluster project",
                            p.name, t.qname, name,
                        ),
                        location: None,
                    });
                }
            }
        }
    }
    // Default privileges: check the target_role and each grantee.
    for r in &cat.default_privileges {
        if !cluster_roles.contains(&r.target_role) {
            findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Error,
                message: format!(
                    "default privileges for role {}: target role is not declared in the \
                     linked cluster project",
                    r.target_role,
                ),
                location: None,
            });
        }
        let label = format!("default privileges for role {}", r.target_role);
        check_grants(&label, &r.grants, &mut findings);
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::grant::{Grant, GrantTarget, Privilege};
    use crate::ir::schema::Schema;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn cluster_with(roles: &[&str]) -> BTreeSet<Identifier> {
        roles.iter().map(|r| id(r)).collect()
    }

    fn schema_with_grant(role: &str) -> Schema {
        Schema {
            name: id("app"),
            comment: None,
            owner: None,
            grants: vec![Grant {
                grantee: GrantTarget::Role(id(role)),
                privilege: Privilege::Usage,
                with_grant_option: false,
                columns: None,
            }],
        }
    }

    #[test]
    fn cluster_roles_none_silent() {
        let mut cat = Catalog::empty();
        cat.schemas.push(schema_with_grant("readers"));
        assert!(check(&cat, None).is_empty());
    }

    #[test]
    fn known_role_silent() {
        let mut cat = Catalog::empty();
        cat.schemas.push(schema_with_grant("readers"));
        let cluster_roles = cluster_with(&["readers"]);
        assert!(check(&cat, Some(&cluster_roles)).is_empty());
    }

    #[test]
    fn unknown_role_errors() {
        let mut cat = Catalog::empty();
        cat.schemas.push(schema_with_grant("unknown_role"));
        let cluster_roles = cluster_with(&["readers"]);
        let f = check(&cat, Some(&cluster_roles));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
        assert_eq!(f[0].rule, RULE_ID);
        assert!(f[0].message.contains("unknown_role"));
    }

    #[test]
    fn public_always_silent() {
        let mut cat = Catalog::empty();
        let mut s = schema_with_grant("readers");
        s.grants[0].grantee = GrantTarget::Public;
        cat.schemas.push(s);
        let cluster_roles = cluster_with(&[]); // empty cluster
        assert!(check(&cat, Some(&cluster_roles)).is_empty());
    }

    #[test]
    fn unknown_owner_errors() {
        let mut cat = Catalog::empty();
        let mut s = schema_with_grant("readers");
        s.owner = Some(id("unknown_owner"));
        cat.schemas.push(s);
        let cluster_roles = cluster_with(&["readers"]);
        let f = check(&cat, Some(&cluster_roles));
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("OWNER"));
        assert!(f[0].message.contains("unknown_owner"));
    }

    #[test]
    fn policy_to_clause_with_unknown_role_fires() {
        use crate::identifier::QualifiedName;
        use crate::ir::policy::{Policy, PolicyCommand};
        use crate::ir::table::Table;

        fn qn(schema: &str, name: &str) -> QualifiedName {
            QualifiedName::new(id(schema), id(name))
        }

        let mut cat = Catalog::empty();
        let t = Table {
            qname: qn("app", "orders"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: true,
            rls_forced: false,
            policies: vec![Policy {
                name: id("p1"),
                permissive: true,
                command: PolicyCommand::All,
                roles: vec![GrantTarget::Role(id("unknown_role"))],
                using: None,
                with_check: None,
            }],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        };
        cat.tables.push(t);
        let cluster_roles = cluster_with(&["readers"]);
        let f = check(&cat, Some(&cluster_roles));
        assert!(
            f.iter()
                .any(|f| f.message.contains("policy p1") && f.message.contains("unknown_role")),
            "expected policy-level lint finding"
        );
    }
}
