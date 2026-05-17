//! `cargo xtask fixture-cost` — per-fixture timing report.

use anyhow::Result;
use std::path::Path;

pub fn run() -> Result<()> {
    let path = Path::new("target/conformance-timings.tsv");
    if !path.exists() {
        anyhow::bail!(
            "no timings file at {}; run `cargo test -p pgevolve-conformance` first to generate it",
            path.display(),
        );
    }
    let content = std::fs::read_to_string(path)?;
    let mut rows: Vec<(String, f64)> = content
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(2, '\t');
            let dir = parts.next()?;
            let secs: f64 = parts.next()?.parse().ok()?;
            Some((dir.to_string(), secs))
        })
        .collect();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    println!("top fixtures by wall-clock:");
    for (dir, secs) in rows.iter().take(20) {
        println!("  {secs:>6.2}s  {dir}");
    }
    let total: f64 = rows.iter().map(|r| r.1).sum();
    println!("---");
    println!("  {total:>6.2}s  total over {} fixtures", rows.len());
    Ok(())
}
