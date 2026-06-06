//! Per-table policy diffing + RLS toggles.

use std::collections::BTreeMap;

use crate::diff::change::Change;
use crate::ir::table::Table;

/// Compute policy + RLS-toggle changes for a single table pair.
///
/// All five change kinds produced here are `Destructiveness::Safe` — dropping
/// a policy reveals data (reverts to surrounding RLS state) rather than
/// destroying it; toggling RLS is metadata-only.
pub fn diff_policies(target: &Table, source: &Table, out: &mut Vec<Change>) {
    // RLS toggle.
    if target.rls_enabled != source.rls_enabled {
        out.push(Change::SetTableRowSecurity {
            qname: source.qname.clone(),
            enable: source.rls_enabled,
        });
    }
    if target.rls_forced != source.rls_forced {
        out.push(Change::SetTableForceRowSecurity {
            qname: source.qname.clone(),
            force: source.rls_forced,
        });
    }

    // Policy pair-by-name diff.
    let target_map: BTreeMap<_, _> = target.policies.iter().map(|p| (&p.name, p)).collect();
    let source_map: BTreeMap<_, _> = source.policies.iter().map(|p| (&p.name, p)).collect();

    // Adds + modifies.
    for (name, src_p) in &source_map {
        match target_map.get(name) {
            None => out.push(Change::CreatePolicy {
                table: source.qname.clone(),
                policy: (*src_p).clone(),
            }),
            Some(tgt_p) => {
                if tgt_p.command != src_p.command {
                    // PG can't ALTER POLICY when the command kind changes — recreate.
                    out.push(Change::DropPolicy {
                        table: target.qname.clone(),
                        name: (*name).clone(),
                    });
                    out.push(Change::CreatePolicy {
                        table: source.qname.clone(),
                        policy: (*src_p).clone(),
                    });
                } else if *tgt_p != *src_p {
                    out.push(Change::AlterPolicy {
                        table: source.qname.clone(),
                        policy: (*src_p).clone(),
                    });
                }
            }
        }
    }

    // Drops.
    for name in target_map.keys() {
        if !source_map.contains_key(name) {
            out.push(Change::DropPolicy {
                table: target.qname.clone(),
                name: (*name).clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::grant::GrantTarget;
    use crate::ir::policy::{Policy, PolicyCommand};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
        }
    }

    fn policy(name: &str, cmd: PolicyCommand) -> Policy {
        Policy {
            name: id(name),
            permissive: true,
            command: cmd,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        }
    }

    #[test]
    fn enable_rls_emits_one_change() {
        let target = empty_table(qn("app", "t"));
        let mut source = empty_table(qn("app", "t"));
        source.rls_enabled = true;
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Change::SetTableRowSecurity { enable: true, .. }
        ));
    }

    #[test]
    fn force_rls_emits_one_change() {
        let target = empty_table(qn("app", "t"));
        let mut source = empty_table(qn("app", "t"));
        source.rls_forced = true;
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Change::SetTableForceRowSecurity { force: true, .. }
        ));
    }

    #[test]
    fn new_policy_emits_create() {
        let target = empty_table(qn("app", "t"));
        let mut source = empty_table(qn("app", "t"));
        source.policies.push(policy("p", PolicyCommand::All));
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::CreatePolicy { .. }));
    }

    #[test]
    fn removed_policy_emits_drop() {
        let mut target = empty_table(qn("app", "t"));
        target.policies.push(policy("p", PolicyCommand::All));
        let source = empty_table(qn("app", "t"));
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::DropPolicy { .. }));
    }

    #[test]
    fn changed_policy_emits_alter_only() {
        let mut target = empty_table(qn("app", "t"));
        target.policies.push(policy("p", PolicyCommand::All));
        let mut source = empty_table(qn("app", "t"));
        let mut p = policy("p", PolicyCommand::All);
        p.roles.push(GrantTarget::Role(id("readers")));
        source.policies.push(p);
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Change::AlterPolicy { .. }));
    }

    #[test]
    fn command_kind_change_recreates() {
        let mut target = empty_table(qn("app", "t"));
        target.policies.push(policy("p", PolicyCommand::Select));
        let mut source = empty_table(qn("app", "t"));
        source.policies.push(policy("p", PolicyCommand::Insert));
        let mut out = vec![];
        diff_policies(&target, &source, &mut out);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Change::DropPolicy { .. }));
        assert!(matches!(out[1], Change::CreatePolicy { .. }));
    }
}
