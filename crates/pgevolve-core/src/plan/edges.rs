//! Stub — replaced in task 5.2.
#![allow(missing_docs)]

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::plan::graph::Graph;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeId {
    Schema(Identifier),
    Table(QualifiedName),
    Index(QualifiedName),
    Sequence(QualifiedName),
    Constraint { table: QualifiedName, name: Identifier },
}

pub fn build_create_graph(_catalog: &Catalog) -> Graph<NodeId> {
    Graph::new()
}

pub fn build_drop_graph(catalog: &Catalog) -> Graph<NodeId> {
    build_create_graph(catalog)
}
