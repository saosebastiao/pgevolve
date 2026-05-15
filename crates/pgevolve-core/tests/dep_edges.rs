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
