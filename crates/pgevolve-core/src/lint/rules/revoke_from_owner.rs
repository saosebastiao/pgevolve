//! Errors when a REVOKE step targets the object's owner.
//!
//! PG silently rejects (owner has implicit privileges); we pre-empt with
//! a clear plan-time error so the operator can fix the source rather than
//! discovering the issue at apply time.

use crate::diff::changeset::ChangeSet;
use crate::ir::grant::GrantTarget;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "revoke-from-owner";

pub fn check(cs: &ChangeSet) -> Vec<Finding> {
    cs.revokes_with_owner
        .iter()
        .filter_map(|obs| {
            let grantee_matches_owner = match &obs.grantee {
                GrantTarget::Role(name) => name == &obs.owner,
                GrantTarget::Public => false,
            };
            if !grantee_matches_owner {
                return None;
            }
            Some(Finding {
                rule: RULE_ID,
                severity: Severity::Error,
                message: format!(
                    "REVOKE {} ON {} would target the object's owner {}; \
                     PG silently rejects (owner has implicit privileges)",
                    obs.privilege_label, obs.object_label, obs.owner,
                ),
                location: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::changeset::{ChangeSet, RevokeWithOwnerObservation};
    use crate::identifier::Identifier;
    use crate::ir::grant::GrantTarget;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn empty_observations_silent() {
        let cs = ChangeSet::default();
        assert!(check(&cs).is_empty());
    }

    #[test]
    fn revoke_targeting_owner_fires() {
        let mut cs = ChangeSet::default();
        cs.revokes_with_owner.push(RevokeWithOwnerObservation {
            object_label: "table app.t".into(),
            privilege_label: "SELECT".into(),
            grantee: GrantTarget::Role(id("app_owner")),
            owner: id("app_owner"),
        });
        let f = check(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
        assert_eq!(f[0].rule, RULE_ID);
    }

    #[test]
    fn revoke_targeting_other_role_silent() {
        let mut cs = ChangeSet::default();
        cs.revokes_with_owner.push(RevokeWithOwnerObservation {
            object_label: "table app.t".into(),
            privilege_label: "SELECT".into(),
            grantee: GrantTarget::Role(id("readers")),
            owner: id("app_owner"),
        });
        assert!(check(&cs).is_empty());
    }

    #[test]
    fn revoke_targeting_public_silent() {
        let mut cs = ChangeSet::default();
        cs.revokes_with_owner.push(RevokeWithOwnerObservation {
            object_label: "table app.t".into(),
            privilege_label: "SELECT".into(),
            grantee: GrantTarget::Public,
            owner: id("app_owner"),
        });
        assert!(check(&cs).is_empty());
    }
}
