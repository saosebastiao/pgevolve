//! L8 — dep-graph golden.
//!
//! Renders the AST-derived dep graph for the fixture's source IR to
//! DOT format and byte-compares against `expected/dep-graph.dot`.
//! Default-on; opt-out for trivial fixtures.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId, build_create_graph};

/// Assert that the rendered dep graph for `source` matches the golden file.
pub fn assert_dep_graph_golden(
    source: &Catalog,
    fixture_dir: &Path,
    golden_rel: &str,
) -> Result<()> {
    let graph = build_create_graph(source);
    let edges: Vec<DepEdge> = graph.dep_edges().collect();
    let actual = render_dot(&edges);
    let golden_path = fixture_dir.join(golden_rel);
    let expected = std::fs::read_to_string(&golden_path)
        .with_context(|| format!("read {}", golden_path.display()))?;
    let actual_norm = normalize_dot(&actual);
    let expected_norm = normalize_dot(&expected);
    if actual_norm != expected_norm {
        anyhow::bail!(
            "L8 dep-graph golden mismatch in {}:\n--- expected\n{}\n+++ actual\n{}",
            golden_path.display(),
            expected_norm,
            actual_norm,
        );
    }
    Ok(())
}

/// Render the dep graph as DOT.
///
/// Duplicated from `pgevolve/src/commands/graph.rs` because
/// `pgevolve-conformance` cannot depend on the binary crate.
/// Keep byte-identical in behaviour to the binary's `render_dot`; if the
/// binary's renderer drifts, `cargo xtask bless --conformance` will catch it.
pub fn render_dot(edges: &[DepEdge]) -> String {
    let mut out = String::from(
        "digraph pgevolve_deps {\n  rankdir=LR;\n  node [shape=box, fontname=Helvetica];\n",
    );

    let mut nodes: BTreeSet<String> = BTreeSet::default();
    for e in edges {
        nodes.insert(node_label(&e.from));
        nodes.insert(node_label(&e.to));
    }
    for n in &nodes {
        let _ = writeln!(out, "  \"{n}\";");
    }

    let mut sorted = edges.to_vec();
    sorted.sort();
    for e in sorted {
        let style = match e.source {
            DepSource::Structural => "solid",
            DepSource::AstExtracted => "dashed",
            DepSource::AstDeclared => "dotted",
        };
        let _ = writeln!(
            out,
            "  \"{}\" -> \"{}\" [style={style}];",
            node_label(&e.from),
            node_label(&e.to)
        );
    }
    out.push_str("}\n");
    out
}

fn node_label(n: &NodeId) -> String {
    match n {
        NodeId::Schema(s) => format!("schema:{}", s.as_str()),
        NodeId::Table(q) => format!("table:{q}"),
        NodeId::Index(q) => format!("index:{q}"),
        NodeId::Sequence(q) => format!("sequence:{q}"),
        NodeId::Constraint { table, name } => {
            format!("constraint:{table}.{}", name.as_str())
        }
        NodeId::View(q) => format!("view:{q}"),
        NodeId::Mv(q) => format!("mv:{q}"),
        NodeId::Type(q) => format!("type:{q}"),
        NodeId::Function(q, args) => format!(
            "function:{}({})",
            q,
            args.types
                .iter()
                .map(pgevolve_core::ir::column_type::ColumnType::render_sql)
                .collect::<Vec<_>>()
                .join(",")
        ),
        NodeId::Procedure(q) => format!("procedure:{q}"),
        NodeId::Extension(n) => format!("extension:{}", n.as_str()),
        NodeId::Trigger(q) => format!("trigger:{q}"),
        NodeId::Publication(n) => format!("publication:{}", n.as_str()),
        NodeId::Subscription(n) => format!("subscription:{}", n.as_str()),
        NodeId::Statistic(q) => format!("statistic:{q}"),
        NodeId::Collation(q) => format!("collation:{q}"),
        NodeId::EventTrigger(n) => format!("event_trigger:{}", n.as_str()),
        NodeId::Aggregate(q, args) => format!(
            "aggregate:{}({})",
            q,
            args.types
                .iter()
                .map(pgevolve_core::ir::column_type::ColumnType::render_sql)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// Normalize a DOT string for comparison: strip blank lines and sort body
/// lines so insertion order is irrelevant.
fn normalize_dot(s: &str) -> String {
    let mut header = Vec::new();
    let mut body = Vec::new();
    let mut in_body = false;
    for line in s.lines() {
        let t = line.trim();
        if t.starts_with('}') {
            break;
        }
        if t.is_empty() {
            continue;
        }
        if t.starts_with("digraph") {
            header.push(line.to_string());
            in_body = true;
            continue;
        }
        if !in_body {
            continue;
        }
        body.push(line.to_string());
    }
    body.sort();
    let mut out = header.join("\n");
    out.push('\n');
    for e in &body {
        out.push_str(e);
        out.push('\n');
    }
    out.push_str("}\n");
    out
}
