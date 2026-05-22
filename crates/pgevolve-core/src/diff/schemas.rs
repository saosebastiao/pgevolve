//! Schema-level diffing.
//!
//! Pairs schemas by [`Identifier`] name. The only field that can vary in v0.1
//! aside from existence is `comment`.

use std::collections::BTreeMap;

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::schema::Schema;

use super::change::Change;
use super::changeset::ChangeSet;
use super::destructiveness::Destructiveness;

/// Diff schemas in `target` against `source`, appending entries to `out`.
pub fn diff_schemas(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&Identifier, &Schema> =
        target.schemas.iter().map(|s| (&s.name, s)).collect();
    let source_map: BTreeMap<&Identifier, &Schema> =
        source.schemas.iter().map(|s| (&s.name, s)).collect();

    for (name, source_schema) in &source_map {
        if !target_map.contains_key(name) {
            out.push(
                Change::CreateSchema((*source_schema).clone()),
                Destructiveness::Safe,
            );
        }
    }

    for (name, target_schema) in &target_map {
        match source_map.get(name) {
            None => {
                out.push(
                    Change::DropSchema((*name).clone()),
                    Destructiveness::RequiresApproval {
                        reason: format!("drops schema {name}"),
                    },
                );
            }
            Some(source_schema) => {
                if target_schema.comment != source_schema.comment {
                    out.push(
                        Change::AlterSchema {
                            name: (*name).clone(),
                            comment: source_schema.comment.clone(),
                        },
                        Destructiveness::Safe,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn sch(name: &str, comment: Option<&str>) -> Schema {
        Schema {
            name: id(name),
            comment: comment.map(str::to_string),
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn add_schema_emits_create_safe() {
        let target = Catalog::empty();
        let mut source = Catalog::empty();
        source.schemas.push(sch("app", None));
        let mut cs = ChangeSet::new();
        diff_schemas(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = cs.iter().next().unwrap();
        assert!(matches!(entry.change, Change::CreateSchema(_)));
        assert_eq!(entry.destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn drop_schema_emits_drop_requires_approval() {
        let mut target = Catalog::empty();
        target.schemas.push(sch("legacy", None));
        let source = Catalog::empty();
        let mut cs = ChangeSet::new();
        diff_schemas(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = cs.iter().next().unwrap();
        assert!(matches!(entry.change, Change::DropSchema(_)));
        assert!(entry.destructiveness.requires_approval());
        assert!(!entry.destructiveness.data_loss_risk());
    }

    #[test]
    fn comment_change_emits_alter_safe() {
        let mut target = Catalog::empty();
        target.schemas.push(sch("app", Some("v1")));
        let mut source = Catalog::empty();
        source.schemas.push(sch("app", Some("v2")));
        let mut cs = ChangeSet::new();
        diff_schemas(&target, &source, &mut cs);
        assert_eq!(cs.len(), 1);
        let entry = &cs.entries[0];
        match &entry.change {
            Change::AlterSchema { name, comment } => {
                assert_eq!(name, &id("app"));
                assert_eq!(comment.as_deref(), Some("v2"));
            }
            other => panic!("expected AlterSchema, got {other:?}"),
        }
        assert_eq!(entry.destructiveness, Destructiveness::Safe);
    }

    #[test]
    fn comment_clear_emits_alter_with_none() {
        let mut target = Catalog::empty();
        target.schemas.push(sch("app", Some("v1")));
        let mut source = Catalog::empty();
        source.schemas.push(sch("app", None));
        let mut cs = ChangeSet::new();
        diff_schemas(&target, &source, &mut cs);
        let entry = &cs.entries[0];
        match &entry.change {
            Change::AlterSchema { comment, .. } => assert!(comment.is_none()),
            other => panic!("expected AlterSchema, got {other:?}"),
        }
    }

    #[test]
    fn equal_schemas_emit_nothing() {
        let mut target = Catalog::empty();
        target.schemas.push(sch("app", Some("v1")));
        let mut source = Catalog::empty();
        source.schemas.push(sch("app", Some("v1")));
        let mut cs = ChangeSet::new();
        diff_schemas(&target, &source, &mut cs);
        assert!(cs.is_empty());
    }
}
