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
use crate::ir::collation::BUILTIN_COLLATIONS;
use crate::ir::column_type::ColumnType;
use crate::ir::constraint::ConstraintKind;
use crate::ir::default_expr::DefaultExpr;
use crate::ir::function::ReturnType;
use crate::ir::user_type::UserTypeKind;
use crate::plan::graph::Graph;

pub use crate::ir::function::NormalizedArgTypes;

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
    /// A view (`CREATE VIEW`).
    View(QualifiedName),
    /// A materialized view (`CREATE MATERIALIZED VIEW`).
    Mv(QualifiedName),
    /// A user-defined type (enum, domain, or composite).
    Type(QualifiedName),
    /// A user-defined function — disambiguated by argument types (Decision 7).
    Function(QualifiedName, NormalizedArgTypes),
    /// A user-defined procedure — identified by qname only (Decision 2).
    Procedure(QualifiedName),
    /// An installed extension.
    Extension(Identifier),
    /// A trigger (qname unique within schema).
    Trigger(QualifiedName),
    /// A publication (not schema-qualified — publications are a per-database
    /// global namespace).
    Publication(Identifier),
    /// A subscription (not schema-qualified — subscriptions are a per-database
    /// global namespace, like publications).
    Subscription(Identifier),
    /// A statistics object (`CREATE STATISTICS schema.name`).
    Statistic(QualifiedName),
    /// A user-defined collation (`CREATE COLLATION schema.name`).
    ///
    /// The variant lands in v0.3.8 so `ordering::change_node` can route
    /// collation changes correctly; the actual graph *edges* (column /
    /// domain / range / composite-attribute → collation) are added in a
    /// follow-up stage.
    Collation(QualifiedName),
}

/// Returns `true` iff `qname` refers to a managed collation in `catalog` —
/// i.e., a collation we should emit a dependency edge for. Built-in
/// `pg_catalog` collations (`C`, `POSIX`, `und-x-icu`, …) are skipped.
fn should_add_collation_edge(qname: &QualifiedName, catalog: &Catalog) -> bool {
    if qname.schema.as_str() == "pg_catalog" {
        return false;
    }
    let name = qname.name.as_str();
    if BUILTIN_COLLATIONS.contains(&name) {
        return false;
    }
    catalog.collations.iter().any(|c| &c.qname == qname)
}

/// Build the dependency graph for `catalog`, used for create/modify ordering.
///
/// Topologically sorting this graph yields **dependencies first**: schemas
/// before tables, tables before indexes, referenced tables before FKs, etc.
#[allow(clippy::too_many_lines)] // two-phase walk of every catalog object family adding nodes + edges; one place per object.
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
    // Register view and MV nodes so they participate in topological ordering
    // and body-dependency edges are rooted correctly.
    for v in &catalog.views {
        g.add_node(NodeId::View(v.qname.clone()));
    }
    for mv in &catalog.materialized_views {
        g.add_node(NodeId::Mv(mv.qname.clone()));
    }
    // Register user-defined type nodes.
    for t in &catalog.types {
        g.add_node(NodeId::Type(t.qname.clone()));
    }
    // Register function and procedure nodes.
    for f in &catalog.functions {
        g.add_node(NodeId::Function(
            f.qname.clone(),
            f.arg_types_normalized.clone(),
        ));
    }
    for p in &catalog.procedures {
        g.add_node(NodeId::Procedure(p.qname.clone()));
    }
    // Register triggers; trigger depends on its target relation and function.
    for t in &catalog.triggers {
        g.add_node(NodeId::Trigger(t.qname.clone()));
        let target_node = if catalog.tables.iter().any(|x| x.qname == t.table) {
            NodeId::Table(t.table.clone())
        } else if catalog.views.iter().any(|x| x.qname == t.table) {
            NodeId::View(t.table.clone())
        } else if catalog
            .materialized_views
            .iter()
            .any(|x| x.qname == t.table)
        {
            NodeId::Mv(t.table.clone())
        } else {
            // Unresolved target — the lint rule trigger-references-unmanaged-table
            // catches this in T9. Skip the edge so the graph builder doesn't
            // panic on a missing target node.
            continue;
        };
        g.add_edge(NodeId::Trigger(t.qname.clone()), target_node);
        if let Some(func) = catalog
            .functions
            .iter()
            .find(|f| f.qname == t.function_qname)
        {
            g.add_edge(
                NodeId::Trigger(t.qname.clone()),
                NodeId::Function(t.function_qname.clone(), func.arg_types_normalized.clone()),
            );
        }
    }
    // Register extensions; an extension with WITH SCHEMA s depends on the schema.
    for e in &catalog.extensions {
        g.add_node(NodeId::Extension(e.name.clone()));
        if let Some(schema) = &e.schema {
            g.add_edge(
                NodeId::Extension(e.name.clone()),
                NodeId::Schema(schema.clone()),
            );
        }
    }
    // Register publications. For Selective publications, add edges from each
    // referenced table and schema to the publication node so publications are
    // ordered after their dependencies. AllTables publications have no explicit
    // edges; they are ordered by tier rule.
    for p in &catalog.publications {
        let pub_node = NodeId::Publication(p.name.clone());
        g.add_node(pub_node.clone());
        if let crate::ir::publication::PublicationScope::Selective { schemas, tables } = &p.scope {
            for t in tables {
                g.add_edge(pub_node.clone(), NodeId::Table(t.qname.clone()));
            }
            for s in schemas {
                g.add_edge(pub_node.clone(), NodeId::Schema(s.clone()));
            }
        }
    }
    // Register subscriptions. Subscriptions cross-reference publications in
    // a *different* cluster — no local dep edges anchor them. They are
    // registered as isolated nodes; the tier rule in ordering.rs schedules
    // them create-last, drop-first via sort_key.
    for s in &catalog.subscriptions {
        g.add_node(NodeId::Subscription(s.name.clone()));
    }
    // Register statistics; each depends on its target table (must be created
    // after the table exists and dropped before the table is dropped).
    for s in &catalog.statistics {
        let stat_node = NodeId::Statistic(s.qname.clone());
        g.add_node(stat_node.clone());
        g.add_edge(stat_node, NodeId::Table(s.target.clone()));
    }
    // Register collations. Edges (table/type → collation) are added below in
    // the column / domain / range / composite-attribute loops.
    for c in &catalog.collations {
        g.add_node(NodeId::Collation(c.qname.clone()));
    }

    // Phase 1b.0: type → schema edges. Every user-defined type lives inside
    // a schema and must be created after CREATE SCHEMA emits.
    for t in &catalog.types {
        g.add_edge(
            NodeId::Type(t.qname.clone()),
            NodeId::Schema(t.qname.schema.clone()),
        );
    }
    // Phase 1b.0 (routines): every function/procedure depends on its schema.
    for f in &catalog.functions {
        let node = NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone());
        g.add_edge(node, NodeId::Schema(f.qname.schema.clone()));
    }
    for p in &catalog.procedures {
        g.add_edge(
            NodeId::Procedure(p.qname.clone()),
            NodeId::Schema(p.qname.schema.clone()),
        );
    }

    // Phase 1b: type → type edges from composite attributes and domain bases.
    // These edges ensure composites/domains that reference other user-defined
    // types are created after those types. Range types additionally depend on
    // their subtype (when managed) and on their canonical / subtype_diff
    // functions (when managed).
    for ut in &catalog.types {
        match &ut.kind {
            UserTypeKind::Composite { attributes } => {
                for attr in attributes {
                    if let ColumnType::UserDefined(dep_qname) = &attr.ty {
                        g.add_edge(
                            NodeId::Type(ut.qname.clone()),
                            NodeId::Type(dep_qname.clone()),
                        );
                    }
                    if let Some(coll_qname) = &attr.collation
                        && should_add_collation_edge(coll_qname, catalog)
                    {
                        g.add_edge(
                            NodeId::Type(ut.qname.clone()),
                            NodeId::Collation(coll_qname.clone()),
                        );
                    }
                }
            }
            UserTypeKind::Domain {
                base, collation, ..
            } => {
                if let ColumnType::UserDefined(base_qname) = base {
                    g.add_edge(
                        NodeId::Type(ut.qname.clone()),
                        NodeId::Type(base_qname.clone()),
                    );
                }
                if let Some(coll_qname) = collation
                    && should_add_collation_edge(coll_qname, catalog)
                {
                    g.add_edge(
                        NodeId::Type(ut.qname.clone()),
                        NodeId::Collation(coll_qname.clone()),
                    );
                }
            }
            UserTypeKind::Range {
                subtype,
                canonical,
                subtype_diff,
                collation,
                ..
            } => {
                // Skip built-in subtypes (pg_catalog.* are not user-managed).
                if subtype.schema.as_str() != "pg_catalog"
                    && catalog.types.iter().any(|t| &t.qname == subtype)
                {
                    g.add_edge(
                        NodeId::Type(ut.qname.clone()),
                        NodeId::Type(subtype.clone()),
                    );
                }
                if let Some(fn_qname) = canonical
                    && let Some(f) = catalog.functions.iter().find(|f| &f.qname == fn_qname)
                {
                    g.add_edge(
                        NodeId::Type(ut.qname.clone()),
                        NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone()),
                    );
                }
                if let Some(fn_qname) = subtype_diff
                    && let Some(f) = catalog.functions.iter().find(|f| &f.qname == fn_qname)
                {
                    g.add_edge(
                        NodeId::Type(ut.qname.clone()),
                        NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone()),
                    );
                }
                if let Some(coll_qname) = collation
                    && should_add_collation_edge(coll_qname, catalog)
                {
                    g.add_edge(
                        NodeId::Type(ut.qname.clone()),
                        NodeId::Collation(coll_qname.clone()),
                    );
                }
            }
            UserTypeKind::Enum { .. } => {}
        }
    }

    // Phase 1c: table → type edges from columns with user-defined types.
    // Tables that reference a user-defined type must be created after the type.
    // The same loop emits table → collation edges for columns with an explicit
    // `COLLATE` clause that points at a managed collation.
    for t in &catalog.tables {
        for col in &t.columns {
            if let ColumnType::UserDefined(type_qname) = &col.ty {
                g.add_edge(
                    NodeId::Table(t.qname.clone()),
                    NodeId::Type(type_qname.clone()),
                );
            }
            if let Some(coll_qname) = &col.collation
                && should_add_collation_edge(coll_qname, catalog)
            {
                g.add_edge(
                    NodeId::Table(t.qname.clone()),
                    NodeId::Collation(coll_qname.clone()),
                );
            }
        }
    }
    // Phase 1c (routines): function/procedure → types referenced in args and
    // return types. These ensure routines are created after their type deps.
    for f in &catalog.functions {
        let node = NodeId::Function(f.qname.clone(), f.arg_types_normalized.clone());
        for arg in &f.args {
            if let ColumnType::UserDefined(t_qname) = &arg.ty {
                g.add_edge(node.clone(), NodeId::Type(t_qname.clone()));
            }
        }
        match &f.return_type {
            ReturnType::Scalar {
                ty: ColumnType::UserDefined(t),
            }
            | ReturnType::SetOf {
                ty: ColumnType::UserDefined(t),
            } => {
                g.add_edge(node.clone(), NodeId::Type(t.clone()));
            }
            ReturnType::Table { columns } => {
                for col in columns {
                    if let ColumnType::UserDefined(t) = &col.ty {
                        g.add_edge(node.clone(), NodeId::Type(t.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    for p in &catalog.procedures {
        let node = NodeId::Procedure(p.qname.clone());
        for arg in &p.args {
            if let ColumnType::UserDefined(t_qname) = &arg.ty {
                g.add_edge(node.clone(), NodeId::Type(t_qname.clone()));
            }
        }
    }

    // Phase 2: tables depend on their schema and on any sequence used as a
    // column default. We add the schema node implicitly via add_edge in case
    // the caller did not declare it (defensive: source-side parsing typically
    // does declare every referenced schema, but a hand-built Catalog might not).
    // Partition children depend on their parent table.
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
        if let Some(po) = &t.partition_of {
            // Partition child depends on its parent existing first.
            g.add_edge(
                NodeId::Table(t.qname.clone()),
                NodeId::Table(po.parent.clone()),
            );
        }
    }

    // Phase 2b: views and MVs depend on objects in their body_dependencies.
    // `body_dependencies` edges use NodeId directly (already the correct
    // variant); we just re-register each edge into the graph.
    for v in &catalog.views {
        for dep in &v.body_dependencies {
            g.add_edge(dep.from.clone(), dep.to.clone());
        }
    }
    for mv in &catalog.materialized_views {
        for dep in &mv.body_dependencies {
            g.add_edge(dep.from.clone(), dep.to.clone());
        }
    }

    // Phase 2c: functions and procedures body_dependencies.
    for f in &catalog.functions {
        for dep in &f.body_dependencies {
            g.add_edge(dep.from.clone(), dep.to.clone());
        }
    }
    for p in &catalog.procedures {
        for dep in &p.body_dependencies {
            g.add_edge(dep.from.clone(), dep.to.clone());
        }
    }

    // Phase 3: indexes depend on their parent (table or MV).
    // For `IndexParent::Mv`, we use `NodeId::Mv` so the graph correctly
    // orders CREATE INDEX after CREATE MATERIALIZED VIEW.
    for i in &catalog.indexes {
        use crate::ir::index::IndexParent;
        let parent_node = match &i.on {
            IndexParent::Table(q) => NodeId::Table(q.clone()),
            IndexParent::Mv(q) => NodeId::Mv(q.clone()),
        };
        g.add_edge(NodeId::Index(i.qname.clone()), parent_node);
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
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn col_text_notnull(name: &str) -> Column {
        Column {
            name: id(name),
            ty: ColumnType::Text,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
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
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
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
            owner: None,
            grants: vec![],
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
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
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
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
                    storage: None,
                    compression: None,
                    comment: None,
                },
            ],
            constraints: vec![fk("users_orgs_fk", qn("app", "orgs"))],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            owner: None,
            grants: vec![],
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
                storage: None,
                compression: None,
                comment: None,
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            owner: None,
            grants: vec![],
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
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
                    storage: None,
                    compression: None,
                    comment: None,
                },
            ],
            constraints: vec![pk("a_pk", &["id"]), fk("a_to_b", qn("app", "b"))],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
                    storage: None,
                    compression: None,
                    comment: None,
                },
            ],
            constraints: vec![pk("b_pk", &["id"]), fk("b_to_a", qn("app", "a"))],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
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
                    storage: None,
                    compression: None,
                    comment: None,
                },
            ],
            constraints: vec![
                pk("tree_pk", &["id"]),
                fk("tree_parent_fk", qn("app", "tree")),
            ],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        let g = build_create_graph(&c);
        assert!(g.topological_sort().is_ok());
    }

    #[test]
    fn partition_child_depends_on_parent() {
        // A partition child table depends on its parent table being created first.
        use crate::ir::partition::{PartitionBounds, PartitionBy, PartitionOf};
        let mut c = Catalog::empty();

        // Parent table with PARTITION BY LIST.
        let parent = Table {
            qname: qn("app", "parent"),
            columns: vec![col_id_bigint(), col_text_notnull("status")],
            constraints: vec![pk("parent_pkey", &["id"])],
            partition_by: Some(PartitionBy {
                strategy: crate::ir::partition::PartitionStrategy::List,
                columns: vec![crate::ir::partition::PartitionColumn {
                    kind: crate::ir::partition::PartitionColumnKind::Column(id("status")),
                    collation: None,
                    opclass: None,
                }],
            }),
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        };
        c.tables.push(parent);

        // Child partition table.
        let child = Table {
            qname: qn("app", "child"),
            columns: vec![col_id_bigint(), col_text_notnull("status")],
            constraints: vec![],
            partition_by: None,
            partition_of: Some(PartitionOf {
                parent: qn("app", "parent"),
                bounds: PartitionBounds::List { values: vec![] },
            }),
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        };
        c.tables.push(child);

        let g = build_create_graph(&c);
        assert!(
            has_edge(
                &g,
                &NodeId::Table(qn("app", "child")),
                &NodeId::Table(qn("app", "parent")),
            ),
            "expected child partition → parent table edge"
        );
    }

    // ── User-defined type edge tests ──────────────────────────────────────────

    use crate::ir::user_type::{CompositeAttribute, UserType, UserTypeKind};

    fn make_enum(schema: &str, name: &str) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Enum { values: vec![] },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_composite_with_attr(schema: &str, name: &str, attr_type: ColumnType) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Composite {
                attributes: vec![CompositeAttribute {
                    name: id("val"),
                    ty: attr_type,
                    collation: None,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_domain_over(schema: &str, name: &str, base: ColumnType) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Domain {
                base,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn type_nodes_registered() {
        let mut c = Catalog::empty();
        c.schemas.push(crate::ir::schema::Schema::new(id("app")));
        c.types.push(make_enum("app", "status"));
        let g = build_create_graph(&c);
        // Type depends ONLY on its schema (no other edges for a bare enum).
        let deps: Vec<_> = g
            .dependencies_of(&NodeId::Type(qn("app", "status")))
            .collect();
        assert_eq!(deps, vec![&NodeId::Schema(id("app"))]);
        // Both the schema node and the type node are registered.
        assert_eq!(g.node_count(), 2);
    }

    #[test]
    fn table_depends_on_user_defined_column_type() {
        let mut c = Catalog::empty();
        c.types.push(make_enum("app", "status"));
        c.tables.push(Table {
            qname: qn("app", "orders"),
            columns: vec![Column {
                name: id("status"),
                ty: ColumnType::UserDefined(qn("app", "status")),
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                storage: None,
                compression: None,
                comment: None,
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        let g = build_create_graph(&c);
        assert!(
            has_edge(
                &g,
                &NodeId::Table(qn("app", "orders")),
                &NodeId::Type(qn("app", "status"))
            ),
            "table must depend on its user-defined column type"
        );
    }

    #[test]
    fn composite_depends_on_user_defined_attribute_type() {
        let mut c = Catalog::empty();
        c.types.push(make_enum("app", "inner_t"));
        c.types.push(make_composite_with_attr(
            "app",
            "outer_t",
            ColumnType::UserDefined(qn("app", "inner_t")),
        ));
        let g = build_create_graph(&c);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "outer_t")),
                &NodeId::Type(qn("app", "inner_t"))
            ),
            "composite must depend on the type of its user-defined attribute"
        );
    }

    #[test]
    fn domain_depends_on_user_defined_base_type() {
        let mut c = Catalog::empty();
        c.types.push(make_enum("app", "base_t"));
        c.types.push(make_domain_over(
            "app",
            "derived_t",
            ColumnType::UserDefined(qn("app", "base_t")),
        ));
        let g = build_create_graph(&c);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "derived_t")),
                &NodeId::Type(qn("app", "base_t"))
            ),
            "domain must depend on its user-defined base type"
        );
    }

    // ── Range type edge tests ────────────────────────────────────────────────

    fn make_range(
        schema: &str,
        name: &str,
        subtype: QualifiedName,
        canonical: Option<QualifiedName>,
        subtype_diff: Option<QualifiedName>,
    ) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Range {
                subtype,
                subtype_opclass: None,
                collation: None,
                canonical,
                subtype_diff,
                multirange_type_name: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_function(schema: &str, name: &str) -> crate::ir::function::Function {
        use crate::ir::function::{
            FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType, SecurityMode,
            Volatility,
        };
        use crate::parse::normalize_body::NormalizedBody;
        crate::ir::function::Function {
            qname: qn(schema, name),
            args: vec![],
            arg_types_normalized: NormalizedArgTypes::from_args(&[]),
            return_type: ReturnType::Scalar {
                ty: ColumnType::Integer,
            },
            language: FunctionLanguage::Sql,
            body: NormalizedBody::empty(),
            body_dependencies: vec![],
            volatility: Volatility::Volatile,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Unsafe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn range_with_builtin_subtype_adds_no_subtype_edge() {
        let mut c = Catalog::empty();
        c.schemas.push(crate::ir::schema::Schema::new(id("app")));
        c.types.push(make_range(
            "app",
            "ir",
            qn("pg_catalog", "int4"),
            None,
            None,
        ));
        let g = build_create_graph(&c);
        // Type edges: only schema; no edge to pg_catalog.int4.
        let deps: Vec<&NodeId> = g.dependencies_of(&NodeId::Type(qn("app", "ir"))).collect();
        assert_eq!(deps, vec![&NodeId::Schema(id("app"))]);
    }

    #[test]
    fn range_with_managed_subtype_adds_type_edge() {
        let mut c = Catalog::empty();
        c.schemas.push(crate::ir::schema::Schema::new(id("app")));
        c.types.push(make_enum("app", "base_t"));
        c.types.push(make_range(
            "app",
            "myrange",
            qn("app", "base_t"),
            None,
            None,
        ));
        let g = build_create_graph(&c);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "myrange")),
                &NodeId::Type(qn("app", "base_t"))
            ),
            "range type must depend on its managed subtype"
        );
    }

    #[test]
    fn range_canonical_fn_adds_function_edge() {
        let mut c = Catalog::empty();
        c.schemas.push(crate::ir::schema::Schema::new(id("app")));
        let f = make_function("app", "canon_fn");
        let arg_types = f.arg_types_normalized.clone();
        c.functions.push(f);
        c.types.push(make_range(
            "app",
            "myrange",
            qn("pg_catalog", "int4"),
            Some(qn("app", "canon_fn")),
            None,
        ));
        let g = build_create_graph(&c);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "myrange")),
                &NodeId::Function(qn("app", "canon_fn"), arg_types),
            ),
            "range type must depend on its canonical function"
        );
    }

    #[test]
    fn range_subtype_diff_fn_adds_function_edge() {
        let mut c = Catalog::empty();
        c.schemas.push(crate::ir::schema::Schema::new(id("app")));
        let f = make_function("app", "diff_fn");
        let arg_types = f.arg_types_normalized.clone();
        c.functions.push(f);
        c.types.push(make_range(
            "app",
            "myrange",
            qn("pg_catalog", "int4"),
            None,
            Some(qn("app", "diff_fn")),
        ));
        let g = build_create_graph(&c);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "myrange")),
                &NodeId::Function(qn("app", "diff_fn"), arg_types),
            ),
            "range type must depend on its subtype_diff function"
        );
    }

    #[test]
    fn range_unmanaged_canonical_fn_adds_no_edge() {
        // canonical references a function that is NOT in the source catalog.
        // No edge should be added (the lint rule would surface this later).
        let mut c = Catalog::empty();
        c.schemas.push(crate::ir::schema::Schema::new(id("app")));
        c.types.push(make_range(
            "app",
            "myrange",
            qn("pg_catalog", "int4"),
            Some(qn("app", "unmanaged_fn")),
            None,
        ));
        let g = build_create_graph(&c);
        // Only the schema edge.
        let deps: Vec<&NodeId> = g
            .dependencies_of(&NodeId::Type(qn("app", "myrange")))
            .collect();
        assert_eq!(deps, vec![&NodeId::Schema(id("app"))]);
    }

    // ── Collation edge tests ─────────────────────────────────────────────────

    use crate::ir::collation::{Collation, CollationProvider};

    fn make_collation(schema: &str, name: &str) -> Collation {
        Collation {
            qname: qn(schema, name),
            provider: CollationProvider::Libc,
            lc_collate: "C".into(),
            lc_ctype: "C".into(),
            deterministic: true,
            version: None,
            owner: None,
            comment: None,
        }
    }

    fn make_table_empty(schema: &str, name: &str) -> Table {
        Table {
            qname: qn(schema, name),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        }
    }

    fn col_text_with_collation(name: &str, collation: QualifiedName) -> Column {
        Column {
            name: id(name),
            ty: ColumnType::Text,
            nullable: true,
            default: None,
            identity: None,
            generated: None,
            collation: Some(collation),
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn make_domain_with_collation(schema: &str, name: &str, collation: QualifiedName) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Domain {
                base: ColumnType::Text,
                nullable: true,
                default: None,
                check_constraints: vec![],
                collation: Some(collation),
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_range_with_collation(
        schema: &str,
        name: &str,
        subtype: QualifiedName,
        collation: QualifiedName,
    ) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Range {
                subtype,
                subtype_opclass: None,
                collation: Some(collation),
                canonical: None,
                subtype_diff: None,
                multirange_type_name: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn make_composite_with_collated_attr(
        schema: &str,
        name: &str,
        collation: QualifiedName,
    ) -> UserType {
        UserType {
            qname: qn(schema, name),
            kind: UserTypeKind::Composite {
                attributes: vec![CompositeAttribute {
                    name: id("val"),
                    ty: ColumnType::Text,
                    collation: Some(collation),
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn collation_node_registered() {
        let mut c = Catalog::empty();
        c.schemas.push(crate::ir::schema::Schema::new(id("app")));
        c.collations.push(make_collation("app", "ci"));
        let g = build_create_graph(&c);
        // schema + collation nodes; no edges.
        assert_eq!(g.node_count(), 2);
    }

    #[test]
    fn column_with_managed_collation_adds_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.collations.push(make_collation("app", "ci"));
        let mut t = make_table_empty("app", "users");
        t.columns
            .push(col_text_with_collation("email", qn("app", "ci")));
        cat.tables.push(t);
        let g = build_create_graph(&cat);
        assert!(
            has_edge(
                &g,
                &NodeId::Table(qn("app", "users")),
                &NodeId::Collation(qn("app", "ci")),
            ),
            "table must depend on its column's managed collation"
        );
    }

    #[test]
    fn column_with_pg_catalog_collation_no_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        let mut t = make_table_empty("app", "users");
        t.columns
            .push(col_text_with_collation("email", qn("pg_catalog", "C")));
        cat.tables.push(t);
        let g = build_create_graph(&cat);
        assert!(
            !g.edges().any(|(_, to)| matches!(to, NodeId::Collation(_))),
            "no collation edge should be added for pg_catalog.C"
        );
    }

    #[test]
    fn column_with_builtin_shortname_collation_no_edge() {
        // Even with no schema (or any schema), the builtin shortname `POSIX`
        // should be skipped because it's in BUILTIN_COLLATIONS.
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        let mut t = make_table_empty("app", "users");
        t.columns
            .push(col_text_with_collation("email", qn("app", "POSIX")));
        cat.tables.push(t);
        let g = build_create_graph(&cat);
        assert!(!g.edges().any(|(_, to)| matches!(to, NodeId::Collation(_))),);
    }

    #[test]
    fn column_with_unmanaged_collation_no_edge() {
        // Source references a collation that isn't in catalog.collations →
        // no edge (the lint rule surfaces the drift; here we just don't add
        // a phantom edge to a node that doesn't exist).
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        let mut t = make_table_empty("app", "users");
        t.columns
            .push(col_text_with_collation("email", qn("app", "unmanaged")));
        cat.tables.push(t);
        let g = build_create_graph(&cat);
        assert!(!g.edges().any(|(_, to)| matches!(to, NodeId::Collation(_))),);
    }

    #[test]
    fn domain_with_managed_collation_adds_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.collations.push(make_collation("app", "ci"));
        cat.types.push(make_domain_with_collation(
            "app",
            "email_t",
            qn("app", "ci"),
        ));
        let g = build_create_graph(&cat);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "email_t")),
                &NodeId::Collation(qn("app", "ci")),
            ),
            "domain must depend on its managed collation"
        );
    }

    #[test]
    fn domain_with_pg_catalog_collation_no_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.types.push(make_domain_with_collation(
            "app",
            "email_t",
            qn("pg_catalog", "C"),
        ));
        let g = build_create_graph(&cat);
        assert!(!g.edges().any(|(_, to)| matches!(to, NodeId::Collation(_))),);
    }

    #[test]
    fn range_with_managed_collation_adds_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.collations.push(make_collation("app", "ci"));
        cat.types.push(make_range_with_collation(
            "app",
            "textrange",
            qn("pg_catalog", "text"),
            qn("app", "ci"),
        ));
        let g = build_create_graph(&cat);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "textrange")),
                &NodeId::Collation(qn("app", "ci")),
            ),
            "range type must depend on its managed collation"
        );
    }

    #[test]
    fn range_with_pg_catalog_collation_no_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.types.push(make_range_with_collation(
            "app",
            "textrange",
            qn("pg_catalog", "text"),
            qn("pg_catalog", "C"),
        ));
        let g = build_create_graph(&cat);
        assert!(!g.edges().any(|(_, to)| matches!(to, NodeId::Collation(_))),);
    }

    #[test]
    fn composite_attribute_with_managed_collation_adds_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.collations.push(make_collation("app", "ci"));
        cat.types.push(make_composite_with_collated_attr(
            "app",
            "addr_t",
            qn("app", "ci"),
        ));
        let g = build_create_graph(&cat);
        assert!(
            has_edge(
                &g,
                &NodeId::Type(qn("app", "addr_t")),
                &NodeId::Collation(qn("app", "ci")),
            ),
            "composite type must depend on collation of a collated attribute"
        );
    }

    #[test]
    fn composite_attribute_with_pg_catalog_collation_no_edge() {
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.types.push(make_composite_with_collated_attr(
            "app",
            "addr_t",
            qn("pg_catalog", "C"),
        ));
        let g = build_create_graph(&cat);
        assert!(!g.edges().any(|(_, to)| matches!(to, NodeId::Collation(_))),);
    }

    #[test]
    fn collation_ordered_before_table_using_it() {
        // Topological sort must place the collation before the table that
        // references it (since the edge is Table → Collation).
        let mut cat = Catalog::empty();
        cat.schemas.push(crate::ir::schema::Schema::new(id("app")));
        cat.collations.push(make_collation("app", "ci"));
        let mut t = make_table_empty("app", "users");
        t.columns
            .push(col_text_with_collation("email", qn("app", "ci")));
        cat.tables.push(t);
        let g = build_create_graph(&cat);
        let order = g.topological_sort().expect("no cycle");
        let coll_pos = order
            .iter()
            .position(|n| n == &NodeId::Collation(qn("app", "ci")))
            .expect("collation in order");
        let table_pos = order
            .iter()
            .position(|n| n == &NodeId::Table(qn("app", "users")))
            .expect("table in order");
        assert!(
            coll_pos < table_pos,
            "collation must be created before the table that uses it"
        );
    }

    #[test]
    fn type_create_ordering_respects_edges() {
        // derived_t depends on base_t; topological sort must put base_t first.
        let mut c = Catalog::empty();
        c.types.push(make_enum("app", "base_t"));
        c.types.push(make_domain_over(
            "app",
            "derived_t",
            ColumnType::UserDefined(qn("app", "base_t")),
        ));
        let g = build_create_graph(&c);
        let order = g.topological_sort().expect("no cycle expected");
        let base_pos = order
            .iter()
            .position(|n| n == &NodeId::Type(qn("app", "base_t")))
            .expect("base_t in order");
        let derived_pos = order
            .iter()
            .position(|n| n == &NodeId::Type(qn("app", "derived_t")))
            .expect("derived_t in order");
        assert!(base_pos < derived_pos, "base_t must come before derived_t");
    }
}
