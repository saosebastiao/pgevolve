//! `cargo xtask diagnose-pg-version <fixture-dir> --pg-major N`
//!
//! Runs the specified fixture against the requested PG major
//! (via `PGEVOLVE_TEST_PG_VERSION` if appropriate) and reports per-layer
//! outcomes plus suggested fixture.toml edits.

use anyhow::Result;
use std::path::Path;
use std::process::Command;

pub fn run(fixture_dir: &Path, pg_major: u32) -> Result<()> {
    if !fixture_dir.exists() {
        anyhow::bail!("fixture directory not found: {}", fixture_dir.display());
    }
    let fixture_name = fixture_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("bad fixture path"))?;

    println!("running fixture {fixture_name} against PG {pg_major}");
    let mut cmd = Command::new("cargo");
    cmd.args([
        "test",
        "-p",
        "pgevolve-conformance",
        "--test",
        "run",
        "--",
        fixture_name,
    ]);
    cmd.env("PGEVOLVE_TEST_PG_VERSION", pg_major.to_string());

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        println!("All layers passed on PG {pg_major}");
        return Ok(());
    }

    println!("FAIL on PG {pg_major}");
    println!("Failed output:");
    for line in stderr
        .lines()
        .rev()
        .take(40)
        .collect::<Vec<_>>()
        .iter()
        .rev()
    {
        println!("  {line}");
    }
    // stdout may also have relevant output
    if !stdout.is_empty() {
        for line in stdout.lines().rev().take(10).collect::<Vec<_>>().iter().rev() {
            println!("  {line}");
        }
    }
    println!();
    println!("Suggested fixture.toml edits:");
    println!("  - If L3 mismatch:    run `cargo xtask bless --conformance` to regenerate plan.sql goldens");
    println!(
        "  - If L2 step count:  add `[expect.plan.per_pg.pg{pg_major}] steps = N`"
    );
    println!(
        "  - If L4 apply fail:  add `[pg.expect]\\n\"{pg_major}\" = \"failure\"` with `[expect.failure]` block"
    );
    println!();
    println!(
        "Full output saved by cargo at target/debug/deps/run-*.stderr"
    );
    Ok(())
}
