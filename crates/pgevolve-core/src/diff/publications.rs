//! Differ for publications. Pair by name; per-publication granular diff.
//!
//! Key behaviors:
//! - Source has it, target doesn't → `CreatePublication` (Safe).
//! - Target has it, source doesn't → no auto-drop (lenient); surfaces via
//!   `unmanaged-publication` lint in Stage 9.
//! - Both have it, mode mismatch (`AllTables` ↔ `Selective`) → `ReplacePublication`
//!   (`RequiresApproval`). No per-field diffs — replace handles everything.
//! - Both have it, same `Selective` mode → per-table add/drop/set, per-schema
//!   add/drop, per-publication scalar diffs, owner (v0.3.1 lenient pattern).
//!
//! Spec: `docs/superpowers/specs/2026-05-26-publications-design.md`.

use std::collections::BTreeMap;

use crate::diff::change::{Change, PublicationChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::diff::owner_op::{AlterObjectOwner, OwnerObjectKind};
use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;
use crate::ir::publication::{Publication, PublicationScope, PublishedTable};

/// Compute granular publication changes needed to converge `target` toward
/// `source`. Appends all emitted changes to `out`.
pub fn diff_publications(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<&Identifier, &Publication> =
        target.publications.iter().map(|p| (&p.name, p)).collect();
    let source_map: BTreeMap<&Identifier, &Publication> =
        source.publications.iter().map(|p| (&p.name, p)).collect();

    // Creates: in source but not in target.
    for (name, src) in &source_map {
        if !target_map.contains_key(name) {
            out.push(
                Change::Publication(PublicationChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only: lenient — no auto-drop. Surfaces via unmanaged-publication lint.
    // Intentionally no-op loop; Stage 9 adds the unmanaged-publication lint rule.
    for _name in target_map.keys() {
        // unmanaged-publication lint (Stage 9) handles publications absent from source.
    }

    // Modifies: in both.
    for (name, src) in &source_map {
        let Some(tgt) = target_map.get(name) else {
            continue;
        };
        diff_one_publication(tgt, src, out);
    }
}

fn diff_one_publication(target: &Publication, source: &Publication, out: &mut ChangeSet) {
    // Mode mismatch → ReplacePublication (RequiresApproval).
    // A mode swap stops replication for the old set of tables, so it needs
    // explicit approval. No data is destroyed (WAL is not deleted), but
    // subscribers will see an interruption.
    let target_mode = std::mem::discriminant(&target.scope);
    let source_mode = std::mem::discriminant(&source.scope);
    if target_mode != source_mode {
        out.push(
            Change::Publication(PublicationChange::Replace {
                from: target.clone(),
                to: source.clone(),
            }),
            Destructiveness::RequiresApproval {
                reason: format!(
                    "publication {} mode swap (AllTables ↔ Selective)",
                    source.name
                ),
            },
        );
        // Do not emit per-field diffs — the replace handles everything.
        return;
    }

    // Same mode. For Selective, diff tables and schemas granularly.
    if let (
        PublicationScope::Selective {
            schemas: t_schemas,
            tables: t_tables,
        },
        PublicationScope::Selective {
            schemas: s_schemas,
            tables: s_tables,
        },
    ) = (&target.scope, &source.scope)
    {
        diff_selective_tables(&source.name, t_tables, s_tables, out);
        diff_selective_schemas(&source.name, t_schemas, s_schemas, out);
    }
    // AllTables mode has no per-table or per-schema granular diffs.

    // Per-publication scalar diffs.
    if target.publish != source.publish {
        out.push(
            Change::Publication(PublicationChange::SetPublish {
                publication: source.name.clone(),
                kinds: source.publish,
            }),
            Destructiveness::Safe,
        );
    }
    if target.publish_via_partition_root != source.publish_via_partition_root {
        out.push(
            Change::Publication(PublicationChange::SetViaRoot {
                publication: source.name.clone(),
                value: source.publish_via_partition_root,
            }),
            Destructiveness::Safe,
        );
    }
    if target.comment != source.comment {
        out.push(
            Change::Publication(PublicationChange::CommentOn {
                name: source.name.clone(),
                comment: source.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Owner: v0.3.1 lenient pattern — only emit when source declares an owner
    // and it differs from target. Source `None` = unmanaged, no change emitted.
    if let Some(s_owner) = &source.owner
        && target.owner.as_ref() != Some(s_owner)
    {
        out.push(
            Change::AlterObjectOwner(AlterObjectOwner {
                kind: OwnerObjectKind::Publication,
                id: crate::diff::owner_op::OwnedObjectId::Cluster(source.name.clone()),
                signature: String::new(),
                from: target.owner.clone(),
                to: s_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

fn diff_selective_tables(
    pub_name: &Identifier,
    target_tables: &[PublishedTable],
    source_tables: &[PublishedTable],
    out: &mut ChangeSet,
) {
    let t_map: BTreeMap<_, _> = target_tables.iter().map(|t| (&t.qname, t)).collect();
    let s_map: BTreeMap<_, _> = source_tables.iter().map(|t| (&t.qname, t)).collect();

    // Added tables: in source but not in target.
    for (qname, t) in &s_map {
        if !t_map.contains_key(qname) {
            out.push(
                Change::Publication(PublicationChange::AddTable {
                    publication: pub_name.clone(),
                    table: (*t).clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }

    // Dropped tables: in target but not in source.
    for qname in t_map.keys() {
        if !s_map.contains_key(qname) {
            out.push(
                Change::Publication(PublicationChange::DropTable {
                    publication: pub_name.clone(),
                    qname: (*qname).clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }

    // Changed tables: in both, but row_filter or columns differ.
    for (qname, src_table) in &s_map {
        let Some(tgt_table) = t_map.get(qname) else {
            continue;
        };
        if tgt_table.row_filter != src_table.row_filter || tgt_table.columns != src_table.columns {
            out.push(
                Change::Publication(PublicationChange::SetTable {
                    publication: pub_name.clone(),
                    table: (*src_table).clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }
}

fn diff_selective_schemas(
    pub_name: &Identifier,
    target_schemas: &std::collections::BTreeSet<Identifier>,
    source_schemas: &std::collections::BTreeSet<Identifier>,
    out: &mut ChangeSet,
) {
    // Added schemas: in source but not in target.
    for s in source_schemas.difference(target_schemas) {
        out.push(
            Change::Publication(PublicationChange::AddSchema {
                publication: pub_name.clone(),
                schema: s.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Dropped schemas: in target but not in source.
    for s in target_schemas.difference(source_schemas) {
        out.push(
            Change::Publication(PublicationChange::DropSchema {
                publication: pub_name.clone(),
                schema: s.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};
    use std::collections::BTreeSet;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn pub_all_tables(name: &str) -> Publication {
        Publication {
            name: id(name),
            scope: PublicationScope::AllTables,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    fn pub_selective(name: &str, tables: Vec<PublishedTable>) -> Publication {
        Publication {
            name: id(name),
            scope: PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables,
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    fn table_entry(schema: &str, name: &str) -> PublishedTable {
        PublishedTable {
            qname: qn(schema, name),
            row_filter: None,
            columns: None,
        }
    }

    fn catalog_with(pubs: Vec<Publication>) -> Catalog {
        let mut c = Catalog::empty();
        c.publications = pubs;
        c
    }

    fn run_diff(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_publications(target, source, &mut out);
        out
    }

    // ---- creates ----

    #[test]
    fn create_pub_when_source_has_it_and_target_doesnt() {
        let target = Catalog::empty();
        let source = catalog_with(vec![pub_all_tables("p")]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::Create(_))
        ));
    }

    // ---- lenient: no auto-drop ----

    #[test]
    fn no_drop_when_target_has_pub_but_source_doesnt() {
        let target = catalog_with(vec![pub_all_tables("p")]);
        let source = Catalog::empty();
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "expected no changes (lenient), got {changes:?}"
        );
    }

    // ---- mode mismatch ----

    #[test]
    fn mode_mismatch_emits_replace_publication() {
        let target = catalog_with(vec![pub_all_tables("p")]);
        let source = catalog_with(vec![pub_selective("p", vec![table_entry("app", "t")])]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        assert!(
            matches!(
                entry.change,
                Change::Publication(PublicationChange::Replace { .. })
            ),
            "expected ReplacePublication, got {:?}",
            entry.change
        );
        assert!(
            entry.destructiveness.requires_approval(),
            "mode swap must be RequiresApproval"
        );
    }

    #[test]
    fn mode_mismatch_emits_no_per_field_diffs() {
        // Even if publish differs, only ReplacePublication is emitted on mode mismatch.
        let target = catalog_with(vec![pub_all_tables("p")]);
        let mut src_pub = pub_selective("p", vec![table_entry("app", "t")]);
        src_pub.publish = PublishKinds {
            insert: true,
            update: false,
            delete: false,
            truncate: false,
        };
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1, "only ReplacePublication, no scalar diffs");
    }

    // ---- same-Selective: per-table diffs ----

    #[test]
    fn add_table_when_source_has_it_and_target_doesnt() {
        let target = catalog_with(vec![pub_selective("p", vec![])]);
        let source = catalog_with(vec![pub_selective("p", vec![table_entry("app", "t")])]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::AddTable { .. })
        ));
    }

    #[test]
    fn drop_table_when_target_has_it_and_source_doesnt() {
        let target = catalog_with(vec![pub_selective("p", vec![table_entry("app", "t")])]);
        let source = catalog_with(vec![pub_selective("p", vec![])]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::DropTable { .. })
        ));
    }

    #[test]
    fn set_table_when_columns_differ() {
        let mut src_table = table_entry("app", "t");
        src_table.columns = Some(vec![id("id"), id("name")]);
        let target = catalog_with(vec![pub_selective("p", vec![table_entry("app", "t")])]);
        let source = catalog_with(vec![pub_selective("p", vec![src_table])]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::SetTable { .. })
        ));
    }

    #[test]
    fn no_change_when_table_identical() {
        let t = table_entry("app", "orders");
        let target = catalog_with(vec![pub_selective("p", vec![t.clone()])]);
        let source = catalog_with(vec![pub_selective("p", vec![t])]);
        let changes = run_diff(&target, &source);
        assert!(changes.is_empty());
    }

    // ---- per-schema diffs ----

    #[test]
    fn add_schema_when_source_has_it_and_target_doesnt() {
        let tgt_pub = Publication {
            name: id("p"),
            scope: PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![table_entry("app", "t")],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        let src_pub = Publication {
            name: id("p"),
            scope: PublicationScope::Selective {
                schemas: BTreeSet::from([id("app")]),
                tables: vec![table_entry("app", "t")],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        let target = catalog_with(vec![tgt_pub]);
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::AddSchema { .. })
        ));
    }

    #[test]
    fn drop_schema_when_target_has_it_and_source_doesnt() {
        let tgt_pub = Publication {
            name: id("p"),
            scope: PublicationScope::Selective {
                schemas: BTreeSet::from([id("app")]),
                tables: vec![table_entry("app", "t")],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        let src_pub = Publication {
            name: id("p"),
            scope: PublicationScope::Selective {
                schemas: BTreeSet::new(),
                tables: vec![table_entry("app", "t")],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        let target = catalog_with(vec![tgt_pub]);
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::DropSchema { .. })
        ));
    }

    // ---- scalar diffs ----

    #[test]
    fn set_publish_when_publish_kinds_differ() {
        let target = catalog_with(vec![pub_all_tables("p")]);
        let mut src_pub = pub_all_tables("p");
        src_pub.publish = PublishKinds {
            insert: true,
            update: false,
            delete: false,
            truncate: false,
        };
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::SetPublish { .. })
        ));
    }

    #[test]
    fn set_via_root_when_differs() {
        let target = catalog_with(vec![pub_all_tables("p")]);
        let mut src_pub = pub_all_tables("p");
        src_pub.publish_via_partition_root = true;
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::SetViaRoot { value: true, .. })
        ));
    }

    #[test]
    fn comment_on_publication_when_comment_differs() {
        let target = catalog_with(vec![pub_all_tables("p")]);
        let mut src_pub = pub_all_tables("p");
        src_pub.comment = Some("my pub".into());
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::Publication(PublicationChange::CommentOn { .. })
        ));
    }

    // ---- owner: lenient pattern ----

    #[test]
    fn owner_change_emits_alter_object_owner() {
        let mut tgt_pub = pub_all_tables("p");
        tgt_pub.owner = Some(id("alice"));
        let mut src_pub = pub_all_tables("p");
        src_pub.owner = Some(id("bob"));
        let target = catalog_with(vec![tgt_pub]);
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::AlterObjectOwner(_)
        ));
    }

    #[test]
    fn no_owner_change_when_source_owner_is_none() {
        // Source `None` = unmanaged ownership; no change emitted.
        let mut tgt_pub = pub_all_tables("p");
        tgt_pub.owner = Some(id("alice"));
        let src_pub = pub_all_tables("p"); // owner = None
        let target = catalog_with(vec![tgt_pub]);
        let source = catalog_with(vec![src_pub]);
        let changes = run_diff(&target, &source);
        assert!(
            changes.is_empty(),
            "source owner None = unmanaged, no change expected"
        );
    }

    // ---- identity (diff against self) ----

    #[test]
    fn diff_against_self_is_empty_all_tables() {
        let c = catalog_with(vec![pub_all_tables("p")]);
        let changes = run_diff(&c, &c);
        assert!(changes.is_empty());
    }

    #[test]
    fn diff_against_self_is_empty_selective() {
        let c = catalog_with(vec![pub_selective("p", vec![table_entry("app", "users")])]);
        let changes = run_diff(&c, &c);
        assert!(changes.is_empty());
    }
}
