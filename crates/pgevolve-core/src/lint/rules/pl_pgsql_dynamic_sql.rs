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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;
    use crate::lint::test_helpers::{
        empty_arg_types, empty_tree, id, make_plpgsql_function, make_procedure, qn,
    };
    use crate::plan::edges::{DepEdge, DepSource, NodeId};

    #[test]
    fn pl_pgsql_dynamic_sql_fires_when_execute_without_directive() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Body contains EXECUTE but no AstDeclared dep edge.
        c.functions.push(make_plpgsql_function(
            "app",
            "dyn_fn",
            "BEGIN EXECUTE 'SELECT 1'; END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "dyn_fn"), empty_arg_types()),
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstExtracted, // NOT AstDeclared
            }],
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "pl-pgsql-dynamic-sql")
            .count();
        assert_eq!(count, 1, "expected one pl-pgsql-dynamic-sql finding");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "pl-pgsql-dynamic-sql")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn pl_pgsql_dynamic_sql_silent_when_directive_present() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Body has EXECUTE + an AstDeclared dep — should be silent.
        c.functions.push(make_plpgsql_function(
            "app",
            "dyn_fn_ok",
            "BEGIN EXECUTE 'SELECT 1'; END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "dyn_fn_ok"), empty_arg_types()),
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstDeclared,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert!(
            findings.iter().all(|f| f.rule != "pl-pgsql-dynamic-sql"),
            "pl-pgsql-dynamic-sql must not fire when directive present",
        );
    }

    #[test]
    fn pl_pgsql_dynamic_sql_fires_for_procedure_without_directive() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Procedure with EXECUTE but no AstDeclared dep.
        c.procedures.push(make_procedure(
            "app",
            "dyn_proc",
            "BEGIN EXECUTE 'DELETE FROM users'; END",
            false,
            vec![],
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "pl-pgsql-dynamic-sql")
            .count();
        assert_eq!(
            count, 1,
            "expected one pl-pgsql-dynamic-sql finding for procedure"
        );
    }
}
