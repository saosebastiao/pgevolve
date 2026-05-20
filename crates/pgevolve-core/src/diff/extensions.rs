//! Differ for `Catalog::extensions`.
//!
//! Pair-by-name. Source can leave `schema` or `version` as `None` to mean
//! "don't care"; the differ never emits a change when the source side is
//! `None` for the relevant field.

use std::collections::BTreeMap;

use crate::diff::change::{Change, ExtensionChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::ir::extension::Extension;

/// Compute extension-level changes needed to converge `target` toward `source`.
///
/// Pair-by-name semantics: source extensions with `schema = None` or
/// `version = None` match any catalog value for that field.
pub fn diff_extensions(target: &[Extension], source: &[Extension], out: &mut ChangeSet) {
    let target_by_name: BTreeMap<_, _> = target.iter().map(|e| (e.name.clone(), e)).collect();
    let source_by_name: BTreeMap<_, _> = source.iter().map(|e| (e.name.clone(), e)).collect();

    // Drops: in target but not source.
    for name in target_by_name.keys() {
        if !source_by_name.contains_key(name) {
            out.push(
                Change::Extension(ExtensionChange::Drop(name.clone())),
                Destructiveness::RequiresApprovalAndDataLossWarning {
                    reason: format!(
                        "DROP EXTENSION {name} CASCADE removes every object owned by the extension."
                    ),
                },
            );
        }
    }

    // Creates and alters.
    for (name, s) in &source_by_name {
        match target_by_name.get(name) {
            None => out.push(
                Change::Extension(ExtensionChange::Create((*s).clone())),
                Destructiveness::Safe,
            ),
            Some(t) => {
                // Schema mismatch (source-None matches anything).
                if let Some(source_schema) = &s.schema
                    && t.schema.as_ref() != Some(source_schema)
                {
                    out.push(
                        Change::Extension(ExtensionChange::ReplaceWithCascade((*s).clone())),
                        Destructiveness::RequiresApprovalAndDataLossWarning {
                            reason: format!(
                                "Changing the schema of extension {name} requires DROP CASCADE; \
                                 every object owned by the extension is removed and re-created."
                            ),
                        },
                    );
                    continue;
                }
                // Version mismatch (source-None matches anything).
                if let Some(source_version) = &s.version
                    && t.version.as_ref() != Some(source_version)
                {
                    out.push(
                        Change::Extension(ExtensionChange::AlterUpdate {
                            name: name.clone(),
                            to_version: source_version.clone(),
                        }),
                        Destructiveness::Safe,
                    );
                }
                // Comment mismatch (source-None means "don't care" — consistent
                // with how schema and version are treated above). Extensions
                // often ship with auto-assigned PG descriptions; if the source
                // omits the comment field, we leave whatever PG has in place.
                if let Some(source_comment) = &s.comment
                    && t.comment.as_ref() != Some(source_comment)
                {
                    out.push(
                        Change::Extension(ExtensionChange::CommentOn {
                            name: name.clone(),
                            comment: s.comment.clone(),
                        }),
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
    use crate::identifier::Identifier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn ext(name: &str) -> Extension {
        Extension {
            name: id(name),
            schema: None,
            version: None,
            comment: None,
        }
    }

    #[test]
    fn create_when_only_in_source() {
        let mut cs = ChangeSet::new();
        diff_extensions(&[], &[ext("pgcrypto")], &mut cs);
        assert!(matches!(
            cs.iter().next().map(|e| &e.change),
            Some(Change::Extension(ExtensionChange::Create(_)))
        ));
    }

    #[test]
    fn drop_when_only_in_target() {
        let mut cs = ChangeSet::new();
        diff_extensions(&[ext("pgcrypto")], &[], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Extension(ExtensionChange::Drop(_))
        ));
        assert!(matches!(
            &first.destructiveness,
            Destructiveness::RequiresApprovalAndDataLossWarning { .. }
        ));
    }

    #[test]
    fn version_unpinned_in_source_matches_any_catalog_version() {
        let mut t = ext("pgcrypto");
        t.version = Some("1.3".into());
        let s = ext("pgcrypto"); // source unpinned
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        assert!(cs.iter().next().is_none(), "unpinned source must not diff");
    }

    #[test]
    fn version_pinned_in_source_triggers_alter_update() {
        let mut t = ext("pgcrypto");
        t.version = Some("1.3".into());
        let mut s = ext("pgcrypto");
        s.version = Some("1.4".into());
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Extension(ExtensionChange::AlterUpdate { to_version, .. })
                if to_version == "1.4"
        ));
    }

    #[test]
    fn schema_change_triggers_replace_with_cascade() {
        let mut t = ext("pgcrypto");
        t.schema = Some(id("public"));
        let mut s = ext("pgcrypto");
        s.schema = Some(id("app"));
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        let first = cs.iter().next().expect("one change");
        assert!(matches!(
            &first.change,
            Change::Extension(ExtensionChange::ReplaceWithCascade(_))
        ));
        assert!(matches!(
            &first.destructiveness,
            Destructiveness::RequiresApprovalAndDataLossWarning { .. }
        ));
    }

    #[test]
    fn schema_unpinned_in_source_skips_schema_diff() {
        let mut t = ext("pgcrypto");
        t.schema = Some(id("public"));
        let s = ext("pgcrypto"); // unpinned schema
        let mut cs = ChangeSet::new();
        diff_extensions(&[t], &[s], &mut cs);
        assert!(cs.iter().next().is_none());
    }
}
