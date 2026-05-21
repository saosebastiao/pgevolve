//! `cargo xtask capture-regression --seed <hex> --issue <n>`
//!
//! Scaffolds a regression fixture from a proptest seed. The fixture
//! body uses placeholder SQL when the seed→IR replay mechanism in
//! testkit isn't yet wired; the maintainer fills in the actual
//! before/after SQL by reproducing the proptest failure locally.

use anyhow::Result;
use std::path::PathBuf;

pub fn run(seed_hex: &str, issue: u64) -> Result<()> {
    let slug = format!("issue-{issue}");
    let fixture_dir =
        PathBuf::from("crates/pgevolve-conformance/tests/cases/regressions").join(&slug);
    if fixture_dir.exists() {
        anyhow::bail!(
            "fixture already exists at {}; remove it first or pick a different issue number",
            fixture_dir.display()
        );
    }
    std::fs::create_dir_all(&fixture_dir)?;
    std::fs::create_dir_all(fixture_dir.join("expected"))?;

    let placeholder_before = "-- @pgevolve schema=app\n\
                              CREATE SCHEMA app;\n\
                              -- replace this with the minimized before-IR from the proptest seed.\n";
    let placeholder_after = "-- @pgevolve schema=app\n\
                             CREATE SCHEMA app;\n\
                             -- replace this with the minimized after-IR from the proptest seed.\n";

    std::fs::write(fixture_dir.join("before.sql"), placeholder_before)?;
    std::fs::write(fixture_dir.join("after.sql"), placeholder_after)?;
    std::fs::write(fixture_dir.join("expected/diff.txt"), "")?;

    let fixture_toml = format!(
        r#"[meta]
title = "regression: issue {issue}"
authoring = "regressions"
issue = "https://github.com/saosebastiao/pgevolve/issues/{issue}"
spec_refs = []

[pg]
min = 14
max = 17

[expect.diff]
contains = []

[expect.plan]
steps = 0
minimality = false

[expect.dep_graph]
enabled = false

# CAPTURED FROM PROPTEST SEED: {seed_hex}
# Replay locally with: cargo test -p pgevolve-core --test property_tests --release -- --ignored
# then edit before.sql and after.sql with the failing IR pair.
"#
    );
    std::fs::write(fixture_dir.join("fixture.toml"), fixture_toml)?;

    println!("scaffolded {}", fixture_dir.display());
    println!("Next steps:");
    println!("  1. Reproduce the proptest failure with seed {seed_hex}.");
    println!(
        "  2. Edit {}/before.sql and after.sql with the minimized IR pair.",
        fixture_dir.display()
    );
    println!(
        "  3. Run `cargo xtask verify-regression {}` to confirm the fixture fails as expected.",
        fixture_dir.display()
    );
    Ok(())
}
