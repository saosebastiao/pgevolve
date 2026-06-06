//! Warns when a publication row filter references a column not declared in
//! source on the target table.
//!
//! A row filter expression like `status = 'active'` uses column names from
//! the published table. If source doesn't declare those columns, the row
//! filter is referencing something outside the managed schema — likely drift
//! or a typo that would fail at apply time.
//!
//! Source-only rule: checks the source catalog against itself (no live catalog
//! needed). Fires for each `PublishedTable` whose `row_filter` references a
//! column name not present in the corresponding source table.
//!
//! Implementation parses the row filter's `canonical_text` with `pg_query`
//! (wrapping in `SELECT … WHERE …` so the parser can handle an expression),
//! then walks the AST for `ColumnRef` nodes.

use crate::ir::catalog::Catalog;
use crate::ir::publication::PublicationScope;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "publication-row-filter-references-unmanaged-column";

/// Extract simple (unqualified) column names referenced in a SQL expression.
///
/// The expression is wrapped in `SELECT * FROM t WHERE <expr>` so `pg_query`
/// can parse it. Only bare `ColumnRef` nodes (no schema prefix) are returned;
/// qualified references are ignored because a row filter on a single table
/// cannot have schema-qualified column refs.
///
/// Returns `None` if the expression fails to parse (caller silently skips).
fn extract_column_refs_from_expr(expr_text: &str) -> Option<Vec<String>> {
    // Wrap in a syntactically valid SELECT so pg_query can parse the expression.
    let sql = format!("SELECT * FROM _t WHERE {expr_text}");
    let parsed = pg_query::parse(&sql).ok()?;
    let mut names = Vec::new();
    for stmt in &parsed.protobuf.stmts {
        let Some(node) = &stmt.stmt else { continue };
        collect_column_refs(node, &mut names);
    }
    Some(names)
}

/// Recursively walk a protobuf node and collect unqualified `ColumnRef` names.
fn collect_column_refs(node: &pg_query::protobuf::Node, out: &mut Vec<String>) {
    use pg_query::NodeEnum;
    let Some(inner) = &node.node else { return };

    match inner {
        NodeEnum::ColumnRef(cref)
            // Only collect single-field column refs (not schema.table.col).
            if cref.fields.len() == 1 =>
        {
            if let Some(field_node) = cref.fields.first()
                && let Some(NodeEnum::String(s)) = &field_node.node
                && !s.sval.is_empty()
            {
                out.push(s.sval.clone());
            }
        }
        // Walk into sub-nodes via the standard protobuf node children.
        // We handle the most common expression node types here.
        NodeEnum::SelectStmt(sel) => {
            if let Some(w) = &sel.where_clause {
                collect_column_refs(w, out);
            }
            for t in &sel.target_list {
                collect_column_refs(t, out);
            }
        }
        NodeEnum::BoolExpr(b) => {
            for arg in &b.args {
                collect_column_refs(arg, out);
            }
        }
        NodeEnum::AExpr(a) => {
            if let Some(l) = &a.lexpr {
                collect_column_refs(l, out);
            }
            if let Some(r) = &a.rexpr {
                collect_column_refs(r, out);
            }
        }
        NodeEnum::SubLink(sl) => {
            if let Some(t) = &sl.testexpr {
                collect_column_refs(t, out);
            }
            if let Some(q) = &sl.subselect {
                collect_column_refs(q, out);
            }
        }
        NodeEnum::FuncCall(fc) => {
            for arg in &fc.args {
                collect_column_refs(arg, out);
            }
        }
        NodeEnum::NullTest(nt) => {
            if let Some(a) = &nt.arg {
                collect_column_refs(a, out);
            }
        }
        NodeEnum::BooleanTest(bt) => {
            if let Some(a) = &bt.arg {
                collect_column_refs(a, out);
            }
        }
        NodeEnum::CaseExpr(ce) => {
            if let Some(a) = &ce.arg {
                collect_column_refs(a, out);
            }
            for w in &ce.args {
                collect_column_refs(w, out);
            }
            if let Some(d) = &ce.defresult {
                collect_column_refs(d, out);
            }
        }
        NodeEnum::CaseWhen(cw) => {
            if let Some(e) = &cw.expr {
                collect_column_refs(e, out);
            }
            if let Some(r) = &cw.result {
                collect_column_refs(r, out);
            }
        }
        NodeEnum::ResTarget(rt) => {
            if let Some(v) = &rt.val {
                collect_column_refs(v, out);
            }
        }
        NodeEnum::TypeCast(tc) => {
            if let Some(a) = &tc.arg {
                collect_column_refs(a, out);
            }
        }
        _ => {}
    }
}

/// Source-only check: fires for each `PublishedTable` in source's publications
/// whose row filter references a column not declared on the source table.
pub fn check(source: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();

    for pub_ in &source.publications {
        let PublicationScope::Selective { tables, .. } = &pub_.scope else {
            continue;
        };

        for pt in tables {
            let Some(row_filter) = &pt.row_filter else {
                continue;
            };

            // Find the corresponding source table to check column existence.
            let Some(src_table) = source.tables.iter().find(|t| t.qname == pt.qname) else {
                // Table not in source — another rule covers this; skip here.
                continue;
            };

            let source_col_names: std::collections::BTreeSet<&str> =
                src_table.columns.iter().map(|c| c.name.as_str()).collect();

            let Some(ref_cols) = extract_column_refs_from_expr(&row_filter.canonical_text) else {
                // Parse failed — silently skip.
                continue;
            };

            for col_name in ref_cols {
                if !source_col_names.contains(col_name.as_str()) {
                    findings.push(Finding {
                        rule: RULE_ID,
                        severity: Severity::Warning,
                        message: format!(
                            "publication {}: row filter on table {} references column {} which is not declared in source",
                            pub_.name, pt.qname, col_name,
                        ),
                        location: None,
                    });
                }
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};
    use crate::ir::reloptions::TableStorageOptions;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn make_table_with_cols(qname: QualifiedName, cols: &[&str]) -> Table {
        Table {
            qname,
            columns: cols
                .iter()
                .map(|c| Column {
                    name: id(c),
                    ty: ColumnType::Text,
                    nullable: true,
                    default: None,
                    identity: None,
                    generated: None,
                    collation: None,
                    storage: None,
                    compression: None,
                    comment: None,
                })
                .collect(),
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: TableStorageOptions::default(),
            access_method: None,
        }
    }

    fn make_pub_with_filter(
        pub_name: &str,
        table_qname: QualifiedName,
        filter_text: &str,
    ) -> Publication {
        Publication {
            name: id(pub_name),
            scope: PublicationScope::Selective {
                schemas: std::collections::BTreeSet::new(),
                tables: vec![PublishedTable {
                    qname: table_qname,
                    row_filter: Some(NormalizedExpr::from_text(filter_text)),
                    columns: None,
                }],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn empty_catalog_silent() {
        let source = Catalog::empty();
        assert!(check(&source).is_empty());
    }

    #[test]
    fn row_filter_with_managed_column_silent() {
        let mut source = Catalog::empty();
        source
            .tables
            .push(make_table_with_cols(qn("app", "orders"), &["id", "status"]));
        source.publications.push(make_pub_with_filter(
            "p",
            qn("app", "orders"),
            "status = 'active'",
        ));
        let findings = check(&source);
        assert!(
            findings.is_empty(),
            "should be silent for managed column: {findings:?}"
        );
    }

    #[test]
    fn row_filter_references_unmanaged_column_fires() {
        let mut source = Catalog::empty();
        // Table only has 'id' — 'status' is not declared.
        source
            .tables
            .push(make_table_with_cols(qn("app", "orders"), &["id"]));
        source.publications.push(make_pub_with_filter(
            "p",
            qn("app", "orders"),
            "status = 'active'",
        ));
        let findings = check(&source);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("status"),
            "message should mention the column: {}",
            findings[0].message
        );
    }

    #[test]
    fn no_row_filter_silent() {
        let mut source = Catalog::empty();
        source
            .tables
            .push(make_table_with_cols(qn("app", "orders"), &["id", "status"]));
        // Publication without row filter.
        source.publications.push(Publication {
            name: id("p"),
            scope: PublicationScope::Selective {
                schemas: std::collections::BTreeSet::new(),
                tables: vec![PublishedTable {
                    qname: qn("app", "orders"),
                    row_filter: None,
                    columns: None,
                }],
            },
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        });
        assert!(check(&source).is_empty());
    }

    #[test]
    fn all_tables_scope_skipped() {
        // AllTables publications have no row filters at the publication level.
        let mut source = Catalog::empty();
        source
            .tables
            .push(make_table_with_cols(qn("app", "orders"), &["id", "status"]));
        source.publications.push(Publication {
            name: id("p"),
            scope: PublicationScope::AllTables,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        });
        assert!(check(&source).is_empty());
    }

    #[test]
    fn table_not_in_source_skipped() {
        let mut source = Catalog::empty();
        // Table not in source tables — skip.
        source.publications.push(make_pub_with_filter(
            "p",
            qn("app", "orders"),
            "status = 'active'",
        ));
        // No source table registered → rule should skip, not fire.
        assert!(check(&source).is_empty());
    }
}
