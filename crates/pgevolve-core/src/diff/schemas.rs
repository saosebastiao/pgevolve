//! Schema-level diffing.
//!
//! Pairs schemas by [`Identifier`] name. The only field that can vary in v0.1
//! aside from existence is `comment`.

use std::collections::{BTreeMap, BTreeSet};

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::grant::GrantTarget;
use crate::ir::schema::Schema;

use super::change::Change;
use super::changeset::{ChangeSet, RevokeWithOwnerObservation, UnmanagedGrantObservation};
use super::destructiveness::Destructiveness;
use super::grants::diff_grants;
use super::owner_op::{AlterObjectOwner, OwnerObjectKind};

/// Diff schemas in `target` against `source`, appending entries to `out`.
#[allow(clippy::too_many_lines)]
pub fn diff_schemas(
    target: &Catalog,
    source: &Catalog,
    out: &mut ChangeSet,
    managed_roles: &BTreeSet<Identifier>,
) {
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

                // Owner diff.
                if let Some(source_owner) = &source_schema.owner
                    && target_schema.owner.as_ref() != Some(source_owner)
                {
                    let from = target_schema.owner.clone().unwrap_or_else(|| {
                        Identifier::from_unquoted("__unknown_owner__").expect("literal is valid")
                    });
                    out.push(
                        Change::AlterObjectOwner(AlterObjectOwner {
                            kind: OwnerObjectKind::Schema,
                            qname: crate::identifier::QualifiedName::new(
                                (*name).clone(),
                                (*name).clone(),
                            ),
                            signature: String::new(),
                            from,
                            to: source_owner.clone(),
                        }),
                        Destructiveness::Safe,
                    );
                }

                // Grant diff.
                let object_label = format!("schema {name}");
                let (to_add, to_revoke, unmanaged) =
                    diff_grants(&target_schema.grants, &source_schema.grants, managed_roles);
                for g in to_add {
                    out.push(
                        Change::GrantObjectPrivilege {
                            qname: crate::identifier::QualifiedName::new(
                                (*name).clone(),
                                (*name).clone(),
                            ),
                            kind: OwnerObjectKind::Schema,
                            signature: String::new(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                }
                for g in to_revoke {
                    if let Some(source_owner) = &source_schema.owner {
                        out.revokes_with_owner.push(RevokeWithOwnerObservation {
                            object_label: object_label.clone(),
                            privilege_label: g.privilege.sql_keyword().into(),
                            grantee: g.grantee.clone(),
                            owner: source_owner.clone(),
                        });
                    }
                    out.push(
                        Change::RevokeObjectPrivilege {
                            qname: crate::identifier::QualifiedName::new(
                                (*name).clone(),
                                (*name).clone(),
                            ),
                            kind: OwnerObjectKind::Schema,
                            signature: String::new(),
                            grant: g,
                        },
                        Destructiveness::Safe,
                    );
                }
                for g in unmanaged {
                    if let GrantTarget::Role(role_name) = &g.grantee {
                        out.unmanaged_grants.push(UnmanagedGrantObservation {
                            object_label: object_label.clone(),
                            privilege_label: g.privilege.sql_keyword().into(),
                            role_name: role_name.clone(),
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

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
        diff_schemas(&target, &source, &mut cs, &BTreeSet::new());
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
        diff_schemas(&target, &source, &mut cs, &BTreeSet::new());
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
        diff_schemas(&target, &source, &mut cs, &BTreeSet::new());
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
        diff_schemas(&target, &source, &mut cs, &BTreeSet::new());
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
        diff_schemas(&target, &source, &mut cs, &BTreeSet::new());
        assert!(cs.is_empty());
    }
}
