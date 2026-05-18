//! Dependency edge extraction from a [`Catalog`].
//!
//! Edges follow the convention "A depends on B" — i.e., for each edge A → B,
//! B must be created before A. The four edge sources from spec §6.4:
//!
//! - schema ⟵ table ⟵ default-using-sequence
//! - table ⟵ index
//! - FK constraint ⟵ both endpoints (own table + referenced table)
//! - sequence ⟵ owning table (`OWNED BY`)
//!
//! Both `build_create_graph` (over the source catalog) and `build_drop_graph`
//! (over the target catalog) use the same edge logic — drop ordering is
//! produced by reversing the topological sort, not by reversing the edges.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::constraint::ConstraintKind;
use crate::ir::default_expr::DefaultExpr;
use crate::plan::graph::Graph;

/// Identifies any IR object uniquely within a [`Catalog`].
///
/// `Schema` carries an `Identifier` (schemas are not schema-qualified). All
/// other variants carry [`QualifiedName`]. `Constraint` is identified by
/// `(table, name)` because constraint names are scoped to their table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum NodeId {
    /// A schema (namespace).
    Schema(Identifier),
    /// A table.
    Table(QualifiedName),
    /// An index.
    Index(QualifiedName),
    /// A sequence.
    Sequence(QualifiedName),
    /// A constraint identified by its owning table and constraint name.
    Constraint {
        /// Owning table.
        table: QualifiedName,
        /// Constraint name (the `name` half of the constraint's qname).
        name: Identifier,
    },
}

/// Build the dependency graph for `catalog`, used for create/modify ordering.
///
/// Topologically sorting this graph yields **dependencies first**: schemas
/// before tables, tables before indexes, referenced tables before FKs, etc.
pub fn build_create_graph(catalog: &Catalog) -> Graph<NodeId> {
    let mut g = Graph::new();

    // Phase 1: every IR object gets a node, even if it has no edges.
    for s in &catalog.schemas {
        g.add_node(NodeId::Schema(s.name.clone()));
    }
    for t in &catalog.tables {
        g.add_node(NodeId::Table(t.qname.clone()));
    }
    for i in &catalog.indexes {
        g.add_node(NodeId::Index(i.qname.clone()));
    }
    for s in &catalog.sequences {
        g.add_node(NodeId::Sequence(s.qname.clone()));
    }
    for t in &catalog.tables {
        for c in &t.constraints {
            g.add_node(NodeId::Constraint {
                table: t.qname.clone(),
                name: c.qname.name.clone(),
            });
        }
    }

    // Phase 2: tables depend on their schema and on any sequence used as a
    // column default. We add the schema node implicitly via add_edge in case
    // the caller did not declare it (defensive: source-side parsing typically
    // does declare every referenced schema, but a hand-built Catalog might not).
    for t in &catalog.tables {
        g.add_edge(
            NodeId::Table(t.qname.clone()),
            NodeId::Schema(t.qname.schema.clone()),
        );
        for col in &t.columns {
            if let Some(DefaultExpr::Sequence(seq_qname)) = &col.default {
                g.add_edge(
                    NodeId::Table(t.qname.clone()),
                    NodeId::Sequence(seq_qname.clone()),
                );
            }
        }
    }

    // Phase 3: indexes depend on their parent (table or MV).
    // TODO(T7): Until T7 adds `NodeId::Mv` and Phase 1 of build_create_graph
    // registers MV nodes, an `IndexParent::Mv` index produces a dep edge to
    // `NodeId::Table(mv_qname)` that doesn't exist in the graph. Unlike a
    // proper "discard", `Graph::add_edge` actually inserts both endpoints as
    // nodes (see `add_edge_internal`), so the phantom `NodeId::Table(mv_qname)`
    // node will appear in topological output even though no `CREATE TABLE`
    // step exists for it — planner output will be silently wrong. T5 (catalog
    // assembly) must not ship without T7's NodeId::Mv variant + Phase-1
    // MV node registration, or planner output will contain spurious ghost
    // table nodes and miss ordering dependencies between MVs and their indexes.
    for i in &catalog.indexes {
        g.add_edge(
            NodeId::Index(i.qname.clone()),
            NodeId::Table(i.on.qname().clone()),
        );
    }

    // Phase 4: constraints depend on their owning table; FKs additionally
    // depend on the referenced table.
    //
    // For FKs we ALSO add a direct table → referenced_table edge. Inline FKs
    // are emitted as part of `CREATE TABLE`, so the owning table's create
    // statement requires the referenced table to exist first. Without this
    // edge, two-table FK cycles never produce a cycle in the graph and the
    // planner's FK-extraction post-pass would have nothing to detect.
    for t in &catalog.tables {
        for c in &t.constraints {
            let constraint_node = NodeId::Constraint {
                table: t.qname.clone(),
                name: c.qname.name.clone(),
            };
            g.add_edge(constraint_node.clone(), NodeId::Table(t.qname.clone()));
            if let ConstraintKind::ForeignKey(fk) = &c.kind {
                g.add_edge(constraint_node, NodeId::Table(fk.referenced_table.clone()));
                // Self-referential FKs don't induce a real table-level cycle
                // (table can be created first, FK satisfied at row time).
                if fk.referenced_table != t.qname {
                    g.add_edge(
                        NodeId::Table(t.qname.clone()),
                        NodeId::Table(fk.referenced_table.clone()),
                    );
                }
            }
        }
    }

    // Phase 5: an `OWNED BY` sequence depends on its owner table.
    for s in &catalog.sequences {
        if let Some(owner) = &s.owned_by {
            g.add_edge(
                NodeId::Sequence(s.qname.clone()),
                NodeId::Table(owner.table.clone()),
            );
        }
    }

    g
}

/// Build the dependency graph for drop-ordering. Same edges as the create
/// graph; the ordering is reversed at sort time.
pub fn build_drop_graph(catalog: &Catalog) -> Graph<NodeId> {
    build_create_graph(catalog)
}

/// Where a dependency edge came from.
///
/// `Structural` edges are derived from the IR shape itself (schema←table,
/// table←index, FK references, sequence ownership). They exist in v0.1.
///
/// `AstExtracted` edges are derived by walking the parsed AST of an object
/// body (view body, function body, expression-index predicate, etc.).
/// First produced in v0.2 view sub-spec.
///
/// `AstDeclared` edges come from explicit `-- @pgevolve dep:` directives
/// that close the PL/pgSQL-dynamic-SQL gap (Decision 11). First produced
/// in v0.2 function sub-spec.
///
/// Ordering: `Structural < AstExtracted < AstDeclared` — structural edges
/// are tie-broken first in the Kahn min-heap to preserve v0.1 ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DepSource {
    /// Derived from the IR shape; v0.1 default.
    Structural,
    /// Walked out of a parsed body AST.
    AstExtracted,
    /// Declared by a `-- @pgevolve dep:` directive in source SQL.
    AstDeclared,
}

/// An edge in the dependency graph, with provenance metadata.
///
/// Convention matches the existing graph: `from` depends on `to`, so `to`
/// must be created before `from`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DepEdge {
    /// Dependent node (the one that needs `to` to exist first).
    pub from: NodeId,
    /// Dependency target.
    pub to: NodeId,
    /// Provenance of this edge.
    pub source: DepSource,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::ir::schema::Schema;
    use crate::ir::sequence::{Sequence, SequenceOwner};
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn col_id_bigint() -> Column {
        Column {
            name: id("id"),
            ty: ColumnType::BigInt,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            comment: None,
        }
    }

    fn pk(name: &str, cols: &[&str]) -> Constraint {
        Constraint {
            qname: qn("app", name),
            kind: ConstraintKind::PrimaryKey {
                columns: cols.iter().map(|c| id(c)).collect(),
                include: vec![],
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn fk(name: &str, ref_table: QualifiedName) -> Constraint {
        Constraint {
            qname: qn("app", name),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: vec![id("ref_id")],
                referenced_table: ref_table,
                referenced_columns: vec![id("id")],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::NoAction,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn has_edge(g: &Graph<NodeId>, from: &NodeId, to: &NodeId) -> bool {
        g.dependencies_of(from).any(|n| n == to)
    }

    #[test]
    fn empty_catalog_yields_empty_graph() {
        let g = build_create_graph(&Catalog::empty());
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn every_object_appears_as_a_node() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col_id_bigint()],
            constraints: vec![pk("users_pkey", &["id"])],
            comment: None,
        });
        c.indexes.push(Index {
            qname: qn("app", "users_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        });
        c.sequences.push(Sequence {
            qname: qn("app", "seq1"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        });

        let g = build_create_graph(&c);
        // schema + table + index + sequence + constraint = 5
        assert_eq!(g.node_count(), 5);
    }

    #[test]
    fn table_depends_on_its_schema() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col_id_bigint()],
            constraints: vec![],
            comment: None,
        });
        let g = build_create_graph(&c);
        assert!(has_edge(
            &g,
            &NodeId::Table(qn("app", "users")),
            &NodeId::Schema(id("app")),
        ));
    }

    #[test]
    fn index_depends_on_its_table() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col_id_bigint()],
            constraints: vec![],
            comment: None,
        });
        c.indexes.push(Index {
            qname: qn("app", "users_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        });
        let g = build_create_graph(&c);
        assert!(has_edge(
            &g,
            &NodeId::Index(qn("app", "users_idx")),
            &NodeId::Table(qn("app", "users")),
        ));
    }

    #[test]
    fn fk_constraint_depends_on_both_endpoints() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![col_id_bigint()],
            constraints: vec![pk("orgs_pkey", &["id"])],
            comment: None,
        });
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
                col_id_bigint(),
                Column {
                    name: id("ref_id"),
                    ty: ColumnType::BigInt,
                    nullable: false,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    comment: None,
                },
            ],
            constraints: vec![fk("users_orgs_fk", qn("app", "orgs"))],
            comment: None,
        });
        let g = build_create_graph(&c);
        let fk_node = NodeId::Constraint {
            table: qn("app", "users"),
            name: id("users_orgs_fk"),
        };
        // Owning-table edge.
        assert!(has_edge(&g, &fk_node, &NodeId::Table(qn("app", "users"))));
        // Referenced-table edge.
        assert!(has_edge(&g, &fk_node, &NodeId::Table(qn("app", "orgs"))));
    }

    #[test]
    fn table_depends_on_default_sequence() {
        let mut c = Catalog::empty();
        c.sequences.push(Sequence {
            qname: qn("app", "id_seq"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        });
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
                name: id("id"),
                ty: ColumnType::BigInt,
                nullable: false,
                default: Some(DefaultExpr::Sequence(qn("app", "id_seq"))),
                identity: None,
                generated: None,
                collation: None,
                comment: None,
            }],
            constraints: vec![],
            comment: None,
        });
        let g = build_create_graph(&c);
        assert!(has_edge(
            &g,
            &NodeId::Table(qn("app", "users")),
            &NodeId::Sequence(qn("app", "id_seq")),
        ));
    }

    #[test]
    fn owned_sequence_depends_on_owner_table() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col_id_bigint()],
            constraints: vec![],
            comment: None,
        });
        c.sequences.push(Sequence {
            qname: qn("app", "users_id_seq"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: Some(SequenceOwner {
                table: qn("app", "users"),
                column: id("id"),
            }),
            comment: None,
        });
        let g = build_create_graph(&c);
        assert!(has_edge(
            &g,
            &NodeId::Sequence(qn("app", "users_id_seq")),
            &NodeId::Table(qn("app", "users")),
        ));
    }

    #[test]
    fn non_fk_constraint_depends_only_on_its_table() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col_id_bigint()],
            constraints: vec![pk("users_pkey", &["id"])],
            comment: None,
        });
        let g = build_create_graph(&c);
        let pk_node = NodeId::Constraint {
            table: qn("app", "users"),
            name: id("users_pkey"),
        };
        let deps: Vec<&NodeId> = g.dependencies_of(&pk_node).collect();
        assert_eq!(deps, vec![&NodeId::Table(qn("app", "users"))]);
    }

    #[test]
    fn drop_graph_matches_create_graph() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![col_id_bigint()],
            constraints: vec![pk("users_pkey", &["id"])],
            comment: None,
        });
        // Same edges; equality is structural via topological output.
        let cg = build_create_graph(&c);
        let dg = build_drop_graph(&c);
        assert_eq!(cg.topological_sort(), dg.topological_sort());
    }

    #[test]
    fn fk_cycle_produces_table_level_cycle() {
        // Two tables with FKs to each other; each FK induces an inline-create
        // edge table → referenced_table, so the table subgraph cycles.
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "a"),
            columns: vec![
                col_id_bigint(),
                Column {
                    name: id("ref_id"),
                    ty: ColumnType::BigInt,
                    nullable: false,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    comment: None,
                },
            ],
            constraints: vec![pk("a_pk", &["id"]), fk("a_to_b", qn("app", "b"))],
            comment: None,
        });
        c.tables.push(Table {
            qname: qn("app", "b"),
            columns: vec![
                col_id_bigint(),
                Column {
                    name: id("ref_id"),
                    ty: ColumnType::BigInt,
                    nullable: false,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    comment: None,
                },
            ],
            constraints: vec![pk("b_pk", &["id"]), fk("b_to_a", qn("app", "a"))],
            comment: None,
        });
        let g = build_create_graph(&c);
        let err = g.topological_sort().unwrap_err();
        assert!(err.nodes.contains(&NodeId::Table(qn("app", "a"))));
        assert!(err.nodes.contains(&NodeId::Table(qn("app", "b"))));
    }

    #[test]
    fn self_referential_fk_does_not_cycle() {
        // A self-referential FK doesn't force the table to depend on itself —
        // the rows are inserted after the table exists.
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("app", "tree"),
            columns: vec![
                col_id_bigint(),
                Column {
                    name: id("ref_id"),
                    ty: ColumnType::BigInt,
                    nullable: true,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    comment: None,
                },
            ],
            constraints: vec![
                pk("tree_pk", &["id"]),
                fk("tree_parent_fk", qn("app", "tree")),
            ],
            comment: None,
        });
        let g = build_create_graph(&c);
        assert!(g.topological_sort().is_ok());
    }
}
