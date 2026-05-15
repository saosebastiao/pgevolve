//! Universal lint rules. Spec §12.1.
//!
//! Most of §12.1 is already enforced by the parser (unsupported kinds,
//! duplicate qnames, schema qualification, ALTER outside the FK whitelist).
//! Two rules remain that the parser can't enforce:
//!
//! - **`managed_schemas_match`** — every schema declared in source has a
//!   matching `managed.schemas` entry, and vice versa.
//! - **`closed_world_references`** — every FK target table exists in source.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::ManagedConfig;
use super::finding::Finding;
use super::source_tree::{ObjectKey, SourceTree};
use crate::ir::catalog::Catalog;
use crate::ir::constraint::ConstraintKind;

/// Run every universal rule.
pub fn check_universal(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(managed_schemas_match(tree, managed));
    out.extend(no_duplicate_qnames(tree));
    out.extend(closed_world_references(tree));
    out
}

fn managed_schemas_match(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    let in_source: HashSet<_> = tree
        .catalog
        .schemas
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let in_config: HashSet<_> = managed.schemas.iter().cloned().collect();

    // managed.schemas may be empty (some projects don't list them and rely
    // on filter at apply time). Only flag mismatches when the user has
    // explicitly populated the list.
    if in_config.is_empty() {
        return out;
    }
    for s in &in_source {
        if !in_config.contains(s) {
            out.push(Finding::error(
                "managed_schemas_match",
                format!(
                    "schema `{s}` is declared in source but not listed in `[managed].schemas`",
                ),
            ));
        }
    }
    for s in &in_config {
        if !in_source.contains(s) {
            out.push(Finding::error(
                "managed_schemas_match",
                format!(
                    "schema `{s}` is listed in `[managed].schemas` but not declared in source",
                ),
            ));
        }
    }
    out
}

/// Duplicate qnames are already rejected by `parse_directory`; this is a
/// belt-and-suspenders check that confirms the same invariant on any
/// `SourceTree` regardless of how it was constructed.
fn no_duplicate_qnames(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen: HashSet<&ObjectKey> = HashSet::new();
    for key in tree.objects() {
        if !seen.insert(key) {
            out.push(Finding::error(
                "no_duplicate_qnames",
                format!("duplicate object: {key}"),
            ));
        }
    }
    out
}

fn closed_world_references(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let table_names: HashSet<_> = tree
        .catalog
        .tables
        .iter()
        .map(|t| t.qname.clone())
        .collect();

    for table in &tree.catalog.tables {
        for c in &table.constraints {
            if let ConstraintKind::ForeignKey(fk) = &c.kind
                && !table_names.contains(&fk.referenced_table)
            {
                let loc = tree
                    .object_locations
                    .get(&ObjectKey::Table(table.qname.clone()))
                    .cloned();
                let mut f = Finding::error(
                    "closed_world_references",
                    format!(
                        "FK `{constraint}` on `{owner}` references unknown table `{ref_table}`",
                        constraint = c.qname.name,
                        owner = table.qname,
                        ref_table = fk.referenced_table,
                    ),
                );
                if let Some(l) = loc {
                    f = f.at(l);
                }
                out.push(f);
            }
        }
    }

    // Indexes' table references.
    for idx in &tree.catalog.indexes {
        if !table_names.contains(&idx.table) {
            let mut f = Finding::error(
                "closed_world_references",
                format!(
                    "index `{idx}` references unknown table `{tbl}`",
                    idx = idx.qname,
                    tbl = idx.table,
                ),
            );
            if let Some(loc) = tree
                .object_locations
                .get(&ObjectKey::Index(idx.qname.clone()))
            {
                f = f.at(loc.clone());
            }
            out.push(f);
        }
    }

    // Sequences' OWNED BY references.
    for seq in &tree.catalog.sequences {
        if let Some(owner) = &seq.owned_by
            && !table_names.contains(&owner.table)
        {
            let mut f = Finding::error(
                "closed_world_references",
                format!(
                    "sequence `{seq}` is OWNED BY unknown table `{tbl}`",
                    seq = seq.qname,
                    tbl = owner.table,
                ),
            );
            if let Some(loc) = tree
                .object_locations
                .get(&ObjectKey::Sequence(seq.qname.clone()))
            {
                f = f.at(loc.clone());
            }
            out.push(f);
        }
    }

    out
}

/// All rule IDs that carry [`Severity::LintAtPlan`].
///
/// Preflight uses this to warn about waivers that reference unknown rule IDs
/// (typos, stale waivers for renamed rules). Add new `LintAtPlan` rule IDs
/// here when introducing them; remove stale ones when a rule is deleted.
pub const LINT_AT_PLAN_RULES: &[&str] = &["column-position-drift"];

/// Run all drift-detection rules that compare `source` against a `target`
/// catalog (e.g. the live database). Returns a list of [`Finding`]s.
///
/// This is the entry point for lint rules that need both the source and target
/// catalogs, as opposed to [`check_universal`] which only needs the source tree.
pub fn run_drift_lints(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut out = Vec::new();
    column_position_drift_rule(source, target, &mut out);
    out
}

/// `column-position-drift` — fires when a table's column order in source
/// disagrees with the target catalog's column order, with no other structural
/// change accompanying that column.
///
/// Source is canonical; the lint says "your DB has columns in a different
/// order." Severity is `LintAtPlan` — plan refuses unless the finding is
/// waived in `intent.toml` (waiver mechanism in Task 8).
fn column_position_drift_rule(
    source: &Catalog,
    target: &Catalog,
    out: &mut Vec<Finding>,
) {
    let target_tables: BTreeMap<_, _> =
        target.tables.iter().map(|t| (t.qname.clone(), t)).collect();

    for source_table in &source.tables {
        let Some(target_table) = target_tables.get(&source_table.qname) else {
            continue;
        };
        let source_names: Vec<_> =
            source_table.columns.iter().map(|c| c.name.clone()).collect();
        let target_names: Vec<_> =
            target_table.columns.iter().map(|c| c.name.clone()).collect();

        // Only compare columns that exist in both catalogs. Added or removed
        // columns do not constitute position drift — those are handled by the
        // planner.
        let source_set: BTreeSet<_> = source_names.iter().cloned().collect();
        let target_set: BTreeSet<_> = target_names.iter().cloned().collect();
        let common: BTreeSet<_> = source_set.intersection(&target_set).cloned().collect();

        let source_order: Vec<_> = source_names
            .iter()
            .filter(|n| common.contains(n))
            .collect();
        let target_order: Vec<_> = target_names
            .iter()
            .filter(|n| common.contains(n))
            .collect();

        if source_order != target_order {
            out.push(Finding::lint_at_plan(
                "column-position-drift",
                format!(
                    "{}: column position drift. source order [{}] vs catalog order [{}]",
                    source_table.qname,
                    source_order
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    target_order
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn empty_tree(catalog: Catalog) -> SourceTree {
        SourceTree::new(catalog, std::collections::HashMap::new())
    }

    #[test]
    fn managed_schemas_match_passes_when_lists_align() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        let tree = empty_tree(c);
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check_universal(&tree, &managed);
        assert!(f.is_empty(), "got findings: {f:?}");
    }

    #[test]
    fn managed_schemas_match_flags_missing_source_schema() {
        let tree = empty_tree(Catalog::empty());
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check_universal(&tree, &managed);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "managed_schemas_match");
    }

    #[test]
    fn managed_schemas_match_flags_unlisted_source_schema() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("audit")));
        let tree = empty_tree(c);
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check_universal(&tree, &managed);
        // Two findings: app missing in source, audit not listed in managed.
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn managed_schemas_match_skips_when_managed_is_empty() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("any")));
        let tree = empty_tree(c);
        let managed = ManagedConfig::default();
        let f = check_universal(&tree, &managed);
        // Other universal rules may still fire, but managed_schemas_match
        // must be silent.
        assert!(f.iter().all(|x| x.rule != "managed_schemas_match"));
    }

    #[test]
    fn closed_world_references_flags_dangling_fk() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![Constraint {
                qname: qn("app", "users_fk"),
                kind: ConstraintKind::ForeignKey(ForeignKey {
                    columns: vec![id("orgs_id")],
                    referenced_table: qn("app", "orgs"), // doesn't exist
                    referenced_columns: vec![id("id")],
                    on_update: ReferentialAction::NoAction,
                    on_delete: ReferentialAction::NoAction,
                    match_type: FkMatchType::Simple,
                }),
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count_cwr = findings
            .iter()
            .filter(|f| f.rule == "closed_world_references")
            .count();
        assert_eq!(count_cwr, 1);
    }
}
