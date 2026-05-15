//! Directed graph and topological sort over arbitrary node identifiers.
//!
//! Edge direction convention: `A -> B` means **`A` depends on `B`**.
//! Topological order therefore visits dependencies before their dependents
//! (i.e., leaves first), which is what `creates_and_adds` ordering requires.
//!
//! Sort is deterministic: when multiple nodes are simultaneously eligible,
//! the smallest one (by `Ord`) wins. Identical input ⇒ byte-identical output.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::hash::Hash;

/// A directed graph over nodes of type `N`.
#[derive(Debug, Clone)]
pub struct Graph<N> {
    nodes: BTreeSet<N>,
    /// `edges[A]` = the set of `B` such that A depends on B.
    edges: BTreeMap<N, BTreeSet<N>>,
}

/// A cycle reported by [`Graph::topological_sort`] / [`Graph::reverse_topological_sort`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle<N> {
    /// Nodes that participate in at least one cycle, in deterministic order.
    pub nodes: Vec<N>,
}

impl<N> Default for Graph<N>
where
    N: Hash + Eq + Clone + Ord,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<N> Graph<N>
where
    N: Hash + Eq + Clone + Ord,
{
    /// Construct an empty graph.
    pub const fn new() -> Self {
        Self {
            nodes: BTreeSet::new(),
            edges: BTreeMap::new(),
        }
    }

    /// Add a node. No-op if already present.
    pub fn add_node(&mut self, n: N) {
        self.nodes.insert(n);
    }

    /// Add an edge `from -> to`, meaning `from` depends on `to`.
    /// Both endpoints are added as nodes if not already present.
    pub fn add_edge(&mut self, from: N, to: N) {
        self.nodes.insert(from.clone());
        self.nodes.insert(to.clone());
        self.edges.entry(from).or_default().insert(to);
    }

    /// Remove an edge `from -> to`. No-op if absent.
    pub fn remove_edge(&mut self, from: &N, to: &N) {
        if let Some(set) = self.edges.get_mut(from) {
            set.remove(to);
            if set.is_empty() {
                self.edges.remove(from);
            }
        }
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Iterate over all nodes in `Ord` order.
    pub fn nodes(&self) -> impl Iterator<Item = &N> {
        self.nodes.iter()
    }

    /// Iterate the dependencies of `n` (the set `edges[n]`).
    pub fn dependencies_of<'a>(&'a self, n: &N) -> impl Iterator<Item = &'a N> + 'a + use<'a, N> {
        self.edges.get(n).into_iter().flat_map(BTreeSet::iter)
    }

    /// Topological sort using Kahn's algorithm.
    ///
    /// Returns nodes with no remaining dependencies first. Ties are broken by
    /// the smallest node per `Ord`, so the result is deterministic.
    pub fn topological_sort(&self) -> Result<Vec<N>, Cycle<N>> {
        // Build reverse adjacency: dependents[B] = nodes that depend on B.
        // We compute in-degree based on outgoing dependency edges.
        let mut in_degree: BTreeMap<N, usize> =
            self.nodes.iter().map(|n| (n.clone(), 0_usize)).collect();
        let mut dependents: BTreeMap<N, Vec<N>> = BTreeMap::new();

        for (from, deps) in &self.edges {
            // `from` depends on each `to`, so `from` has out-edges to each `to`.
            // In a "dependencies first" topo sort, we treat the *dependency* edge
            // as `from -> to` and want `to` emitted before `from`.
            // Kahn's algorithm: in-degree of `from` = number of unresolved deps.
            *in_degree.entry(from.clone()).or_insert(0) += deps.len();
            for to in deps {
                dependents.entry(to.clone()).or_default().push(from.clone());
            }
        }

        // Min-heap by Ord (BinaryHeap is max-heap, so wrap with Reverse).
        let mut ready: BinaryHeap<std::cmp::Reverse<N>> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(n, _)| std::cmp::Reverse(n.clone()))
            .collect();

        let mut out = Vec::with_capacity(self.nodes.len());

        while let Some(std::cmp::Reverse(n)) = ready.pop() {
            out.push(n.clone());
            if let Some(parents) = dependents.get(&n) {
                for p in parents {
                    if let Some(d) = in_degree.get_mut(p) {
                        *d -= 1;
                        if *d == 0 {
                            ready.push(std::cmp::Reverse(p.clone()));
                        }
                    }
                }
            }
        }

        if out.len() == self.nodes.len() {
            Ok(out)
        } else {
            // Remaining nodes (in-degree > 0) participate in at least one cycle.
            let cycle_nodes: Vec<N> = in_degree
                .into_iter()
                .filter_map(|(n, d)| (d > 0).then_some(n))
                .collect();
            Err(Cycle { nodes: cycle_nodes })
        }
    }

    /// Reverse topological sort: dependents first, dependencies last.
    /// Used for drop ordering (drop the index before the table it indexes).
    pub fn reverse_topological_sort(&self) -> Result<Vec<N>, Cycle<N>> {
        let mut v = self.topological_sort()?;
        v.reverse();
        Ok(v)
    }

    /// Iterate all edges as `(from, to)` pairs in deterministic order.
    ///
    /// Pairs are yielded in `(from Ord, to Ord)` order because the underlying
    /// storage is a `BTreeMap<N, BTreeSet<N>>`.
    pub fn edges(&self) -> impl Iterator<Item = (&N, &N)> {
        self.edges
            .iter()
            .flat_map(|(from, targets)| targets.iter().map(move |to| (from, to)))
    }
}

impl crate::plan::graph::Graph<crate::plan::edges::NodeId> {
    /// Iterate edges as [`DepEdge`](crate::plan::edges::DepEdge) records.
    ///
    /// Every v0.1 edge is tagged [`DepSource::Structural`](crate::plan::edges::DepSource::Structural).
    /// v0.2 sub-specs will populate `AstExtracted` and `AstDeclared` via
    /// `add_dep_edge` (added in Task 2).
    pub fn dep_edges(&self) -> impl Iterator<Item = crate::plan::edges::DepEdge> + '_ {
        self.edges().map(|(from, to)| crate::plan::edges::DepEdge {
            from: from.clone(),
            to: to.clone(),
            source: crate::plan::edges::DepSource::Structural,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_sorts_to_empty() {
        let g: Graph<i32> = Graph::new();
        assert_eq!(g.topological_sort().unwrap(), Vec::<i32>::new());
    }

    #[test]
    fn single_node_sorts_to_self() {
        let mut g: Graph<i32> = Graph::new();
        g.add_node(7);
        assert_eq!(g.topological_sort().unwrap(), vec![7]);
    }

    #[test]
    fn linear_chain_sorts_in_dependency_order() {
        // a -> b -> c  (a depends on b depends on c)
        // Output: c, b, a
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "b");
        g.add_edge("b", "c");
        assert_eq!(g.topological_sort().unwrap(), vec!["c", "b", "a"]);
    }

    #[test]
    fn diamond_sorts_with_deterministic_tie_break() {
        // a -> b, a -> c, b -> d, c -> d
        // d emitted first, then b/c (tie → smaller "b" first), then a
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "b");
        g.add_edge("a", "c");
        g.add_edge("b", "d");
        g.add_edge("c", "d");
        assert_eq!(g.topological_sort().unwrap(), vec!["d", "b", "c", "a"]);
    }

    #[test]
    fn disconnected_components_sort_deterministically() {
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "b");
        g.add_edge("x", "y");
        // Leaves first (b, y); after popping b, a unblocks. Among {a, y} the
        // smaller is a, then y unblocks no one, then x.
        assert_eq!(g.topological_sort().unwrap(), vec!["b", "a", "y", "x"]);
    }

    #[test]
    fn isolated_nodes_appear_in_order() {
        let mut g: Graph<i32> = Graph::new();
        g.add_node(3);
        g.add_node(1);
        g.add_node(2);
        assert_eq!(g.topological_sort().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn cycle_of_two_detected() {
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "b");
        g.add_edge("b", "a");
        let err = g.topological_sort().unwrap_err();
        assert_eq!(err.nodes, vec!["a", "b"]);
    }

    #[test]
    fn cycle_of_three_detected() {
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "b");
        g.add_edge("b", "c");
        g.add_edge("c", "a");
        let err = g.topological_sort().unwrap_err();
        assert_eq!(err.nodes, vec!["a", "b", "c"]);
    }

    #[test]
    fn cycle_does_not_block_acyclic_part() {
        // Acyclic part: x -> y. Cycle: a <-> b. Both are reported correctly:
        // sort errors with cycle nodes; we only check that.
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("x", "y");
        g.add_edge("a", "b");
        g.add_edge("b", "a");
        let err = g.topological_sort().unwrap_err();
        assert_eq!(err.nodes, vec!["a", "b"]);
    }

    #[test]
    fn reverse_sort_inverts_order() {
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "b");
        g.add_edge("b", "c");
        assert_eq!(g.reverse_topological_sort().unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn remove_edge_breaks_cycle() {
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "b");
        g.add_edge("b", "a");
        assert!(g.topological_sort().is_err());
        g.remove_edge(&"b", &"a");
        assert_eq!(g.topological_sort().unwrap(), vec!["b", "a"]);
    }

    #[test]
    fn deterministic_under_insertion_order_changes() {
        let mut g1: Graph<&'static str> = Graph::new();
        g1.add_edge("a", "b");
        g1.add_edge("a", "c");
        g1.add_edge("b", "d");
        g1.add_edge("c", "d");

        let mut g2: Graph<&'static str> = Graph::new();
        g2.add_edge("c", "d");
        g2.add_edge("a", "c");
        g2.add_edge("b", "d");
        g2.add_edge("a", "b");

        assert_eq!(g1.topological_sort(), g2.topological_sort());
    }

    #[test]
    fn dependencies_of_returns_in_ord_order() {
        let mut g: Graph<&'static str> = Graph::new();
        g.add_edge("a", "z");
        g.add_edge("a", "b");
        g.add_edge("a", "m");
        let deps: Vec<&&str> = g.dependencies_of(&"a").collect();
        assert_eq!(deps, vec![&"b", &"m", &"z"]);
    }
}
