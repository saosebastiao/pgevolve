# Phase 5 — Dependency analyzer & planner core

**Goal:** Turn an unordered `ChangeSet` into an ordered, dependency-correct `OrderedChangeSet`, ready for the rewrite pass and step grouping. Handle FK forward-reference cycles by post-pass extraction.

**Spec coverage:** §6.4.

**Depends on:** Phase 4 (Differ).

**Exit criteria:**

- `pgevolve_core::plan::order(target: &Catalog, source: &Catalog, changes: ChangeSet) -> Result<OrderedChangeSet, PlanError>`.
- Three-phase order: creates+adds → modifies → drops. Each phase is independently topologically sorted using the appropriate graph (source-side for creates/modifies, target-side for drops).
- Cycles in the create graph extract as a separate post-pass FK-add step list.
- Stable, deterministic order: identical input → byte-identical output.
- > 25 unit tests covering every dependency edge type and the cycle case.

---

## File structure

```
crates/pgevolve-core/src/
└── plan/
    ├── mod.rs                # public re-exports + order() entry point
    ├── error.rs              # PlanError
    ├── graph.rs              # Graph type, topo sort, cycle detection
    ├── edges.rs              # edge extraction from IR
    ├── ordered.rs            # OrderedChangeSet type + phase model
    └── ordering.rs           # the three-phase ordering logic
```

---

### Task 5.1: `Graph` and topological sort

**File:** `crates/pgevolve-core/src/plan/graph.rs`

```rust
pub struct Graph<N: Hash + Eq + Clone> {
    nodes: HashSet<N>,
    edges: HashMap<N, HashSet<N>>,  // edges[A] = set of B such that A depends on B
}

impl<N: Hash + Eq + Clone + Ord> Graph<N> {
    pub fn new() -> Self;
    pub fn add_node(&mut self, n: N);
    pub fn add_edge(&mut self, from: N, to: N);
    pub fn topological_sort(&self) -> Result<Vec<N>, Cycle<N>>;
    pub fn reverse_topological_sort(&self) -> Result<Vec<N>, Cycle<N>>;
}

pub struct Cycle<N> { pub nodes: Vec<N> }
```

Implement Kahn's algorithm. Tie-break by `Ord` on the node type so order is deterministic.

Tests: linear chain, diamond, cycle of 2, cycle of 3, disconnected components.

Commit: `feat(core): Graph type with deterministic topological sort + cycle detection`

---

### Task 5.2: Node identifiers and edge extraction

**File:** `crates/pgevolve-core/src/plan/edges.rs`

Define a `NodeId` enum identifying any IR object uniquely:

```rust
pub enum NodeId {
    Schema(QualifiedName),     // schema is a QualifiedName with schema=name=schema_name for simplicity
    Table(QualifiedName),
    Index(QualifiedName),
    Sequence(QualifiedName),
    Constraint { table: QualifiedName, name: Identifier },
}
```

Edge extractors over a `Catalog`:

```rust
pub fn build_create_graph(catalog: &Catalog) -> Graph<NodeId> {
    let mut g = Graph::new();

    // Add every node.
    for s in &catalog.schemas         { g.add_node(NodeId::Schema(s.qname())); }
    for t in &catalog.tables          { g.add_node(NodeId::Table(t.qname.clone())); }
    for i in &catalog.indexes         { g.add_node(NodeId::Index(i.qname.clone())); }
    for s in &catalog.sequences       { g.add_node(NodeId::Sequence(s.qname.clone())); }
    for t in &catalog.tables {
        for c in &t.constraints {
            g.add_node(NodeId::Constraint { table: t.qname.clone(), name: c.qname.name.clone() });
        }
    }

    // Edges:
    for t in &catalog.tables {
        // Table depends on its schema.
        g.add_edge(NodeId::Table(t.qname.clone()),
                   NodeId::Schema(schema_qname(&t.qname.schema)));

        // Table depends on any sequence used in column defaults.
        for col in &t.columns {
            if let Some(DefaultExpr::Sequence(seq_qname)) = &col.default {
                g.add_edge(
                    NodeId::Table(t.qname.clone()),
                    NodeId::Sequence(seq_qname.clone()),
                );
            }
        }
    }

    for i in &catalog.indexes {
        g.add_edge(NodeId::Index(i.qname.clone()),
                   NodeId::Table(i.table.clone()));
    }

    for t in &catalog.tables {
        for c in &t.constraints {
            let constraint_node = NodeId::Constraint {
                table: t.qname.clone(), name: c.qname.name.clone()
            };
            // Constraint depends on its owning table.
            g.add_edge(constraint_node.clone(), NodeId::Table(t.qname.clone()));
            // FK constraints depend on the referenced table.
            if let ConstraintKind::ForeignKey(fk) = &c.kind {
                g.add_edge(constraint_node.clone(),
                           NodeId::Table(fk.referenced_table.clone()));
            }
        }
    }

    for s in &catalog.sequences {
        if let Some(owner) = &s.owned_by {
            g.add_edge(NodeId::Sequence(s.qname.clone()),
                       NodeId::Table(owner.table.clone()));
        }
    }

    g
}

pub fn build_drop_graph(catalog: &Catalog) -> Graph<NodeId> {
    // Same edges as create graph, but used in reverse for drop ordering.
    build_create_graph(catalog)
}
```

Tests: build a graph for a known catalog, assert specific edges exist.

Commit: `feat(core): dependency edge extraction from Catalog IR`

---

### Task 5.3: `OrderedChangeSet` type

**File:** `crates/pgevolve-core/src/plan/ordered.rs`

```rust
pub struct OrderedChangeSet {
    pub creates_and_adds: Vec<ChangeEntry>,    // dependency order
    pub modifies:         Vec<ChangeEntry>,    // dependency order
    pub drops:            Vec<ChangeEntry>,    // reverse dependency order
    pub deferred_fks:     Vec<DeferredFkAdd>,  // post-pass FK adds for cycle handling
}

pub struct DeferredFkAdd {
    pub table: QualifiedName,
    pub constraint: Constraint,
}

impl OrderedChangeSet {
    pub fn is_empty(&self) -> bool {
        self.creates_and_adds.is_empty()
            && self.modifies.is_empty()
            && self.drops.is_empty()
            && self.deferred_fks.is_empty()
    }
    pub fn len(&self) -> usize {
        self.creates_and_adds.len() + self.modifies.len() + self.drops.len() + self.deferred_fks.len()
    }
}
```

Commit: `feat(core): OrderedChangeSet type`

---

### Task 5.4: Three-phase ordering

**File:** `crates/pgevolve-core/src/plan/ordering.rs`

Algorithm:

```rust
pub fn order(
    target: &Catalog,
    source: &Catalog,
    changes: ChangeSet,
) -> Result<OrderedChangeSet, PlanError> {

    let create_graph = build_create_graph(source);
    let drop_graph   = build_drop_graph(target);

    // 1. Partition changes into three buckets.
    let mut creates: Vec<ChangeEntry> = Vec::new();
    let mut modifies: Vec<ChangeEntry> = Vec::new();
    let mut drops: Vec<ChangeEntry> = Vec::new();
    for entry in changes.entries {
        match &entry.change {
            Change::CreateSchema(_)
            | Change::CreateTable(_)
            | Change::CreateIndex(_)
            | Change::CreateSequence(_) => creates.push(entry),

            Change::DropSchema(_)
            | Change::DropTable { .. }
            | Change::DropIndex(_)
            | Change::DropSequence(_) => drops.push(entry),

            Change::AlterTable { .. }
            | Change::AlterSchema { .. }
            | Change::AlterSequence { .. }
            | Change::ReplaceIndex { .. } => modifies.push(entry),
        }
    }

    // 2. Detect cycles in the create graph; extract FKs to break them.
    let create_topo_attempt = create_graph.topological_sort();
    let (sorted_creates, deferred_fks) = match create_topo_attempt {
        Ok(order) => (order, Vec::new()),
        Err(_cycle) => extract_fk_cycles_and_resort(source, &create_graph)?,
    };

    // 3. Sort each bucket by graph order.
    let creates = sort_changes_by_order(&creates, &sorted_creates);

    let modify_topo = create_graph.topological_sort()
        .map_err(|c| PlanError::UnexpectedCycleAfterFkExtraction(c.into()))?;
    let modifies = sort_changes_by_order(&modifies, &modify_topo);

    let drop_topo = drop_graph.reverse_topological_sort()
        .map_err(|c| PlanError::UnexpectedDropCycle(c.into()))?;
    let drops = sort_changes_by_order(&drops, &drop_topo);

    Ok(OrderedChangeSet {
        creates_and_adds: creates,
        modifies,
        drops,
        deferred_fks,
    })
}
```

`extract_fk_cycles_and_resort`:

1. Run Tarjan's SCC algorithm to find strongly-connected components > 1 node.
2. For each non-trivial SCC: find FK constraints whose referenced-table node is also in the SCC. Remove those FK constraints from the graph (add them to `deferred_fks`).
3. Re-run topological sort. If still cyclic → `PlanError::UnbreakableCycle`.

`sort_changes_by_order`: matches each `ChangeEntry` to its `NodeId` via the `Change` body, then sorts by the index of that `NodeId` in `sorted_creates`.

Tests:
- Linear schema (schema → table → index) sorts in dependency order.
- FK between independent tables.
- Two-table FK cycle → both tables created, FK extracted to `deferred_fks`.
- Drop order: index dropped before table, FK constraint dropped before referenced table's PK.

Commit: `feat(core): three-phase ordering with FK cycle extraction`

---

### Task 5.5: `PlanError`

**File:** `crates/pgevolve-core/src/plan/error.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("unbreakable dependency cycle: {0:?}")]
    UnbreakableCycle(Vec<String>),
    #[error("unexpected cycle in modify graph after FK extraction: {0:?}")]
    UnexpectedCycleAfterFkExtraction(Vec<String>),
    #[error("unexpected cycle in drop graph: {0:?}")]
    UnexpectedDropCycle(Vec<String>),
    #[error("internal error: {0}")]
    Internal(String),
}
```

Add `Plan(PlanError)` to the top-level `error::Error`.

Commit: `feat(core): PlanError type`

---

### Task 5.6: Phase 5 self-review

- Walk spec §6.4 line by line — all four edge sources represented.
- Three-phase ordering verified against multi-object fixture.
- `cargo test -p pgevolve-core` passes; clippy clean.

Phase 5 complete.
