//! `procedure-contains-commit` lint rule.

use crate::lint::finding::Finding;
use crate::lint::source_tree::SourceTree;

/// `procedure-contains-commit` — fires (Warning) when a procedure's
/// `commits_in_body` flag is true. Informational: pgevolve will run the step
/// with `transactional=OutsideTransaction`. Surfaces in code review so
/// reviewers know the step cannot be rolled back as part of a larger migration
/// transaction.
pub fn check(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for p in &tree.catalog.procedures {
        if p.commits_in_body {
            out.push(Finding::warning(
                "procedure-contains-commit",
                format!(
                    "procedure `{}` body contains COMMIT/ROLLBACK; pgevolve will run \
                     this step with transactional=OutsideTransaction.",
                    p.qname,
                ),
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;
    use crate::lint::test_helpers::{empty_tree, id, make_procedure};

    #[test]
    fn procedure_contains_commit_fires_when_commits_in_body_true() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.procedures.push(make_procedure(
            "app",
            "commit_proc",
            "BEGIN COMMIT; END",
            true, // commits_in_body
            vec![],
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        let count = findings
            .iter()
            .filter(|f| f.rule == "procedure-contains-commit")
            .count();
        assert_eq!(count, 1, "expected one procedure-contains-commit warning");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "procedure-contains-commit")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn procedure_contains_commit_silent_when_false() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.procedures.push(make_procedure(
            "app",
            "normal_proc",
            "BEGIN NULL; END",
            false, // no COMMIT
            vec![],
        ));
        let tree = empty_tree(c);
        let findings = check(&tree);
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "procedure-contains-commit"),
            "procedure-contains-commit must not fire when commits_in_body=false",
        );
    }
}
