//! SQL rendering for publication operations.
//!
//! Each public function corresponds to one DML kind on a Postgres PUBLICATION.
//! All helpers return a complete SQL statement including the trailing semicolon.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};

/// `CREATE PUBLICATION p … WITH (…);`
///
/// Handles all five scope forms:
/// - `FOR ALL TABLES`
/// - `FOR TABLE x [(cols)] [WHERE (filter)], …`
/// - `FOR TABLES IN SCHEMA s, …`
/// - Mixed `FOR TABLE … , TABLES IN SCHEMA …`
///
/// The `WITH (…)` clause is omitted when all options are at their PG defaults.
#[must_use]
pub fn create_publication(p: &Publication) -> String {
    let mut s = format!("CREATE PUBLICATION {}", p.name.render_sql());
    match &p.scope {
        PublicationScope::AllTables => s.push_str(" FOR ALL TABLES"),
        PublicationScope::Selective { schemas, tables } => {
            s.push_str(" FOR ");
            let mut first = true;
            if !tables.is_empty() {
                s.push_str("TABLE ");
                for t in tables {
                    if !first {
                        s.push_str(", ");
                    }
                    s.push_str(&render_published_table(t));
                    first = false;
                }
            }
            if !schemas.is_empty() {
                if !first {
                    s.push_str(", ");
                }
                s.push_str("TABLES IN SCHEMA ");
                let names: Vec<String> = schemas.iter().map(Identifier::render_sql).collect();
                s.push_str(&names.join(", "));
            }
        }
    }
    s.push_str(&render_with_options(p));
    s.push(';');
    s
}

/// `DROP PUBLICATION p;`
#[must_use]
pub fn drop_publication(name: &Identifier) -> String {
    format!("DROP PUBLICATION {};", name.render_sql())
}

/// Two-step replace: `[DROP PUBLICATION old, CREATE PUBLICATION new]`.
///
/// The caller is responsible for joining with a newline when rendering both
/// steps into a single `RawStep` SQL body.
#[must_use]
pub fn replace_publication(from: &Publication, to: &Publication) -> [String; 2] {
    [drop_publication(&from.name), create_publication(to)]
}

/// `ALTER PUBLICATION p ADD TABLE x [(cols)] [WHERE (filter)];`
#[must_use]
pub fn alter_publication_add_table(pname: &Identifier, t: &PublishedTable) -> String {
    format!(
        "ALTER PUBLICATION {} ADD TABLE {};",
        pname.render_sql(),
        render_published_table(t),
    )
}

/// `ALTER PUBLICATION p DROP TABLE x;`
#[must_use]
pub fn alter_publication_drop_table(pname: &Identifier, qname: &QualifiedName) -> String {
    format!(
        "ALTER PUBLICATION {} DROP TABLE {};",
        pname.render_sql(),
        qname.render_sql(),
    )
}

/// `ALTER PUBLICATION p SET TABLE x [(cols)] [WHERE (filter)];`
///
/// Replaces just that one table's specification without affecting other tables
/// in the publication.
#[must_use]
pub fn alter_publication_set_table(pname: &Identifier, t: &PublishedTable) -> String {
    format!(
        "ALTER PUBLICATION {} SET TABLE {};",
        pname.render_sql(),
        render_published_table(t),
    )
}

/// `ALTER PUBLICATION p ADD TABLES IN SCHEMA s;` (PG 15+)
#[must_use]
pub fn alter_publication_add_schema(pname: &Identifier, schema: &Identifier) -> String {
    format!(
        "ALTER PUBLICATION {} ADD TABLES IN SCHEMA {};",
        pname.render_sql(),
        schema.render_sql(),
    )
}

/// `ALTER PUBLICATION p DROP TABLES IN SCHEMA s;` (PG 15+)
#[must_use]
pub fn alter_publication_drop_schema(pname: &Identifier, schema: &Identifier) -> String {
    format!(
        "ALTER PUBLICATION {} DROP TABLES IN SCHEMA {};",
        pname.render_sql(),
        schema.render_sql(),
    )
}

/// `ALTER PUBLICATION p SET (publish = '…');`
#[must_use]
pub fn alter_publication_set_publish(pname: &Identifier, k: PublishKinds) -> String {
    format!(
        "ALTER PUBLICATION {} SET (publish = '{}');",
        pname.render_sql(),
        render_publish_kinds(k),
    )
}

/// `ALTER PUBLICATION p SET (publish_via_partition_root = …);`
#[must_use]
pub fn alter_publication_set_via_root(pname: &Identifier, value: bool) -> String {
    format!(
        "ALTER PUBLICATION {} SET (publish_via_partition_root = {value});",
        pname.render_sql(),
    )
}

/// `COMMENT ON PUBLICATION p IS '…';` or `… IS NULL;` when `comment` is `None`.
///
/// Single quotes inside the comment body are escaped by doubling.
#[must_use]
pub fn comment_on_publication(name: &Identifier, comment: Option<&str>) -> String {
    let body = comment.map_or_else(
        || "NULL".to_string(),
        |c| format!("'{}'", c.replace('\'', "''")),
    );
    format!("COMMENT ON PUBLICATION {} IS {body};", name.render_sql())
}

// ─── private helpers ──────────────────────────────────────────────────────────

/// Render a single table entry (qname, optional column list, optional WHERE filter).
fn render_published_table(t: &PublishedTable) -> String {
    let mut s = t.qname.render_sql();
    if let Some(cols) = &t.columns {
        s.push_str(" (");
        let names: Vec<String> = cols.iter().map(Identifier::render_sql).collect();
        s.push_str(&names.join(", "));
        s.push(')');
    }
    if let Some(filter) = &t.row_filter {
        s.push_str(" WHERE (");
        s.push_str(&filter.canonical_text);
        s.push(')');
    }
    s
}

/// Render the `WITH (…)` clause, omitting it entirely when all values are at
/// their PG defaults (all DML kinds enabled, `publish_via_partition_root = false`).
fn render_with_options(p: &Publication) -> String {
    let mut parts = Vec::new();
    if p.publish != PublishKinds::pg_default() {
        parts.push(format!("publish = '{}'", render_publish_kinds(p.publish)));
    }
    if p.publish_via_partition_root {
        parts.push("publish_via_partition_root = true".to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" WITH ({})", parts.join(", "))
    }
}

/// Render a `PublishKinds` bitset as a comma-separated keyword list.
///
/// The canonical PG ordering is `insert, update, delete, truncate`.
fn render_publish_kinds(k: PublishKinds) -> String {
    let mut parts = Vec::new();
    if k.insert {
        parts.push("insert");
    }
    if k.update {
        parts.push("update");
    }
    if k.delete {
        parts.push("delete");
    }
    if k.truncate {
        parts.push("truncate");
    }
    parts.join(", ")
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::default_expr::NormalizedExpr;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn all_tables_pub() -> Publication {
        Publication {
            name: id("p"),
            scope: PublicationScope::AllTables,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn renders_create_for_all_tables() {
        let p = all_tables_pub();
        assert_eq!(
            create_publication(&p),
            "CREATE PUBLICATION p FOR ALL TABLES;"
        );
    }

    #[test]
    fn renders_create_for_all_tables_omits_default_with_options() {
        let p = all_tables_pub();
        // With-options empty when everything is at PG defaults.
        assert!(!create_publication(&p).contains("WITH"));
    }

    #[test]
    fn renders_create_for_table_with_columns_and_filter() {
        let filter = NormalizedExpr::from_text("status = 'active'");
        let table = PublishedTable {
            qname: qn("app", "t"),
            row_filter: Some(filter),
            columns: Some(vec![id("id"), id("name")]),
        };
        let p = Publication {
            name: id("p"),
            scope: PublicationScope::Selective {
                schemas: std::collections::BTreeSet::new(),
                tables: vec![table],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        let sql = create_publication(&p);
        assert!(sql.contains("FOR TABLE app.t (id, name) WHERE ("));
        assert!(sql.contains("status = 'active'"));
    }

    #[test]
    fn renders_publish_kinds_subset() {
        let k = PublishKinds {
            insert: true,
            update: true,
            delete: false,
            truncate: false,
        };
        assert_eq!(render_publish_kinds(k), "insert, update");
    }

    #[test]
    fn renders_publish_kinds_single() {
        let k = PublishKinds {
            insert: true,
            update: false,
            delete: false,
            truncate: false,
        };
        assert_eq!(render_publish_kinds(k), "insert");
    }

    #[test]
    fn with_options_empty_at_pg_defaults() {
        let p = all_tables_pub();
        assert_eq!(render_with_options(&p), "");
    }

    #[test]
    fn with_options_renders_non_default_publish() {
        let mut p = all_tables_pub();
        p.publish = PublishKinds {
            insert: true,
            update: false,
            delete: false,
            truncate: false,
        };
        assert_eq!(render_with_options(&p), " WITH (publish = 'insert')");
    }

    #[test]
    fn with_options_renders_via_root() {
        let mut p = all_tables_pub();
        p.publish_via_partition_root = true;
        assert_eq!(
            render_with_options(&p),
            " WITH (publish_via_partition_root = true)"
        );
    }

    #[test]
    fn renders_alter_set_publish() {
        let k = PublishKinds {
            insert: true,
            update: false,
            delete: false,
            truncate: false,
        };
        assert_eq!(
            alter_publication_set_publish(&id("p"), k),
            "ALTER PUBLICATION p SET (publish = 'insert');",
        );
    }

    #[test]
    fn renders_alter_set_via_root() {
        assert_eq!(
            alter_publication_set_via_root(&id("p"), true),
            "ALTER PUBLICATION p SET (publish_via_partition_root = true);",
        );
        assert_eq!(
            alter_publication_set_via_root(&id("p"), false),
            "ALTER PUBLICATION p SET (publish_via_partition_root = false);",
        );
    }

    #[test]
    fn comment_on_publication_with_null() {
        assert_eq!(
            comment_on_publication(&id("p"), None),
            "COMMENT ON PUBLICATION p IS NULL;",
        );
    }

    #[test]
    fn comment_on_publication_with_value() {
        assert_eq!(
            comment_on_publication(&id("p"), Some("my pub")),
            "COMMENT ON PUBLICATION p IS 'my pub';",
        );
    }

    #[test]
    fn comment_on_publication_escapes_single_quotes() {
        assert_eq!(
            comment_on_publication(&id("p"), Some("it's cool")),
            "COMMENT ON PUBLICATION p IS 'it''s cool';",
        );
    }

    #[test]
    fn replace_publication_returns_drop_then_create() {
        let from = all_tables_pub();
        let mut to = all_tables_pub();
        to.name = id("q");
        let [drop_sql, create_sql] = replace_publication(&from, &to);
        assert_eq!(drop_sql, "DROP PUBLICATION p;");
        assert_eq!(create_sql, "CREATE PUBLICATION q FOR ALL TABLES;");
    }

    #[test]
    fn renders_drop_publication() {
        assert_eq!(drop_publication(&id("p")), "DROP PUBLICATION p;");
    }

    #[test]
    fn renders_alter_add_table() {
        let t = PublishedTable {
            qname: qn("app", "orders"),
            row_filter: None,
            columns: None,
        };
        assert_eq!(
            alter_publication_add_table(&id("p"), &t),
            "ALTER PUBLICATION p ADD TABLE app.orders;",
        );
    }

    #[test]
    fn renders_alter_drop_table() {
        assert_eq!(
            alter_publication_drop_table(&id("p"), &qn("app", "orders")),
            "ALTER PUBLICATION p DROP TABLE app.orders;",
        );
    }

    #[test]
    fn renders_alter_add_schema() {
        assert_eq!(
            alter_publication_add_schema(&id("p"), &id("app")),
            "ALTER PUBLICATION p ADD TABLES IN SCHEMA app;",
        );
    }

    #[test]
    fn renders_alter_drop_schema() {
        assert_eq!(
            alter_publication_drop_schema(&id("p"), &id("app")),
            "ALTER PUBLICATION p DROP TABLES IN SCHEMA app;",
        );
    }

    #[test]
    fn renders_create_for_tables_in_schema() {
        let p = Publication {
            name: id("p"),
            scope: PublicationScope::Selective {
                schemas: std::collections::BTreeSet::from([id("app")]),
                tables: vec![],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        };
        assert_eq!(
            create_publication(&p),
            "CREATE PUBLICATION p FOR TABLES IN SCHEMA app;",
        );
    }
}
