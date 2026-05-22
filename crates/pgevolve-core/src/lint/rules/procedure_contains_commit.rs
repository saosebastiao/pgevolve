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
