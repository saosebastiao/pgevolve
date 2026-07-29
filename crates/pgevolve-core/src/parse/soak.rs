//! Long-lived parse+deparse soak test.
//!
//! The multiversion feasibility spec (§14) records an **unreproduced** segfault
//! at roughly statement 6,300 when a single process parsed a large corpus
//! against PG14/15/16 builds of `libpg_query`, where the PG17/18 builds did not
//! crash. Statement-by-statement bisection found no individual reproducer, so
//! whatever it was is cumulative — most likely an allocation-pattern artefact of
//! the ad-hoc harness that observed it. The spec is explicit that it must not be
//! cited as a `libpg_query` defect on the evidence available.
//!
//! "Probably the harness" is not a thing to carry into a parser migration
//! unexamined, though: if it *is* real, it is a crash in the component every
//! other component depends on, and it would surface as a mysterious CI failure
//! at some unrelated later date.
//!
//! # What a green run here does and does not establish
//!
//! Read the scope before citing this test as evidence. The crate pgevolve
//! currently links is the **PG17** build of `libpg_query` — which is one of the
//! builds the spec says did *not* crash. So a green run:
//!
//! - **does** establish that the PG17 build survives 50k statements in one
//!   process, that the harness reaches the volume it claims, and that the
//!   corpus yields a stable statement count (2,199 per pass, matching the
//!   figure the spec's own experiment recorded);
//! - **does not** rule out the PG14/15/16 behaviour, because those builds are
//!   not linked here and cannot be without a per-major binding — the very
//!   architecture the spec rejected;
//! - **does not** rule it out for the vendored PG18 build either. Re-run this
//!   test after the cutover; that is when it becomes evidence about the binding
//!   pgevolve actually ships.
//!
//! The soak is `#[ignore]`d because it takes far longer than a unit test should.
//! Run it explicitly:
//!
//! ```sh
//! cargo test -p pgevolve-core --lib -- --ignored --nocapture soak
//! ```

use std::path::{Path, PathBuf};

/// Statements to push through one process before declaring the soak clean.
///
/// An order of magnitude past the ~6,300 mark the spec recorded, and past the
/// 50,970 of the experiment that chose the single-binding architecture.
const TARGET_STATEMENTS: usize = 50_000;

/// Collect every `*.sql` file in the repository, sorted for determinism.
fn corpus_files() -> Vec<PathBuf> {
    // CARGO_MANIFEST_DIR is crates/pgevolve-core; the corpora live across
    // crates/*/tests and crates/pgevolve-conformance/fixtures.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root is two levels above the crate")
        .to_path_buf();

    let mut files = Vec::new();
    collect_sql(&repo_root.join("crates"), &mut files);
    files.sort();
    files
}

fn collect_sql(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sql(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            out.push(path);
        }
    }
}

/// One pass over the corpus: parse each file, then deparse what parsed.
///
/// Counts statements that survived a full parse→deparse round trip, plus
/// failures of each kind.
///
/// Every file in the corpus is valid SQL, including the `parse_errors` parser
/// fixtures — those exercise pgevolve's *builder* rejecting a construct
/// (unsupported object kind, unqualified name), which happens well after the
/// parser has accepted the text. So the observed failure count is zero, and the
/// caller asserts on that rather than tolerating it: a failure appearing on pass
/// 17 of the same bytes that parsed on pass 1 is precisely the cumulative
/// corruption this test is looking for.
fn soak_pass(files: &[PathBuf], stats: &mut SoakStats) {
    for path in files {
        let Ok(sql) = std::fs::read_to_string(path) else {
            continue;
        };
        match pg_query::parse(&sql) {
            Ok(parsed) => {
                stats.statements += parsed.protobuf.stmts.len();
                // Deparse is the other half of the round trip and allocates far
                // more than parsing does; a leak or corruption that only shows
                // up under volume is likelier to show up here.
                match pg_query::deparse(&parsed.protobuf) {
                    Ok(text) => stats.deparsed_bytes += text.len(),
                    Err(_) => stats.deparse_failures += 1,
                }
            }
            Err(_) => stats.parse_failures += 1,
        }
    }
}

#[derive(Default, Debug)]
struct SoakStats {
    statements: usize,
    parse_failures: usize,
    deparse_failures: usize,
    deparsed_bytes: usize,
}

#[test]
#[ignore = "soak test: pushes 50k statements through one process, takes minutes"]
fn parse_deparse_soak_survives_fifty_thousand_statements() {
    let files = corpus_files();
    assert!(
        files.len() > 100,
        "corpus walk found only {} SQL files — the walk is broken, not the repo",
        files.len()
    );

    let mut stats = SoakStats::default();
    let mut passes = 0usize;
    while stats.statements < TARGET_STATEMENTS {
        soak_pass(&files, &mut stats);
        passes += 1;
        // A pass that yields nothing would loop forever; fail loudly instead.
        assert!(
            stats.statements > 0,
            "first pass over {} files parsed zero statements",
            files.len()
        );
        assert!(
            passes < 10_000,
            "after {passes} passes only {} statements — corpus too small to reach the target",
            stats.statements
        );
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "human-readable progress figure, not a computation"
    )]
    let deparsed_mib = stats.deparsed_bytes as f64 / (1024.0 * 1024.0);

    println!(
        "soak: {} statements over {passes} passes of {} files \
         ({} parse failures, {} deparse failures, {deparsed_mib:.1} MiB deparsed)",
        stats.statements,
        files.len(),
        stats.parse_failures,
        stats.deparse_failures,
    );

    // Reaching here at all is the primary assertion: the recorded failure mode
    // was a segfault, which no `assert!` can catch.
    assert!(
        stats.statements >= TARGET_STATEMENTS,
        "soak did not reach its target"
    );

    // The rest is so a *silent* degradation cannot masquerade as a pass. A run
    // that quietly started failing partway through would still survive to the
    // end, and "no crash" would be a misleading way to report it.
    assert_eq!(
        stats.parse_failures, 0,
        "every corpus file is valid SQL, so any parse failure across {passes} \
         identical passes means state is leaking between parses"
    );
    assert_eq!(stats.deparse_failures, 0, "deparse failed under volume");

    // Deparse returning empty strings would zero out the allocation pressure
    // this test exists to apply, while still reporting success. Observed ratio
    // is ~50 bytes/statement across the DDL corpus; 10 is a floor, not a target.
    assert!(
        stats.deparsed_bytes > stats.statements * 10,
        "only {} bytes deparsed for {} statements — deparse is returning \
         near-empty output, so the soak is not exercising what it claims to",
        stats.deparsed_bytes,
        stats.statements
    );
}
