//! `pl-pgsql-dynamic-sql` lint rule.

use crate::ir::function::FunctionLanguage;
use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;
use crate::plan::edges::{DepEdge, DepSource};

/// `pl-pgsql-dynamic-sql` — fires (Error) when a PL/pgSQL function or
/// procedure body contains `EXECUTE` (dynamic SQL) but has no
/// `-- @pgevolve dep: <qname>` directive (`DepSource::AstDeclared` edge).
///
/// Dynamic SQL bypasses static analysis. Developers must annotate every
/// dynamic reference with a directive so pgevolve can maintain the dependency
/// graph correctly.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for f in &tree.catalog.functions {
        if !matches!(f.language, FunctionLanguage::PlPgSql) {
            continue;
        }
        check_dynamic_sql_in_routine(
            f.body.canonical_text(),
            &f.body_dependencies,
            &f.qname.to_string(),
            "function",
            &mut out,
        );
    }

    for p in &tree.catalog.procedures {
        if !matches!(p.language, FunctionLanguage::PlPgSql) {
            continue;
        }
        check_dynamic_sql_in_routine(
            p.body.canonical_text(),
            &p.body_dependencies,
            &p.qname.to_string(),
            "procedure",
            &mut out,
        );
    }

    out
}

/// Shared inner check for `pl-pgsql-dynamic-sql` — called for both functions
/// and procedures.
fn check_dynamic_sql_in_routine(
    body_text: &str,
    deps: &[DepEdge],
    label: &str,
    kind: &str,
    out: &mut Vec<Finding>,
) {
    let text_lower = body_text.to_lowercase();
    let has_dynamic = text_lower.contains("execute ") || text_lower.contains("execute(");
    if !has_dynamic {
        return;
    }
    let has_directive = deps
        .iter()
        .any(|d| matches!(d.source, DepSource::AstDeclared));
    if !has_directive {
        out.push(Finding::error(
            "pl-pgsql-dynamic-sql",
            format!(
                "{kind} `{label}` contains dynamic SQL (EXECUTE) but has no \
                 `-- @pgevolve dep: <qname>` directive. Add at least one directive \
                 to declare what the dynamic SQL references.",
            ),
        ));
    }
}
