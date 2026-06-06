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

/// Emit one or more [`RawStep`]s per [`ClusterChange`]. All cluster ops run
/// [`InTransaction`](TransactionConstraint::InTransaction).
///
/// Most changes map to exactly one step; `CreateTablespace` with a comment
/// expands to two (the `CREATE`, then a follow-up `COMMENT ON`).
#[must_use]
pub fn emit_cluster_changes(cs: &ClusterChangeSet) -> Vec<RawStep> {
    cs.entries.iter().flat_map(emit_one).collect()
}

fn emit_one(entry: &ClusterChangeEntry) -> Vec<RawStep> {
    match &entry.change {
        ClusterChange::CreateTablespace(ts) => {
            let mut steps = vec![RawStep {
                step_no: 0,
                kind: StepKind::CreateTablespace,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![],
                sql: sql::create_tablespace(ts),
                transactional: TransactionConstraint::InTransaction,
            }];
            // Owner + options ride inline in CREATE; only the comment needs a
            // follow-up COMMENT ON step.
            if let Some(comment) = &ts.comment {
                steps.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnTablespace,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![],
                    sql: sql::comment_on_tablespace(&ts.name, Some(comment)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
            steps
        }
        ClusterChange::DropTablespace { name } => vec![RawStep {
            step_no: 0,
            kind: StepKind::DropTablespace,
            destructive: true,
            destructive_reason: entry.destructiveness.reason().map(str::to_owned),
            intent_id: None,
            targets: vec![],
            sql: sql::drop_tablespace(name),
            transactional: TransactionConstraint::InTransaction,
        }],
        ClusterChange::AlterTablespaceOwner { name, owner } => vec![RawStep {
            step_no: 0,
            kind: StepKind::AlterTablespaceOwner,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::alter_tablespace_owner(name, owner),
            transactional: TransactionConstraint::InTransaction,
        }],
        ClusterChange::SetTablespaceOptions { name, options } => vec![RawStep {
            step_no: 0,
            kind: StepKind::SetTablespaceOptions,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::alter_tablespace_set(name, options),
            transactional: TransactionConstraint::InTransaction,
        }],
        ClusterChange::CommentOnTablespace { name, comment } => vec![RawStep {
            step_no: 0,
            kind: StepKind::CommentOnTablespace,
            destructive: false,
            destructive_reason: None,
            intent_id: None,
            targets: vec![],
            sql: sql::comment_on_tablespace(name, comment.as_deref()),
            transactional: TransactionConstraint::InTransaction,
        }],
        _ => vec![emit_role_change(entry)],
    }
}

fn emit_role_change(entry: &ClusterChangeEntry) -> RawStep {
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

        // Tablespace variants are handled by `emit_one` before dispatching here.
        ClusterChange::CreateTablespace(_)
        | ClusterChange::DropTablespace { .. }
        | ClusterChange::AlterTablespaceOwner { .. }
        | ClusterChange::SetTablespaceOptions { .. }
        | ClusterChange::CommentOnTablespace { .. } => {
            unreachable!("tablespace changes are emitted by emit_one, not emit_role_change")
        }
    }
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
    use crate::ir::cluster::tablespace::Tablespace;
    use std::collections::BTreeMap;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn only(entry: &ClusterChangeEntry) -> RawStep {
        let steps = emit_one(entry);
        assert_eq!(steps.len(), 1, "expected exactly one step, got {steps:?}");
        steps.into_iter().next().expect("one step")
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
        let step = only(&entry);
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
        let step = only(&entry);
        assert!(matches!(step.kind, StepKind::DropRole));
        assert!(step.destructive);
        assert_eq!(step.destructive_reason.as_deref(), Some("drops"));
    }

    fn ts(name: &str, location: &str) -> Tablespace {
        Tablespace {
            name: id(name),
            location: location.to_string(),
            owner: None,
            options: BTreeMap::new(),
            comment: None,
        }
    }

    #[test]
    fn create_tablespace_without_comment_emits_one_step() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::CreateTablespace(ts("fast", "/x")),
            destructiveness: Destructiveness::Safe,
        };
        let step = only(&entry);
        assert!(matches!(step.kind, StepKind::CreateTablespace));
        assert!(
            step.sql.starts_with("CREATE TABLESPACE fast"),
            "got: {}",
            step.sql
        );
        assert!(!step.destructive);
    }

    #[test]
    fn create_tablespace_with_comment_emits_two_steps() {
        let mut t = ts("fast", "/x");
        t.comment = Some("fast storage".into());
        let entry = ClusterChangeEntry {
            change: ClusterChange::CreateTablespace(t),
            destructiveness: Destructiveness::Safe,
        };
        let steps = emit_one(&entry);
        assert_eq!(steps.len(), 2, "got: {steps:?}");
        assert!(matches!(steps[0].kind, StepKind::CreateTablespace));
        assert!(steps[0].sql.starts_with("CREATE TABLESPACE fast"));
        assert!(matches!(steps[1].kind, StepKind::CommentOnTablespace));
        assert_eq!(
            steps[1].sql,
            "COMMENT ON TABLESPACE fast IS 'fast storage';"
        );
    }

    #[test]
    fn drop_tablespace_emits_one_destructive_step() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::DropTablespace { name: id("old") },
            destructiveness: Destructiveness::RequiresApprovalAndDataLossWarning {
                reason: "drops a tablespace".into(),
            },
        };
        let step = only(&entry);
        assert!(matches!(step.kind, StepKind::DropTablespace));
        assert!(step.destructive);
        assert_eq!(
            step.destructive_reason.as_deref(),
            Some("drops a tablespace")
        );
        assert_eq!(step.sql, "DROP TABLESPACE old;");
    }

    #[test]
    fn alter_tablespace_owner_emits_safe_step() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::AlterTablespaceOwner {
                name: id("fast"),
                owner: id("dba"),
            },
            destructiveness: Destructiveness::Safe,
        };
        let step = only(&entry);
        assert!(matches!(step.kind, StepKind::AlterTablespaceOwner));
        assert!(!step.destructive);
        assert_eq!(step.sql, "ALTER TABLESPACE fast OWNER TO dba;");
    }

    #[test]
    fn set_tablespace_options_emits_safe_step() {
        let mut options = BTreeMap::new();
        options.insert("seq_page_cost".to_string(), "1.5".to_string());
        let entry = ClusterChangeEntry {
            change: ClusterChange::SetTablespaceOptions {
                name: id("fast"),
                options,
            },
            destructiveness: Destructiveness::Safe,
        };
        let step = only(&entry);
        assert!(matches!(step.kind, StepKind::SetTablespaceOptions));
        assert!(!step.destructive);
        assert_eq!(step.sql, "ALTER TABLESPACE fast SET (seq_page_cost = 1.5);");
    }

    #[test]
    fn comment_on_tablespace_emits_safe_step() {
        let entry = ClusterChangeEntry {
            change: ClusterChange::CommentOnTablespace {
                name: id("fast"),
                comment: None,
            },
            destructiveness: Destructiveness::Safe,
        };
        let step = only(&entry);
        assert!(matches!(step.kind, StepKind::CommentOnTablespace));
        assert_eq!(step.sql, "COMMENT ON TABLESPACE fast IS NULL;");
    }
}
