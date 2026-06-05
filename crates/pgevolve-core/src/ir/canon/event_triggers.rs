//! Canon for `Catalog::event_triggers`: sort + dedupe each tag filter, sort the
//! collection by name, reject duplicate names.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

/// Canonicalize all event triggers in `cat`.
///
/// - Each `tag_filter` is sorted and deduped.
/// - The collection is sorted by `name`; a duplicate name is an [`IrError`].
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for et in &mut cat.event_triggers {
        et.tag_filter.sort();
        et.tag_filter.dedup();
    }
    cat.event_triggers.sort_by(|a, b| a.name.cmp(&b.name));
    for w in cat.event_triggers.windows(2) {
        if w[0].name == w[1].name {
            return Err(IrError::DuplicateEventTrigger(w[0].name.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn et(name: &str, tags: &[&str]) -> EventTrigger {
        EventTrigger {
            name: id(name),
            event: EventTriggerEvent::DdlCommandEnd,
            tag_filter: tags.iter().map(|s| (*s).to_string()).collect(),
            function: QualifiedName::new(id("app"), id("f")),
            enabled: EventTriggerEnabled::Enabled,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn sorts_and_dedupes_tags_and_names() {
        let mut cat = Catalog::empty();
        cat.event_triggers.push(et("zeta", &["b", "a", "a"]));
        cat.event_triggers.push(et("alpha", &[]));
        run(&mut cat).unwrap();
        assert_eq!(cat.event_triggers[0].name.as_str(), "alpha");
        assert_eq!(
            cat.event_triggers[1].tag_filter,
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn rejects_duplicate_names() {
        let mut cat = Catalog::empty();
        cat.event_triggers.push(et("dup", &[]));
        cat.event_triggers.push(et("dup", &[]));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateEventTrigger(_)
        ));
    }
}
