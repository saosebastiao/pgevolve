//! Translate [`ClusterChange`] → [`RawStep`].
//!
//! Sibling of `plan/rewrite/emit/` for per-DB ops. Each cluster op is
//! self-contained (not scoped to a `QualifiedName`), so `targets` is left
//! empty — downstream consumers that iterate `targets` for status display
//! or dep-graph wiring can skip cluster steps, which is consistent with the
//! current design where cluster and per-DB plans are kept separate.

use crate::diff::cluster::{ClusterChange, ClusterChangeEntry, ClusterChangeSet};
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

use super::sql;

/// Emit one [`RawStep`] per [`ClusterChange`]. All cluster ops run
/// [`InTransaction`](TransactionConstraint::InTransaction).
#[must_use]
pub fn emit_cluster_changes(cs: &ClusterChangeSet) -> Vec<RawStep> {
    cs.entries.iter().map(emit_one).collect()
}

fn emit_one(entry: &ClusterChangeEntry) -> RawStep {
    match &entry.change {
        ClusterChange::CreateRole(role) => RawStep {
            step_no: 0,
            kind: StepKind::CreateRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            // Cluster ops are not scoped to a QualifiedName; targets is empty.
            targets: vec![],
            sql: sql::create_role(role),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::DropRole { name } => RawStep {
            step_no: 0,
            kind: StepKind::DropRole,
            destructive: true,
            destructive_reason: entry.destructiveness.reason().map(str::to_owned),
            intent_id: None,
            targets: vec![],
            sql: sql::drop_role(name),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::AlterRoleAttributes { name, from, to } => RawStep {
            step_no: 0,
            kind: StepKind::AlterRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::alter_role_attributes(name, from, to),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::GrantRoleMembership { member, role } => RawStep {
            step_no: 0,
            kind: StepKind::GrantRoleMembership,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::grant_role_membership(role, member),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::RevokeRoleMembership { member, role } => RawStep {
            step_no: 0,
            kind: StepKind::RevokeRoleMembership,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::revoke_role_membership(role, member),
            transactional: TransactionConstraint::InTransaction,
        },
        ClusterChange::CommentOnRole { name, comment } => RawStep {
            step_no: 0,
            kind: StepKind::CommentOnRole,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::comment_on_role(name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        },

        // TODO(tablespace Task 5): real emitter — these arms are temporary
        // placeholders until StepKind + sql:: helpers are added in Task 5.
        ClusterChange::CreateTablespace(_ts) => unimplemented_tablespace_step(),
        ClusterChange::DropTablespace { name: _name } => unimplemented_tablespace_step(),
        ClusterChange::AlterTablespaceOwner {
            name: _name,
            owner: _owner,
        } => unimplemented_tablespace_step(),
        ClusterChange::SetTablespaceOptions {
            name: _name,
            options: _options,
        } => unimplemented_tablespace_step(),
        ClusterChange::CommentOnTablespace {
            name: _name,
            comment: _comment,
        } => unimplemented_tablespace_step(),
    }
}

/// Placeholder emitter used by the tablespace `ClusterChange` variants until
/// Task 5 adds real `StepKind` entries and SQL helpers.
///
/// # Panics
/// Always panics — tablespace emit is not yet implemented (Task 5).
// TODO(tablespace Task 5): remove this helper once real arms are in place.
#[cold]
fn unimplemented_tablespace_step() -> RawStep {
    panic!("tablespace emit not yet implemented — this path requires Task 5")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::cluster::{ClusterChange, ClusterChangeEntry};
    use crate::diff::destructiveness::Destructiveness;
    use crate::identifier::Identifier;
    use crate::ir::cluster::role::{Role, RoleAttributes};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn create_role_emits_create_kind() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::CreateRole(Role {
                name: id("app_user"),
                attributes: RoleAttributes::default(),
                member_of: vec![],
                comment: None,
            }),
            destructiveness: Destructiveness::Safe,
        };
        let step = emit_one(&entry);
        assert!(matches!(step.kind, StepKind::CreateRole));
        assert!(step.sql.starts_with("CREATE ROLE"), "got: {}", step.sql);
        assert!(!step.destructive);
        assert!(matches!(
            step.transactional,
            TransactionConstraint::InTransaction
        ));
    }

    #[test]
    fn drop_role_emits_destructive() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::DropRole { name: id("old") },
            destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: "drops".into(),
            },
        };
        let step = emit_one(&entry);
        assert!(matches!(step.kind, StepKind::DropRole));
        assert!(step.destructive);
        assert_eq!(step.destructive_reason.as_deref(), Some("drops"));
    }
}
