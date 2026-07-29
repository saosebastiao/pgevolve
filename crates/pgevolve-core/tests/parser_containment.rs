//! The SQL parser binding stays inside `src/parse/`.
//!
//! Stage 2 of the parser-ownership plan moved every parser walk into
//! `parse::from_catalog` and `parse::syntax` and sealed `parse`'s submodules to
//! `pub(crate)`. Nothing mechanically stopped the next `use pg_query::…` from
//! landing in `catalog/assemble/` again, and the containment is what makes
//! swapping the binding a change to one directory instead of a crate-wide
//! migration — so this test is the thing that holds it.
//!
//! It is deliberately a source scan rather than a visibility rule: the parser is
//! an ordinary crate dependency, so any module in `pgevolve-core` *can* import
//! it. The type system cannot express "only this directory", but a test can.

use std::path::{Path, PathBuf};

/// The parser crate, spelled so this file does not trip its own check.
///
/// Moved with the Stage 4 cutover from `pg_query` to `pgevolve-pgquery`. It has
/// to move *with* the rename rather than after it: this test is what keeps the
/// containment from silently regressing, and a stale constant would make it pass
/// while checking for a crate nothing imports any more.
const PARSER_CRATE: &str = concat!("pgevolve", "_pgquery");

/// Directory that owns the parser binding, relative to `src/`.
const OWNING_DIR: &str = "parse";

fn rust_sources(root: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(root).unwrap_or_else(|e| panic!("read {}: {e}", root.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn parser_is_named_only_inside_the_parse_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let owning = src.join(OWNING_DIR);

    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    files.sort();
    assert!(
        files.len() > 50,
        "source scan found only {} files — the walk is broken, not the codebase",
        files.len()
    );

    let mut offenders: Vec<String> = Vec::new();
    for path in files.iter().filter(|p| !p.starts_with(&owning)) {
        let contents = std::fs::read_to_string(path).expect("read source");
        for (lineno, line) in contents.lines().enumerate() {
            if line.contains(PARSER_CRATE) {
                let rel = path.strip_prefix(&src).unwrap_or(path);
                offenders.push(format!(
                    "  {}:{}: {}",
                    rel.display(),
                    lineno + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "the parser crate is named outside src/{OWNING_DIR}/:\n{}\n\n\
         Route the work through `parse::from_catalog` (rebuilding IR from \
         server-emitted definition text) or `parse::syntax` (syntax-only \
         checks), adding a helper there if none fits. Prose counts too — say \
         \"the parser\" rather than naming the crate, so this check can stay a \
         plain substring scan.",
        offenders.join("\n")
    );
}

#[test]
fn the_owning_directory_actually_uses_the_parser() {
    // Guards the guard: if `parse/` stopped naming the parser, the test above
    // would pass vacuously and prove nothing.
    let owning = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(OWNING_DIR);
    let mut files = Vec::new();
    rust_sources(&owning, &mut files);

    let hits = files
        .iter()
        .filter(|p| {
            std::fs::read_to_string(p)
                .expect("read source")
                .contains(PARSER_CRATE)
        })
        .count();

    assert!(
        hits > 0,
        "no file under src/{OWNING_DIR}/ names the parser — the containment \
         test above is passing vacuously"
    );
}
