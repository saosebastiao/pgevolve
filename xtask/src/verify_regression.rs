//! `cargo xtask verify-regression <fixture-dir>`
//!
//! Runs the specified fixture through the conformance suite.
//! Asserts the fixture fails (i.e., the bug it captures is actually
//! present on the current branch). If the fixture passes, the bug
//! has already been fixed and the capture wasn't necessary.

use anyhow::Result;
use std::path::Path;
use std::process::Command;

pub fn run(fixture_dir: &Path) -> Result<()> {
    if !fixture_dir.exists() {
        anyhow::bail!("fixture directory not found: {}", fixture_dir.display());
    }
    let fixture_name = fixture_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("bad fixture path"))?;

    println!("running fixture {fixture_name} through conformance suite...");
    let output = Command::new("cargo")
        .args([
            "test",
            "-p",
            "pgevolve-conformance",
            "--test",
            "run",
            "--",
            fixture_name,
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        anyhow::bail!(
            "fixture {} PASSES; cannot capture as regression. \
             Either the bug is already fixed, or the fixture doesn't exercise it.\n\
             stdout:\n{stdout}\nstderr:\n{stderr}",
            fixture_dir.display(),
        );
    }

    println!("verified: fixture fails as expected on current branch");
    println!("output excerpt:");
    for line in stderr.lines().take(20) {
        println!("  {line}");
    }
    Ok(())
}
