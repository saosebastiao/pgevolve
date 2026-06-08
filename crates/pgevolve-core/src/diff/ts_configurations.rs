//! Differ for `Catalog::ts_configurations`.
//!
//! Text-search configurations are **managed** (schema-scoped): a live
//! configuration that is absent from source IS auto-dropped — unlike lenient
//! objects such as event triggers or statistics.
//!
//! Identity is `qname`. The `BTreeMap` key is `qname.render_sql()` — a stable,
//! canonical representation.
//!
//! Logic summary:
//! - source-only → `Create` (Safe).
//! - target-only → `Drop` (Safe — configurations carry no data).
//! - both present, `parser` differs → `Replace` (Safe, subsumes
//!   mappings/owner/comment; PG has no `ALTER … PARSER`).
//! - else: per-token-type mapping diff (sorted by `token_type` for
//!   determinism):
//!   - source-only token → `AddMapping` (Safe).
//!   - both but `dictionaries` differ → `AlterMapping` (Safe).
//!   - target-only token → `DropMapping` (Safe).
//! - owner lenient (only when source declares one) → `AlterOwner` (Safe).
//! - comment differs → `CommentOn` (Safe).

use std::collections::BTreeMap;

use crate::diff::change::{Change, TsConfigurationChange};
use crate::diff::changeset::ChangeSet;
use crate::diff::destructiveness::Destructiveness;
use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::text_search::configuration::{TsConfiguration, TsMapping};

/// Compute text-search configuration changes needed to converge `target` (live)
/// toward `source`.
///
/// Appends all emitted changes to `out`. Configurations are **managed**: a
/// target-only configuration (absent from source) IS auto-dropped.
pub fn diff_ts_configurations(target: &Catalog, source: &Catalog, out: &mut ChangeSet) {
    let target_map: BTreeMap<String, &TsConfiguration> = target
        .ts_configurations
        .iter()
        .map(|c| (c.qname.render_sql(), c))
        .collect();
    let source_map: BTreeMap<String, &TsConfiguration> = source
        .ts_configurations
        .iter()
        .map(|c| (c.qname.render_sql(), c))
        .collect();

    // Source-only → Create.
    for (key, src) in &source_map {
        if !target_map.contains_key(key) {
            out.push(
                Change::TsConfiguration(TsConfigurationChange::Create((*src).clone())),
                Destructiveness::Safe,
            );
        }
    }

    // Target-only → Drop (managed, not lenient).
    for (key, tgt) in &target_map {
        if !source_map.contains_key(key) {
            out.push(
                Change::TsConfiguration(TsConfigurationChange::Drop {
                    qname: tgt.qname.clone(),
                }),
                Destructiveness::Safe,
            );
        }
    }

    // Both present → granular diff.
    for (key, src) in &source_map {
        let Some(tgt) = target_map.get(key) else {
            continue;
        };
        emit_modify(tgt, src, out);
    }
}

fn emit_modify(t: &TsConfiguration, s: &TsConfiguration, out: &mut ChangeSet) {
    // Parser change requires DROP + CREATE; subsumes mappings/owner/comment.
    if t.parser != s.parser {
        out.push(
            Change::TsConfiguration(TsConfigurationChange::Replace {
                from: t.clone(),
                to: s.clone(),
            }),
            Destructiveness::Safe,
        );
        return;
    }

    // Diff mappings by token_type using a BTreeMap for deterministic ordering.
    diff_mappings(&s.qname, &t.mappings, &s.mappings, out);

    // Owner is lenient: only when source declares one and it differs.
    if let Some(src_owner) = &s.owner
        && t.owner.as_ref() != Some(src_owner)
    {
        out.push(
            Change::TsConfiguration(TsConfigurationChange::AlterOwner {
                qname: s.qname.clone(),
                owner: src_owner.clone(),
            }),
            Destructiveness::Safe,
        );
    }

    // Comment differs → CommentOn (both directions: set or clear).
    if t.comment != s.comment {
        out.push(
            Change::TsConfiguration(TsConfigurationChange::CommentOn {
                qname: s.qname.clone(),
                comment: s.comment.clone(),
            }),
            Destructiveness::Safe,
        );
    }
}

/// Diff token-type → dictionary-chain mappings between `target_mappings` (live)
/// and `source_mappings` (desired). Emits Add/Alter/Drop in `token_type`
/// order (`BTreeMap` guarantees ascending order).
fn diff_mappings(
    qname: &QualifiedName,
    target_mappings: &[TsMapping],
    source_mappings: &[TsMapping],
    out: &mut ChangeSet,
) {
    let t_map: BTreeMap<&str, &Vec<QualifiedName>> = target_mappings
        .iter()
        .map(|m| (m.token_type.as_str(), &m.dictionaries))
        .collect();
    let s_map: BTreeMap<&str, &Vec<QualifiedName>> = source_mappings
        .iter()
        .map(|m| (m.token_type.as_str(), &m.dictionaries))
        .collect();

    // Iterate over all token types in source (sorted by BTreeMap).
    for (token_type, s_dicts) in &s_map {
        match t_map.get(token_type) {
            None => {
                // Source-only → AddMapping.
                out.push(
                    Change::TsConfiguration(TsConfigurationChange::AddMapping {
                        qname: qname.clone(),
                        token_type: (*token_type).to_owned(),
                        dictionaries: (*s_dicts).clone(),
                    }),
                    Destructiveness::Safe,
                );
            }
            Some(t_dicts) if t_dicts != s_dicts => {
                // Both present, dictionaries differ → AlterMapping.
                out.push(
                    Change::TsConfiguration(TsConfigurationChange::AlterMapping {
                        qname: qname.clone(),
                        token_type: (*token_type).to_owned(),
                        dictionaries: (*s_dicts).clone(),
                    }),
                    Destructiveness::Safe,
                );
            }
            Some(_) => {
                // Identical — no change.
            }
        }
    }

    // Target-only token types → DropMapping (iterate in sorted order).
    for token_type in t_map.keys() {
        if !s_map.contains_key(token_type) {
            out.push(
                Change::TsConfiguration(TsConfigurationChange::DropMapping {
                    qname: qname.clone(),
                    token_type: (*token_type).to_owned(),
                }),
                Destructiveness::Safe,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::change::{Change, TsConfigurationChange};
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::text_search::configuration::{TsConfiguration, TsMapping};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// Build a minimal configuration with the default parser and no mappings.
    fn basic_config(name: &str) -> TsConfiguration {
        TsConfiguration {
            qname: qn("app", name),
            parser: qn("pg_catalog", "default"),
            mappings: vec![],
            owner: None,
            comment: None,
        }
    }

    fn mapping(token_type: &str, dicts: Vec<QualifiedName>) -> TsMapping {
        TsMapping {
            token_type: token_type.to_owned(),
            dictionaries: dicts,
        }
    }

    fn cat(configs: Vec<TsConfiguration>) -> Catalog {
        let mut c = Catalog::empty();
        c.ts_configurations = configs;
        c
    }

    fn run(target: &Catalog, source: &Catalog) -> ChangeSet {
        let mut out = ChangeSet::new();
        diff_ts_configurations(target, source, &mut out);
        out
    }

    // ---- source-only → Create ----

    #[test]
    fn source_only_creates() {
        let changes = run(&cat(vec![]), &cat(vec![basic_config("english")]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsConfiguration(TsConfigurationChange::Create(_))
        ));
    }

    // ---- target-only → Drop (managed, NOT lenient) ----

    #[test]
    fn target_only_drops() {
        let changes = run(&cat(vec![basic_config("english")]), &cat(vec![]));
        assert_eq!(
            changes.len(),
            1,
            "managed configuration must emit Drop when absent from source"
        );
        assert!(
            matches!(
                changes.iter().next().unwrap().change,
                Change::TsConfiguration(TsConfigurationChange::Drop { .. })
            ),
            "expected Drop, got {:?}",
            changes.iter().next().unwrap().change
        );
    }

    // ---- parser change → Replace (from=target/live, to=source/desired) ----

    #[test]
    fn different_parser_replaces() {
        let t = basic_config("english");
        let mut s = basic_config("english");
        s.parser = qn("app", "custom_parser");
        let changes = run(&cat(vec![t.clone()]), &cat(vec![s.clone()]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        match &entry.change {
            Change::TsConfiguration(TsConfigurationChange::Replace { from, to }) => {
                assert_eq!(from, &t, "from must be the target (live) configuration");
                assert_eq!(to, &s, "to must be the source (desired) configuration");
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    // ---- parser Replace short-circuits mappings/owner/comment ----

    #[test]
    fn replace_subsumes_mappings_owner_and_comment() {
        let t = basic_config("english");
        let mut s = basic_config("english");
        s.parser = qn("app", "custom_parser"); // structural
        s.mappings = vec![mapping("word", vec![qn("app", "english_stem")])];
        s.owner = Some(id("alice"));
        s.comment = Some("custom config".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(
            changes.len(),
            1,
            "Replace must subsume mappings + owner + comment"
        );
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsConfiguration(TsConfigurationChange::Replace { .. })
        ));
    }

    // ---- AddMapping: token in source only ----

    #[test]
    fn add_mapping_when_token_in_source_only() {
        let t = basic_config("english");
        let mut s = basic_config("english");
        s.mappings = vec![mapping("word", vec![qn("app", "english_stem")])];
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        match &entry.change {
            Change::TsConfiguration(TsConfigurationChange::AddMapping {
                token_type,
                dictionaries,
                ..
            }) => {
                assert_eq!(token_type, "word");
                assert_eq!(dictionaries, &vec![qn("app", "english_stem")]);
            }
            other => panic!("expected AddMapping, got {other:?}"),
        }
    }

    // ---- AlterMapping: dict chain differs ----

    #[test]
    fn alter_mapping_when_dictionaries_differ() {
        let mut t = basic_config("english");
        t.mappings = vec![mapping("word", vec![qn("app", "english_stem")])];
        let mut s = basic_config("english");
        s.mappings = vec![mapping(
            "word",
            vec![qn("app", "english_stem"), qn("pg_catalog", "simple")],
        )];
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        match &entry.change {
            Change::TsConfiguration(TsConfigurationChange::AlterMapping {
                token_type,
                dictionaries,
                ..
            }) => {
                assert_eq!(token_type, "word");
                assert_eq!(
                    dictionaries,
                    &vec![qn("app", "english_stem"), qn("pg_catalog", "simple")]
                );
            }
            other => panic!("expected AlterMapping, got {other:?}"),
        }
    }

    // ---- DropMapping: token in target only ----

    #[test]
    fn drop_mapping_when_token_in_target_only() {
        let mut t = basic_config("english");
        t.mappings = vec![mapping("word", vec![qn("app", "english_stem")])];
        let s = basic_config("english"); // no mappings
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        let entry = changes.iter().next().unwrap();
        match &entry.change {
            Change::TsConfiguration(TsConfigurationChange::DropMapping { token_type, .. }) => {
                assert_eq!(token_type, "word");
            }
            other => panic!("expected DropMapping, got {other:?}"),
        }
    }

    // ---- Mixed: one add + one alter + one drop in a single config ----

    #[test]
    fn mixed_add_alter_drop_mapping() {
        // Target has: "asciiword" → [simple], "word" → [english_stem]
        // Source has: "asciiword" → [english_stem, simple], "numword" → [simple]
        //
        // Expected:
        //   AddMapping "numword"
        //   AlterMapping "asciiword"  (dict chain changed)
        //   DropMapping "word"
        let mut t = basic_config("english");
        t.mappings = vec![
            mapping("asciiword", vec![qn("pg_catalog", "simple")]),
            mapping("word", vec![qn("app", "english_stem")]),
        ];
        let mut s = basic_config("english");
        s.mappings = vec![
            mapping(
                "asciiword",
                vec![qn("app", "english_stem"), qn("pg_catalog", "simple")],
            ),
            mapping("numword", vec![qn("pg_catalog", "simple")]),
        ];

        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(
            changes.len(),
            3,
            "expected 3 changes (add+alter+drop), got: {changes:?}"
        );

        let kinds: Vec<_> = changes
            .iter()
            .map(|e| match &e.change {
                Change::TsConfiguration(c) => c.clone(),
                other => panic!("unexpected change kind: {other:?}"),
            })
            .collect();

        assert!(
            kinds.iter().any(|c| matches!(
                c,
                TsConfigurationChange::AddMapping { token_type, .. } if token_type == "numword"
            )),
            "expected AddMapping for numword"
        );
        assert!(
            kinds.iter().any(|c| matches!(
                c,
                TsConfigurationChange::AlterMapping { token_type, .. } if token_type == "asciiword"
            )),
            "expected AlterMapping for asciiword"
        );
        assert!(
            kinds.iter().any(|c| matches!(
                c,
                TsConfigurationChange::DropMapping { token_type, .. } if token_type == "word"
            )),
            "expected DropMapping for word"
        );
    }

    // ---- owner change → AlterOwner (lenient) ----

    #[test]
    fn owner_change_emits_alter_owner() {
        let mut t = basic_config("english");
        t.owner = Some(id("alice"));
        let mut s = basic_config("english");
        s.owner = Some(id("bob"));
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsConfiguration(TsConfigurationChange::AlterOwner { .. })
        ));
    }

    #[test]
    fn source_owner_none_no_alter_owner() {
        let mut t = basic_config("english");
        t.owner = Some(id("alice"));
        let s = basic_config("english"); // owner = None
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert!(
            changes.is_empty(),
            "source owner None = unmanaged, no change expected"
        );
    }

    // ---- comment change → CommentOn ----

    #[test]
    fn comment_change_emits_comment_on() {
        let t = basic_config("english");
        let mut s = basic_config("english");
        s.comment = Some("English text search configuration".into());
        let changes = run(&cat(vec![t]), &cat(vec![s]));
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes.iter().next().unwrap().change,
            Change::TsConfiguration(TsConfigurationChange::CommentOn { .. })
        ));
    }

    // ---- identical → no changes ----

    #[test]
    fn identical_configurations_produce_no_changes() {
        let mut cfg = basic_config("english");
        cfg.mappings = vec![
            mapping("asciiword", vec![qn("app", "english_stem")]),
            mapping("word", vec![qn("app", "english_stem")]),
        ];
        cfg.owner = Some(id("alice"));
        cfg.comment = Some("English config".into());
        let c = cat(vec![cfg]);
        let changes = run(&c, &c);
        assert!(changes.is_empty());
    }
}
