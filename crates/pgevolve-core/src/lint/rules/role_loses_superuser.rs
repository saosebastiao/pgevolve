//! `role-loses-superuser` lint rule.
//!
//! Warns when an `ALTER ROLE` flips `SUPERUSER` from `true` to `false`.
//! Losing superuser is rarely a routine config change; usually intentional but
//! worth surfacing so operators can confirm it was deliberate.

use crate::diff::cluster::{ClusterChange, ClusterChangeSet};
use crate::lint::finding::{Finding, Severity};

/// Rule ID emitted on the finding; matches the file name.
pub const RULE_ID: &str = "role-loses-superuser";

pub fn check(cs: &ClusterChangeSet) -> Vec<Finding> {
    let mut findings = Vec::new();
    for entry in &cs.entries {
        if let ClusterChange::AlterRoleAttributes { name, from, to } = &entry.change
            && from.superuser
            && !to.superuser
        {
            findings.push(Finding {
                severity: Severity::Warning,
                rule: RULE_ID,
                message: format!(
                    "role {name} loses SUPERUSER — confirm this is intentional; \
                     downgrading superuser is rarely a routine change"
                ),
                location: None,
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::cluster::{ClusterChange, ClusterChangeEntry, ClusterChangeSet};
    use crate::diff::destructiveness::Destructiveness;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::RoleAttributes;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn alter_with(name: &str, from_super: bool, to_super: bool) -> ClusterChangeSet {
        let from = RoleAttributes {
            superuser: from_super,
            ..RoleAttributes::default()
        };
        let to = RoleAttributes {
            superuser: to_super,
            ..RoleAttributes::default()
        };
        ClusterChangeSet {
            entries: vec![ClusterChangeEntry {
                change: ClusterChange::AlterRoleAttributes {
                    name: id(name),
                    from,
                    to,
                },
                destructiveness: Destructiveness::Safe,
            }],
        }
    }

    #[test]
    fn loses_superuser_fires() {
        let cs = alter_with("admin", true, false);
        let f = check(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, RULE_ID);
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn gains_superuser_silent() {
        let cs = alter_with("admin", false, true);
        assert!(check(&cs).is_empty());
    }

    #[test]
    fn no_superuser_change_silent() {
        let cs = alter_with("admin", true, true);
        assert!(check(&cs).is_empty());
    }
}
