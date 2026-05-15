//! `DepEdge` attaches metadata to existing graph edges.

use pgevolve_core::plan::edges::{build_create_graph, DepEdge, DepSource};
use pgevolve_core::parse::parse_directory;
use tempfile::tempdir;

#[test]
fn every_edge_in_v01_graph_has_dep_source_structural() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(
        dir.join("app/schema.sql"),
        "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app/users.sql"),
        "-- @pgevolve schema=app\n\
         CREATE TABLE app.users (id bigint PRIMARY KEY);\n",
    )
    .unwrap();

    let catalog = parse_directory(dir, &[]).unwrap();
    let graph = build_create_graph(&catalog);
    let edges: Vec<DepEdge> = graph.dep_edges().collect();
    assert!(!edges.is_empty(), "v0.1 catalog should produce at least one edge");
    for edge in &edges {
        assert!(
            matches!(edge.source, DepSource::Structural),
            "v0.1 edges should all be Structural, got {edge:?}",
        );
    }
}

#[test]
fn dep_source_ordering_is_structural_first() {
    // Ordering matters for deterministic tie-breaking; assert that
    // DepSource::Structural sorts before AstExtracted before AstDeclared.
    let mut sources = vec![
        DepSource::AstDeclared,
        DepSource::AstExtracted,
        DepSource::Structural,
    ];
    sources.sort();
    assert_eq!(
        sources,
        vec![DepSource::Structural, DepSource::AstExtracted, DepSource::AstDeclared]
    );
}

#[test]
fn remove_dep_edge_clears_source_map() {
    use pgevolve_core::identifier::Identifier;
    use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};
    use pgevolve_core::plan::graph::Graph;

    let mut g: Graph<NodeId> = Graph::new();
    let a = NodeId::Schema(Identifier::from_unquoted("a").unwrap());
    let b = NodeId::Schema(Identifier::from_unquoted("b").unwrap());
    g.add_node(a.clone());
    g.add_node(b.clone());

    g.add_dep_edge(a.clone(), b.clone(), DepSource::AstExtracted);
    g.remove_dep_edge(&a, &b);
    // Re-add via plain add_edge: should now report Structural,
    // proving the stale AstExtracted source was actually removed.
    g.add_edge(a.clone(), b.clone());

    let edges: Vec<DepEdge> = g.dep_edges().collect();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source, DepSource::Structural,
        "remove_dep_edge should have cleared the AstExtracted source");
}

#[test]
fn add_dep_edge_round_trips_source() {
    use pgevolve_core::identifier::Identifier;
    use pgevolve_core::plan::edges::{DepEdge, DepSource, NodeId};
    use pgevolve_core::plan::graph::Graph;

    let mut g: Graph<NodeId> = Graph::new();
    let a = NodeId::Schema(Identifier::from_unquoted("a").unwrap());
    let b = NodeId::Schema(Identifier::from_unquoted("b").unwrap());
    let c = NodeId::Schema(Identifier::from_unquoted("c").unwrap());
    g.add_node(a.clone());
    g.add_node(b.clone());
    g.add_node(c.clone());

    g.add_dep_edge(a.clone(), b.clone(), DepSource::Structural);
    g.add_dep_edge(a, c.clone(), DepSource::AstExtracted);

    let mut edges: Vec<DepEdge> = g.dep_edges().collect();
    edges.sort();
    assert_eq!(edges.len(), 2);
    let by_target: std::collections::BTreeMap<_, _> =
        edges.iter().map(|e| (e.to.clone(), e.source)).collect();
    assert_eq!(by_target[&b], DepSource::Structural);
    assert_eq!(by_target[&c], DepSource::AstExtracted);
}
