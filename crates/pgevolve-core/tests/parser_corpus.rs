//! Tier-2 parser fixture corpus.
//!
//! Walks `tests/fixtures/parser/{equivalent_pairs,different_pairs,parse_errors}`
//! and runs every fixture as a parameterized sub-case. Failures report which
//! fixture (and what within it) didn't match expectations.
//!
//! Layout — see `docs/superpowers/plans/phase-2-parser.md` task 2.13.

// Integration tests are separate compilation units; the crate-level allow
// doesn't propagate. See crates/pgevolve-core/src/lib.rs for rationale.
#![allow(clippy::result_large_err)]

use std::fs;
use std::path::{Path, PathBuf};

use pgevolve_core::ir::catalog::Catalog;
use pgevolve_core::ir::eq::Equiv;
use pgevolve_core::parse::{self, ParseError};

/// Parse a single `*.sql` file's contents into a `Catalog` by spinning up a
/// tempdir and calling [`parse::parse_directory`]. Mirrors what production
/// callers do.
fn parse_fixture(sql: &str) -> Result<Catalog, ParseError> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("fixture.sql");
    fs::write(&path, sql).expect("write fixture");
    parse::parse_directory(tmp.path(), &[])
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("parser")
}

/// Render a list of differences into one diagnostic blob: each line is
/// `path: from -> to` so we can substring-match in fixture expectations.
fn render_diffs(diffs: &[pgevolve_core::ir::difference::Difference]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for d in diffs {
        let _ = writeln!(s, "{}: {} -> {}", d.path, d.from, d.to);
    }
    s
}

fn read_expected_substrings(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("missing expected file: {}", path.display()))
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn equivalent_pairs() {
    let dir = fixtures_root().join("equivalent_pairs");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read equivalent_pairs") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let fixture = entry.path();
        let a_sql = fs::read_to_string(fixture.join("a.sql"))
            .unwrap_or_else(|_| panic!("read a.sql in {}", fixture.display()));
        let b_sql = fs::read_to_string(fixture.join("b.sql"))
            .unwrap_or_else(|_| panic!("read b.sql in {}", fixture.display()));

        let a = parse_fixture(&a_sql)
            .unwrap_or_else(|e| panic!("a.sql failed in {}: {e}", fixture.display()));
        let b = parse_fixture(&b_sql)
            .unwrap_or_else(|e| panic!("b.sql failed in {}: {e}", fixture.display()));

        if !a.canonical_eq(&b) {
            let diffs = a.differences(&b);
            panic!(
                "equivalent_pairs/{} produced different IR:\n{}",
                fixture.file_name().unwrap().to_string_lossy(),
                render_diffs(&diffs)
            );
        }
        count += 1;
    }
    assert!(count > 0, "no equivalent_pairs fixtures discovered");
}

#[test]
fn different_pairs() {
    let dir = fixtures_root().join("different_pairs");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read different_pairs") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let fixture = entry.path();
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let a_sql = fs::read_to_string(fixture.join("a.sql"))
            .unwrap_or_else(|_| panic!("read a.sql in {name}"));
        let b_sql = fs::read_to_string(fixture.join("b.sql"))
            .unwrap_or_else(|_| panic!("read b.sql in {name}"));
        let expected = read_expected_substrings(&fixture.join("expected.txt"));

        let a = parse_fixture(&a_sql).unwrap_or_else(|e| panic!("a.sql failed in {name}: {e}"));
        let b = parse_fixture(&b_sql).unwrap_or_else(|e| panic!("b.sql failed in {name}: {e}"));

        let diffs = a.differences(&b);
        assert!(
            !diffs.is_empty(),
            "different_pairs/{name} produced no diffs"
        );
        let rendered = render_diffs(&diffs);
        for needle in &expected {
            assert!(
                rendered.contains(needle),
                "different_pairs/{name}: missing substring {needle:?}\nactual diffs:\n{rendered}"
            );
        }
        count += 1;
    }
    assert!(count > 0, "no different_pairs fixtures discovered");
}

#[test]
fn parse_errors() {
    let dir = fixtures_root().join("parse_errors");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read parse_errors") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let expected_path = dir.join(format!("{stem}.expected.txt"));
        let expected = read_expected_substrings(&expected_path);
        let sql = fs::read_to_string(&path).unwrap();

        let err = parse_fixture(&sql).expect_err(&format!(
            "parse_errors/{stem} expected to fail but succeeded"
        ));
        let msg = err.to_string();
        for needle in &expected {
            assert!(
                msg.contains(needle),
                "parse_errors/{stem}: error message missing {needle:?}\nactual: {msg}"
            );
        }
        count += 1;
    }
    assert!(count > 0, "no parse_errors fixtures discovered");
}
