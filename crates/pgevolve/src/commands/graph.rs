//! `pgevolve graph` — render the dep graph in DOT or Mermaid format.
//!
//! Reads the source IR and produces either DOT or Mermaid output.
//!
//! For v0.1 every edge is [`DepSource::Structural`] (solid arrow in DOT,
//! plain `-->` in Mermaid). The v0.2 view/function sub-specs produce
//! `AstExtracted` (dashed) and `AstDeclared` (dotted) edges that this
//! renderer already distinguishes.
//!
//! # Note on `render_dot` visibility
//!
//! `render_dot` is `pub` because the test-strategy-v2 L8 dep-graph golden
//! layer (conformance crate) needs the same renderer. Since
//! `pgevolve-conformance` cannot depend on the binary crate, it will
//! reimplement the renderer — but the `pub` here documents intent and allows
//! future extraction into a shared library crate.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;
use pgevolve_core::parse::parse_directory;
use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};

use crate::cli::GraphFormat;
use crate::config::PgevolveConfig;

/// Entry point for `pgevolve graph`.
pub fn run(
    config: &PgevolveConfig,
    format: GraphFormat,
    out: Option<PathBuf>,
    plan: Option<&PathBuf>,
) -> Result<i32> {
    if plan.is_some() {
        anyhow::bail!("--plan rendering not yet implemented (v0.2 sub-spec landing)");
    }

    let catalog = parse_directory(&config.project.schema_dir, &[])?;
    let graph = pgevolve_core::plan::edges::build_create_graph(&catalog);
    let edges: Vec<DepEdge> = graph.dep_edges().collect();

    let rendered = match format {
        GraphFormat::Dot => render_dot(&edges),
        GraphFormat::Mermaid => render_mermaid(&edges),
    };

    if let Some(path) = out {
        std::fs::write(&path, &rendered)?;
        eprintln!("wrote {} bytes to {}", rendered.len(), path.display());
    } else {
        print!("{rendered}");
    }
    Ok(0)
}

/// Render edges as DOT (the graphviz format).
///
/// Edge styles:
/// - `Structural` → `solid`
/// - `AstExtracted` → `dashed`
/// - `AstDeclared` → `dotted`
///
/// Output is deterministic: nodes are listed in sorted order, then edges in
/// sorted order. Golden tests on this output are stable.
///
/// Note: `pgevolve-conformance` cannot depend on the binary crate, so the L8
/// dep-graph golden layer will reimplement this function. This `pub` is
/// intentional as documentation of the canonical rendering contract.
pub fn render_dot(edges: &[DepEdge]) -> String {
    let mut out = String::from(
        "digraph pgevolve_deps {\n  rankdir=LR;\n  node [shape=box, fontname=Helvetica];\n",
    );

    // Collect all node labels in sorted order.
    let mut nodes: BTreeSet<String> = BTreeSet::default();
    for e in edges {
        nodes.insert(node_label(&e.from));
        nodes.insert(node_label(&e.to));
    }
    for n in &nodes {
        let _ = writeln!(out, "  \"{n}\";");
    }

    // Sort edges deterministically.
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

fn render_mermaid(edges: &[DepEdge]) -> String {
    let mut out = String::from("graph LR\n");
    let mut sorted = edges.to_vec();
    sorted.sort();
    for e in sorted {
        let arrow = match e.source {
            DepSource::Structural => "-->",
            DepSource::AstExtracted => "-.->",
            DepSource::AstDeclared => "==>",
        };
        let _ = writeln!(
            out,
            "  {} {arrow} {}",
            mermaid_safe(&node_label(&e.from)),
            mermaid_safe(&node_label(&e.to))
        );
    }
    out
}

fn node_label(n: &NodeId) -> String {
    match n {
        NodeId::Schema(s) => format!("schema:{}", s.as_str()),
        NodeId::Table(q) => format!("table:{q}"),
        NodeId::Index(q) => format!("index:{q}"),
        NodeId::Sequence(q) => format!("sequence:{q}"),
        NodeId::Constraint { table, name } => format!("constraint:{table}.{}", name.as_str()),
        NodeId::View(q) => format!("view:{q}"),
        NodeId::Mv(q) => format!("mv:{q}"),
        NodeId::Type(q) => format!("type:{q}"),
    }
}

fn mermaid_safe(label: &str) -> String {
    // Mermaid node IDs can't contain `.` or `:` freely; escape into bracketed form.
    let id = label.replace(['.', ':'], "_");
    format!("{id}[\"{label}\"]")
}
