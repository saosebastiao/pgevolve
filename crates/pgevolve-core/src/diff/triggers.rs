//! Differ for `Catalog::triggers`.
//!
//! Pair-by-qname. Any structural difference emits Replace (DROP + CREATE
//! in the planner). Comment-only differences emit `CommentOn`.
//! Triggers carry no data; Replace is non-destructive.

use std::collections::BTreeMap;

use crate::diff::change::{Change, TriggerChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::trigger::Trigger;

/// Compute trigger-level changes needed to converge `target` toward `source`.
///
/// Pair-by-qname semantics: any structural difference emits `Replace`; a
/// comment-only difference emits `CommentOn`. Both are `Safe` — triggers
/// carry no data.
pub fn diff_triggers(target: &[Trigger], source: &[Trigger], out: &mut ChangeSet) {
    let target_by: BTreeMap<_, _> = target.iter().map(|t| (t.qname.clone(), t)).collect();
    let source_by: BTreeMap<_, _> = source.iter().map(|t| (t.qname.clone(), t)).collect();

    for (qname, t) in &target_by {
        if !source_by.contains_key(qname) {
            out.push(
                Change::Trigger(TriggerChange::Drop {
                    qname: qname.clone(),
                    table: t.table.clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }

    for (qname, s) in &source_by {
        match target_by.get(qname) {
            None => out.push(
                Change::Trigger(TriggerChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            Some(t) => {
                let structurally_equal = same_structure(s, t);
                let comment_changed = s.comment != t.comment;
                if structurally_equal && comment_changed {
                    out.push(
                        Change::Trigger(TriggerChange::CommentOn {
                            qname: qname.clone(),
                            table: t.table.clone(),
                            comment: s.comment.clone(),
                        }),
                        Destructiveness::Safe,
                    );
                } else if !structurally_equal {
                    out.push(
                        Change::Trigger(TriggerChange::Replace((*s).clone())),
                        Destructiveness::Safe,
                    );
                }
            }
        }
    }
}

/// True iff the two triggers are structurally identical (ignoring comment).
fn same_structure(a: &Trigger, b: &Trigger) -> bool {
    a.table == b.table
        && a.timing == b.timing
        && a.events == b.events
        && a.level == b.level
        && a.when_clause == b.when_clause
        && a.transition_tables == b.transition_tables
        && a.function_qname == b.function_qname
        && a.function_args == b.function_args
        && a.is_constraint == b.is_constraint
        && a.deferrable == b.deferrable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::constraint::Deferrable;
    use crate::ir::trigger::{TriggerEvent, TriggerLevel, TriggerTiming};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }
    fn trg(name: &str) -> Trigger {
        Trigger {
            qname: qn("app", name),
            table: qn("app", "users"),
            timing: TriggerTiming::Before,
            events: vec![TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_clause: None,
            transition_tables: vec![],
            function_qname: qn("app", "f"),
            function_args: vec![],
            is_constraint: false,
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn create_when_only_in_source() {
        let mut cs = ChangeSet::new();
        diff_triggers(&[], &[trg("t1")], &mut cs);
        assert!(matches!(
            cs.iter().next().map(|e| &e.change),
            Some(Change::Trigger(TriggerChange::Create(_)))
        ));
    }

    #[test]
    fn drop_when_only_in_target() {
        let mut cs = ChangeSet::new();
        diff_triggers(&[trg("t1")], &[], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Trigger(TriggerChange::Drop { .. })
        ));
        assert!(matches!(&first.destructiveness, Destructiveness::Safe));
    }

    #[test]
    fn timing_change_emits_replace() {
        let t = trg("t1");
        let mut s = trg("t1");
        s.timing = TriggerTiming::After;
        let mut cs = ChangeSet::new();
        diff_triggers(&[t], &[s], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Trigger(TriggerChange::Replace(_))
        ));
    }

    #[test]
    fn comment_only_change_emits_comment_on() {
        let t = trg("t1");
        let mut s = trg("t1");
        s.comment = Some("docs".into());
        let mut cs = ChangeSet::new();
        diff_triggers(&[t], &[s], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Trigger(TriggerChange::CommentOn { .. })
        ));
    }

    #[test]
    fn identical_triggers_emit_no_change() {
        let t = trg("t1");
        let s = trg("t1");
        let mut cs = ChangeSet::new();
        diff_triggers(&[t], &[s], &mut cs);
        assert!(cs.iter().next().is_none());
    }
}
