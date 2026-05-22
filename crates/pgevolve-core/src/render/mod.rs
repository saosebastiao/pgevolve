//! IR → SQL emitter.
//!
//! Produces source-style `CREATE` statements for each Catalog object.
//! Used by `pgevolve dump` and by future regression-capture tooling.
//!
//! ## Output contract
//!
//! - Schemas first, then tables (with their inline constraints, excluding FKs),
//!   then FK `ALTER TABLE ... ADD CONSTRAINT` statements (to handle FK cycles),
//!   then standalone indexes, then sequences.
//! - Output is deterministic: for equal `Catalog` inputs, byte-identical SQL is
//!   produced.  Objects are emitted in their iteration order (callers wanting
//!   canonical sort should call [`Catalog::canonicalize`] first).
//! - Comments are emitted as `COMMENT ON ...` statements immediately after the
//!   object they describe.
//!
//! ## v0.1 limitations
//!
//! - Views / materialized views / functions / triggers are not emitted (they
//!   are not modelled in the v0.1 IR).
//! - The output does NOT include pgevolve source directives (e.g.
//!   `-- pgevolve: intent = ...`), so a directory written by `pgevolve dump`
//!   cannot be fed directly to `pgevolve lint` or `parse_directory` without
//!   first adding those directives.  Users running `pgevolve init` against the
//!   dump output should add directives manually, or use a future `pgevolve
//!   annotate` helper.

pub mod index;
pub mod schema;
pub mod sequence;
pub mod table;
pub mod view;

use crate::ir::catalog::Catalog;
use crate::ir::constraint::ConstraintKind;

/// Render an entire `Catalog` as a single SQL string with one `CREATE`
/// statement per object, in dependency-correct order.
///
/// Order: schemas → tables (inline PK/UK/CHECK only) → FK `ALTER TABLE ADD
/// CONSTRAINT` stmts → standalone indexes → sequences.  Each block is
/// separated by a blank line for readability.
#[must_use]
pub fn render_catalog(catalog: &Catalog) -> String {
    let mut out = String::new();

    // 1. Schemas.
    for s in &catalog.schemas {
        out.push_str(&schema::render_schema(s));
        out.push('\n');
    }
    if !catalog.schemas.is_empty() {
        out.push('\n');
    }

    // 2. Tables (without FK constraints inline — those come after).
    for t in &catalog.tables {
        out.push_str(&table::render_table(t));
        out.push('\n');
    }
    if !catalog.tables.is_empty() {
        out.push('\n');
    }

    // 3. FK constraints as ALTER TABLE ADD CONSTRAINT.
    let mut had_fk = false;
    for t in &catalog.tables {
        for c in &t.constraints {
            if matches!(c.kind, ConstraintKind::ForeignKey(_)) {
                out.push_str(&table::render_add_fk(&t.qname, c));
                out.push('\n');
                had_fk = true;
            }
        }
    }
    if had_fk {
        out.push('\n');
    }

    // 4. Standalone indexes.
    for i in &catalog.indexes {
        out.push_str(&index::render_index(i));
        out.push('\n');
    }
    if !catalog.indexes.is_empty() {
        out.push('\n');
    }

    // 5. Sequences.
    for s in &catalog.sequences {
        out.push_str(&sequence::render_sequence(s));
        out.push('\n');
    }
    if !catalog.sequences.is_empty() {
        out.push('\n');
    }

    // 6. Views (after sequences — views may reference sequence defaults).
    for v in &catalog.views {
        out.push_str(&view::render_view(v));
        out.push('\n');
    }
    if !catalog.views.is_empty() {
        out.push('\n');
    }

    // 7. Materialized views.
    for mv in &catalog.materialized_views {
        out.push_str(&view::render_materialized_view(mv));
        out.push('\n');
    }

    // Strip trailing blank line.
    while out.ends_with("\n\n") {
        out.pop();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::ir::schema::Schema;
    use crate::ir::sequence::Sequence;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    #[test]
    fn empty_catalog_renders_empty() {
        let rendered = render_catalog(&Catalog::empty());
        // Should be empty or only whitespace.
        assert!(rendered.trim().is_empty());
    }

    #[test]
    fn schema_renders_first() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
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
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        let rendered = render_catalog(&cat);
        let schema_pos = rendered.find("CREATE SCHEMA").unwrap();
        let table_pos = rendered.find("CREATE TABLE").unwrap();
        assert!(schema_pos < table_pos, "schema must come before table");
    }

    #[test]
    #[allow(clippy::too_many_lines)] // exhaustive catalog fixture — structural, not logic complexity
    fn fk_rendered_after_table() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.tables.push(Table {
            qname: qn("app", "orgs"),
            columns: vec![Column {
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
            }],
            constraints: vec![Constraint {
                qname: qn("app", "orgs_pkey"),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![id("id")],
                    include: vec![],
                },
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
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
                },
                Column {
                    name: id("org_id"),
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
            constraints: vec![
                Constraint {
                    qname: qn("app", "users_pkey"),
                    kind: ConstraintKind::PrimaryKey {
                        columns: vec![id("id")],
                        include: vec![],
                    },
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
                Constraint {
                    qname: qn("app", "users_org_fkey"),
                    kind: ConstraintKind::ForeignKey(ForeignKey {
                        columns: vec![id("org_id")],
                        referenced_table: qn("app", "orgs"),
                        referenced_columns: vec![id("id")],
                        on_update: ReferentialAction::NoAction,
                        on_delete: ReferentialAction::NoAction,
                        match_type: FkMatchType::Simple,
                    }),
                    deferrable: Deferrable::NotDeferrable,
                    comment: None,
                },
            ],
            partition_by: None,
            partition_of: None,
            comment: None,
        });

        let rendered = render_catalog(&cat);

        // FK must appear as ALTER TABLE, not inline in CREATE TABLE.
        assert!(
            rendered.contains("ALTER TABLE"),
            "expected ALTER TABLE for FK"
        );
        let create_table_pos = rendered.rfind("CREATE TABLE").unwrap();
        let alter_pos = rendered.find("ALTER TABLE").unwrap();
        assert!(
            create_table_pos < alter_pos,
            "ALTER TABLE must come after last CREATE TABLE"
        );

        // FK should NOT appear inline in CREATE TABLE.
        let create_table_end = rendered.find("ALTER TABLE").unwrap();
        let create_table_section = &rendered[..create_table_end];
        assert!(
            !create_table_section.contains("FOREIGN KEY"),
            "FK should not be inline in CREATE TABLE"
        );
    }

    #[test]
    fn indexes_rendered_after_tables() {
        let mut cat = Catalog::empty();
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
                name: id("email"),
                ty: ColumnType::Text,
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
        });
        cat.indexes.push(Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("email")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        });

        let rendered = render_catalog(&cat);
        let table_pos = rendered.find("CREATE TABLE").unwrap();
        let index_pos = rendered.find("CREATE UNIQUE INDEX").unwrap();
        assert!(table_pos < index_pos, "index must come after table");
    }

    #[test]
    fn sequences_rendered_last() {
        let mut cat = Catalog::empty();
        cat.sequences.push(Sequence {
            qname: qn("app", "users_id_seq"),
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
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
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
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        });

        let rendered = render_catalog(&cat);
        let table_pos = rendered.find("CREATE TABLE").unwrap();
        let seq_pos = rendered.find("CREATE SEQUENCE").unwrap();
        assert!(table_pos < seq_pos, "sequence must come after table");
    }

    #[test]
    fn parseable_by_pg_query() {
        let mut cat = Catalog::empty();
        cat.schemas.push(Schema::new(id("app")));
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![
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
                },
                Column {
                    name: id("email"),
                    ty: ColumnType::Text,
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
            constraints: vec![Constraint {
                qname: qn("app", "users_pkey"),
                kind: ConstraintKind::PrimaryKey {
                    columns: vec![id("id")],
                    include: vec![],
                },
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
            partition_by: None,
            partition_of: None,
            comment: Some("user accounts".into()),
        });
        cat.indexes.push(Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("email")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        });

        let rendered = render_catalog(&cat);

        // The whole block must be parseable by pg_query as multi-statement SQL.
        let r = pg_query::parse(&rendered);
        assert!(
            r.is_ok(),
            "pg_query rejected rendered catalog:\n{rendered}\nerr: {r:?}"
        );
    }

    #[test]
    fn views_rendered_after_sequences() {
        use crate::ir::view::View;
        use crate::parse::normalize_body::NormalizedBody;

        let mut cat = Catalog::empty();
        cat.sequences.push(Sequence {
            qname: qn("app", "users_id_seq"),
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
        cat.views.push(View {
            qname: qn("app", "active_users"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
        });

        let rendered = render_catalog(&cat);
        let seq_pos = rendered.find("CREATE SEQUENCE").unwrap();
        let view_pos = rendered.find("CREATE VIEW").unwrap();
        assert!(seq_pos < view_pos, "views must come after sequences");
    }

    #[test]
    fn materialized_views_rendered_in_catalog() {
        use crate::ir::view::MaterializedView;
        use crate::parse::normalize_body::NormalizedBody;

        let mut cat = Catalog::empty();
        cat.materialized_views.push(MaterializedView {
            qname: qn("app", "summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT count(*) FROM app.users").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
        });

        let rendered = render_catalog(&cat);
        assert!(
            rendered.contains("CREATE MATERIALIZED VIEW"),
            "expected CREATE MATERIALIZED VIEW: {rendered}"
        );
    }
}
