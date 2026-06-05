//! Differ for `Catalog::event_triggers`.
//!
//! Pair by name. Lenient on drop: a live event trigger absent from source is
//! NOT auto-dropped (surfaced by the `unmanaged-event-trigger` lint).
//! `event`/`tag_filter`/`function` change → Replace; `enabled` →
//! `AlterEnable`; `owner` (lenient) → `AlterOwner`; comment → `CommentOn`.

use std::collections::BTreeMap;

use crate::diff::change::{Change, EventTriggerChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::catalog::Catalog;
use crate::ir::event_trigger::EventTrigger;

/// Compute event-trigger changes to converge `target` (live) toward `source`.
pub fn diff_event_triggers(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_by: BTreeMap<_, _> = target
        .event_triggers
        .iter()
        .map(|e| (e.name.clone(), e))
        .collect();
    let source_by: BTreeMap<_, _> = source
        .event_triggers
        .iter()
        .map(|e| (e.name.clone(), e))
        .collect();

    // Source-only → Create. Lenient: target-only emits nothing (lint handles it).
    for (name, s) in &source_by {
        match target_by.get(name) {
            None => out.push(
                Change::EventTrigger(EventTriggerChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            Some(t) => emit_modify(t, s, out),
        }
    }
}

fn structural_differs(t: &EventTrigger, s: &EventTrigger) -> bool {
    t.event != s.event || t.tag_filter != s.tag_filter || t.function != s.function
}

fn emit_modify(t: &EventTrigger, s: &EventTrigger, out: &mut ChangeSet) {
    if structural_differs(t, s) {
        // Replace subsumes enable/owner/comment — the recreate carries them.
        out.push(
            Change::EventTrigger(EventTriggerChange::Replace {
                from: t.clone(),
                to: s.clone(),
            }),
            Destructiveness::Safe,
        );
        return;
    }
    if t.enabled != s.enabled {
        out.push(
            Change::EventTrigger(EventTriggerChange::AlterEnable {
                name: s.name.clone(),
                enabled: s.enabled,
            }),
            Destructiveness::Safe,
        );
    }
    // Owner is lenient: only when source declares one and it differs.
    if let Some(src_owner) = &s.owner
        && t.owner.as_ref() != Some(src_owner)
    {
        out.push(
            Change::EventTrigger(EventTriggerChange::AlterOwner {
                name: s.name.clone(),
                owner: src_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }
    if t.comment != s.comment {
        out.push(
            Change::EventTrigger(EventTriggerChange::CommentOn {
                name: s.name.clone(),
                comment: s.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::event_trigger::{EventTriggerEnabled, EventTriggerEvent};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn et(name: &str) -> EventTrigger {
        EventTrigger {
            name: id(name),
            event: EventTriggerEvent::DdlCommandEnd,
            tag_filter: vec![],
            function: QualifiedName::new(id("app"), id("f")),
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        }
    }
    fn cat(ets: Vec<EventTrigger>) -> Catalog {
        let mut c = Catalog::empty();
        c.event_triggers = ets;
        c
    }

    #[test]
    fn source_only_creates() {
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![]), &cat(vec![et("e")]), &mut out);
        assert!(matches!(
            out.entries[0].change,
            Change::EventTrigger(EventTriggerChange::Create(_))
        ));
        assert_eq!(out.entries.len(), 1);
    }

    #[test]
    fn target_only_is_lenient_no_drop() {
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![et("e")]), &cat(vec![]), &mut out);
        assert!(
            out.is_empty(),
            "live-only event trigger must NOT be auto-dropped"
        );
    }

    #[test]
    fn structural_change_replaces() {
        let t = et("e");
        let mut s = et("e");
        s.event = EventTriggerEvent::SqlDrop;
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![t]), &cat(vec![s]), &mut out);
        assert!(matches!(
            out.entries[0].change,
            Change::EventTrigger(EventTriggerChange::Replace { .. })
        ));
    }

    #[test]
    fn enabled_change_alters() {
        let t = et("e");
        let mut s = et("e");
        s.enabled = EventTriggerEnabled::Disabled;
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![t]), &cat(vec![s]), &mut out);
        assert!(matches!(
            out.entries[0].change,
            Change::EventTrigger(EventTriggerChange::AlterEnable { .. })
        ));
    }

    #[test]
    fn owner_lenient_only_when_source_declares() {
        let mut t = et("e");
        t.owner = Some(id("ops"));
        let s = et("e"); // owner None
        let mut out = ChangeSet::new();
        diff_event_triggers(&cat(vec![t]), &cat(vec![s]), &mut out);
        assert!(out.is_empty());
    }
}
