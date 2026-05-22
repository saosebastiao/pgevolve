//! `closed_world_references` lint rule.

use std::collections::HashSet;

use crate::ir::constraint::ConstraintKind;
use crate::ir::index::IndexParent;
use crate::lint::finding::Finding;
use crate::lint::source_tree::{ObjectKey, SourceTree};

pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let table_names: HashSet<_> = tree
        .catalog
        .tables
        .iter()
        .map(|t| t.qname.clone())
        .collect();

    // T10: also build an MV name set so that MV-parent indexes are accepted.
    let mv_names: HashSet<_> = tree
        .catalog
        .materialized_views
        .iter()
        .map(|mv| mv.qname.clone())
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

    // Indexes' parent references (table or MV). T10: branch on parent kind so
    // MV-parent indexes are validated against the MV set, not the table set,
    // closing the false-positive gap noted in the pre-T10 TODO.
    for idx in &tree.catalog.indexes {
        let parent_known = match &idx.on {
            IndexParent::Table(q) => table_names.contains(q),
            IndexParent::Mv(q) => mv_names.contains(q),
        };
        if !parent_known {
            let parent_kind = if idx.on.is_mv() {
                "materialized view"
            } else {
                "table"
            };
            let mut f = Finding::error(
                "closed_world_references",
                format!(
                    "index `{idx}` references unknown {parent_kind} `{tbl}`",
                    idx = idx.qname,
                    tbl = idx.on.qname(),
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
