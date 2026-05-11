//! Layer 1: diff invariants.
//!
//! Asserts that the diff between `before.sql` and `after.sql` contains
//! every substring listed in `fixture.expect.diff.contains`. Uses the
//! same `path: from -> to` rendering as the parser corpus
//! (`crates/pgevolve-core/tests/parser_corpus.rs`) so fixture authors
//! can transfer the syntax directly.

use std::fmt::Write as _;

use pgevolve_core::ir::difference::Difference;
use pgevolve_core::ir::eq::Diff;

use crate::fixture::Fixture;
use crate::planning::{PipelineError, parse_sql};

/// Result of running the diff assertion.
#[derive(Debug)]
pub struct DiffOutcome {
    /// Rendered diff (one entry per line).
    pub rendered: String,
    /// Substrings that were not found.
    pub missing: Vec<String>,
}

impl DiffOutcome {
    /// True when every expected substring was found.
    pub const fn is_ok(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Render the diff between `before` and `after` and check each
/// `expect.diff.contains` entry.
pub fn check(fixture: &Fixture) -> Result<DiffOutcome, PipelineError> {
    let target = parse_sql(&fixture.before_sql, "before")?;
    let source = parse_sql(&fixture.after_sql, "after")?;
    let differences: Vec<Difference> = target.diff(&source);
    let rendered = render(&differences);
    let missing = fixture
        .expect
        .diff
        .contains
        .iter()
        .filter(|needle| !rendered.contains(needle.as_str()))
        .cloned()
        .collect();
    Ok(DiffOutcome { rendered, missing })
}

fn render(diffs: &[Difference]) -> String {
    let mut s = String::new();
    for d in diffs {
        let _ = writeln!(s, "{}: {} -> {}", d.path, d.from, d.to);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{
        ExpectApply, ExpectDiff, ExpectPlan, FixtureExpect, FixtureMeta, FixturePassthrough,
        FixturePg,
    };
    use std::path::PathBuf;

    fn fixture(before: &str, after: &str, expect_contains: Vec<&str>) -> Fixture {
        Fixture {
            dir: PathBuf::from("/dev/null"),
            before_sql: before.to_string(),
            after_sql: after.to_string(),
            meta: FixtureMeta {
                title: "test".into(),
                spec_refs: vec![],
                issue: None,
            },
            pg: FixturePg::default(),
            passthrough: FixturePassthrough::default(),
            expect: FixtureExpect {
                diff: ExpectDiff {
                    contains: expect_contains.into_iter().map(str::to_string).collect(),
                },
                plan: ExpectPlan::default(),
                apply: ExpectApply::default(),
            },
        }
    }

    #[test]
    fn passes_when_expected_substring_appears() {
        let f = fixture(
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL);\n",
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\nCREATE TABLE app.t (id bigint NOT NULL, name text);\n",
            vec!["app.t"],
        );
        let out = check(&f).unwrap();
        assert!(out.is_ok(), "expected match; rendered:\n{}", out.rendered);
    }

    #[test]
    fn reports_missing_substrings() {
        let f = fixture(
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
            vec!["this.will.not.match"],
        );
        let out = check(&f).unwrap();
        assert!(!out.is_ok());
        assert_eq!(out.missing, vec!["this.will.not.match"]);
    }
}
