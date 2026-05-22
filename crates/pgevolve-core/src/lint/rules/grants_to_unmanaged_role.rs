//! Warns when the catalog has grants to roles not declared in the source.
//!
//! These grants don't produce REVOKE steps (lenient drift policy in
//! `diff::grants`), but operators should know they exist so they can decide
//! whether to bring the role under management or accept the drift.

use crate::diff::changeset::ChangeSet;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "grants-to-unmanaged-role";

pub fn check(cs: &ChangeSet) -> Vec<Finding> {
    cs.unmanaged_grants
        .iter()
        .map(|obs| Finding {
            rule: RULE_ID,
            severity: Severity::Warning,
            message: format!(
                "{}: catalog has grant {} to role {} which is not declared in source",
                obs.object_label, obs.privilege_label, obs.role_name,
            ),
            location: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::changeset::{ChangeSet, UnmanagedGrantObservation};
    use crate::identifier::Identifier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn make_observation() -> UnmanagedGrantObservation {
        UnmanagedGrantObservation {
            object_label: "table app.t".into(),
            privilege_label: "SELECT".into(),
            role_name: id("temp_consultant"),
        }
    }

    #[test]
    fn empty_observations_silent() {
        let cs = ChangeSet::default();
        assert!(check(&cs).is_empty());
    }

    #[test]
    fn observation_produces_warning() {
        let mut cs = ChangeSet::default();
        cs.unmanaged_grants.push(make_observation());
        let f = check(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, RULE_ID);
        assert_eq!(f[0].severity, Severity::Warning);
        assert!(f[0].message.contains("temp_consultant"));
        assert!(f[0].message.contains("table app.t"));
    }

    #[test]
    fn multiple_observations_produce_multiple_findings() {
        let mut cs = ChangeSet::default();
        cs.unmanaged_grants.push(make_observation());
        cs.unmanaged_grants.push(UnmanagedGrantObservation {
            object_label: "schema app".into(),
            privilege_label: "USAGE".into(),
            role_name: id("another_role"),
        });
        let f = check(&cs);
        assert_eq!(f.len(), 2);
    }
}
