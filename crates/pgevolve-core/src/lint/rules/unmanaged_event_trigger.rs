//! Warns when the catalog has an event trigger not declared in source.
//!
//! See [`super::check_unmanaged_objects`] for the shared lenient-drift policy.

use crate::ir::catalog::Catalog;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "unmanaged-event-trigger";

pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    super::check_unmanaged_objects(
        &target.event_triggers,
        &source.event_triggers,
        |e| &e.name,
        RULE_ID,
        "event trigger",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};
    use crate::lint::finding::Severity;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn make_event_trigger(name: &str) -> EventTrigger {
        EventTrigger {
            name: id(name),
            event: EventTriggerEvent::DdlCommandEnd,
            tag_filter: vec![],
            function: qn("public", "my_handler"),
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn empty_catalogs_silent() {
        let source = Catalog::empty();
        let target = Catalog::empty();
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn event_trigger_in_source_and_target_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.event_triggers.push(make_event_trigger("my_et"));
        target.event_triggers.push(make_event_trigger("my_et"));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn event_trigger_only_in_target_fires() {
        let source = Catalog::empty();
        let mut target = Catalog::empty();
        target
            .event_triggers
            .push(make_event_trigger("unmanaged_et"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("unmanaged_et"),
            "message should mention the event trigger name: {}",
            findings[0].message
        );
    }

    #[test]
    fn event_trigger_only_in_source_silent() {
        let mut source = Catalog::empty();
        let target = Catalog::empty();
        source.event_triggers.push(make_event_trigger("managed_et"));
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn partial_overlap_fires_only_for_unmanaged() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();
        source.event_triggers.push(make_event_trigger("managed_et"));
        target.event_triggers.push(make_event_trigger("managed_et"));
        target
            .event_triggers
            .push(make_event_trigger("unmanaged_et"));
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("unmanaged_et"));
    }
}
