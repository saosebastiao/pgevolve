//! `cargo xtask coverage [--check | --gaps]`
//!
//! Cross-checks `docs/spec/*.md` capability rows against fixture coverage in
//! `crates/pgevolve-conformance/tests/cases/`.
//!
//! Each capability row in `docs/spec/*.md` with status `Implemented` or
//! `Partial` and a `change_kinds: [...]` annotation is part of the required
//! coverage matrix. The xtask scans fixtures' `[meta].spec_refs` and
//! `[pg]` fields to determine which (object × change-kind × PG-major) cells
//! are covered.
//!
//! A fixture's `spec_refs` entry is a dotted path such as
//! `objects.column.add` where the first segment is the object family and the
//! last segment is the change-kind (e.g., `add`). The middle segments are
//! informational.

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;
use walkdir::WalkDir;

/// A capability row parsed from a `docs/spec/*.md` table.
#[derive(Debug, Clone)]
struct CapabilityRow {
    /// Object family identifier (first `|`-cell, backtick-stripped).
    object: String,
    /// Change-kinds parsed from `change_kinds: [...]` annotation.
    change_kinds: Vec<String>,
}

/// Mode for `cargo xtask coverage`.
#[derive(Debug, Clone, Copy)]
pub enum CoverageMode {
    /// Gate: fail if any required cell is uncovered.
    Check,
    /// List: print uncovered cells as a tab-separated authoring queue.
    Gaps,
}

/// Entry point: run the coverage check or gap report.
pub fn run(mode: CoverageMode, workspace_root: &Path) -> Result<()> {
    let rows = parse_spec_rows(workspace_root)?;
    eprintln!("coverage: parsed {} capability row(s) with change_kinds annotations", rows.len());

    let fixtures = scan_fixtures(workspace_root)?;
    eprintln!("coverage: scanned {} (object, change-kind, pg-major) fixture cell(s)", fixtures.len());

    let matrix = build_matrix(&rows, &fixtures);
    eprintln!("coverage: matrix has {} required cell(s)", matrix.len());

    match mode {
        CoverageMode::Check => check_matrix(&matrix),
        CoverageMode::Gaps => print_gaps(&matrix),
    }
}

/// Parse capability rows from `docs/spec/*.md`.
fn parse_spec_rows(workspace_root: &Path) -> Result<Vec<CapabilityRow>> {
    let spec_dir = workspace_root.join("docs/spec");
    let mut rows = Vec::new();

    for entry in WalkDir::new(&spec_dir).into_iter().filter_map(Result::ok) {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let content = std::fs::read_to_string(entry.path())?;
        for line in content.lines() {
            if !line.starts_with('|') {
                continue;
            }
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            // Table rows have at least 4 cells when split by |:
            // "", object, status, notes, ""
            if cells.len() < 4 {
                continue;
            }
            let object_cell = cells[1];
            let status_cell = cells[2];
            let desc_cell = cells[3];

            // Skip header separators and empty cells.
            if object_cell.is_empty() || object_cell.contains("---") {
                continue;
            }
            // Only include Implemented or Partial rows.
            if !(status_cell.contains("Implemented") || status_cell.contains("Partial")) {
                continue;
            }
            let change_kinds = parse_change_kinds_annotation(desc_cell);
            if change_kinds.is_empty() {
                continue;
            }
            rows.push(CapabilityRow {
                object: object_cell.trim_matches('`').to_string(),
                change_kinds,
            });
        }
    }
    Ok(rows)
}

/// Parse `change_kinds: [a, b, c]` from a table cell's description text.
fn parse_change_kinds_annotation(desc: &str) -> Vec<String> {
    let Some(start) = desc.find("change_kinds:") else {
        return Vec::new();
    };
    let after = &desc[start + "change_kinds:".len()..];
    let Some(open) = after.find('[') else {
        return Vec::new();
    };
    let close_rel = after[open..].find(']').unwrap_or(after.len() - open);
    let inner = &after[open + 1..open + close_rel];
    inner
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Scan all fixture directories and collect (object, change-kind, pg-major) cells.
///
/// Returns a map from `(object, change_kind, pg_major)` to the fixture directory path.
fn scan_fixtures(workspace_root: &Path) -> Result<BTreeMap<(String, String, u32), String>> {
    use pgevolve_conformance::fixture::Fixture;

    let cases_root = workspace_root.join("crates/pgevolve-conformance/tests/cases");
    let mut out = BTreeMap::new();

    for entry in WalkDir::new(&cases_root).into_iter().filter_map(Result::ok) {
        if entry.file_name() != "fixture.toml" {
            continue;
        }
        let Some(dir) = entry.path().parent() else {
            continue;
        };
        let Ok(fixture) = Fixture::load(dir) else {
            continue;
        };
        for sref in &fixture.meta.spec_refs {
            let segs: Vec<&str> = sref.split('.').collect();
            if segs.len() < 2 {
                continue;
            }
            let object = segs[0].to_string();
            let change_kind = segs[segs.len() - 1].to_string();
            for major in fixture.pg.min..=fixture.pg.max {
                let key_str = major.to_string();
                if fixture
                    .pg
                    .expect
                    .0
                    .get(&key_str)
                    .map(String::as_str)
                    == Some("skip")
                {
                    continue;
                }
                out.insert(
                    (object.clone(), change_kind.clone(), major),
                    dir.display().to_string(),
                );
            }
        }
    }
    Ok(out)
}

/// Build the full required coverage matrix and record which cells are covered.
///
/// Returns a map from `(object, change_kind, pg_major)` to `Some(fixture_path)` if
/// covered, `None` if uncovered.
fn build_matrix(
    rows: &[CapabilityRow],
    fixtures: &BTreeMap<(String, String, u32), String>,
) -> BTreeMap<(String, String, u32), Option<String>> {
    let mut matrix = BTreeMap::new();
    for row in rows {
        for change in &row.change_kinds {
            for major in [14u32, 15, 16, 17] {
                let key = (row.object.clone(), change.clone(), major);
                let cell = fixtures.get(&key).cloned();
                matrix.insert(key, cell);
            }
        }
    }
    matrix
}

/// Check mode: fail if any required cell is uncovered.
fn check_matrix(matrix: &BTreeMap<(String, String, u32), Option<String>>) -> Result<()> {
    let gaps: Vec<_> = matrix.iter().filter(|(_, v)| v.is_none()).collect();
    if gaps.is_empty() {
        println!("coverage: clean ({} cells covered)", matrix.len());
        return Ok(());
    }
    eprintln!(
        "coverage: {} gap(s) of {} required cell(s):",
        gaps.len(),
        matrix.len()
    );
    for ((obj, change, pg), _) in &gaps {
        eprintln!("  - {obj} / {change} on PG {pg}");
    }
    anyhow::bail!("coverage gaps detected ({} of {} cells uncovered)", gaps.len(), matrix.len())
}

/// Gaps mode: print uncovered cells as a TSV authoring queue.
fn print_gaps(matrix: &BTreeMap<(String, String, u32), Option<String>>) -> Result<()> {
    let mut count = 0;
    for ((obj, change, pg), v) in matrix {
        if v.is_none() {
            println!("{obj}\t{change}\tpg{pg}");
            count += 1;
        }
    }
    eprintln!("coverage: {count} gap(s) listed");
    Ok(())
}
