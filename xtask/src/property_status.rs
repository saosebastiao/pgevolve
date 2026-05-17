//! `cargo xtask property-status [--max-age-days N]`
//!
//! Lists open property-test issues from GitHub and fails if any
//! exceed the threshold. Requires the `gh` CLI to be in PATH.

use anyhow::Result;
use std::process::Command;

#[derive(serde::Deserialize)]
struct Issue {
    number: u64,
    title: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

pub fn run(max_age_days: u64) -> Result<()> {
    // Detect gh
    let gh_check = Command::new("gh").arg("--version").output();
    if gh_check.is_err() || !gh_check.unwrap().status.success() {
        eprintln!(
            "gh CLI not found; install with `brew install gh`. Skipping property-status check."
        );
        return Ok(());
    }

    let output = Command::new("gh")
        .args([
            "issue",
            "list",
            "--label",
            "property-test-failure",
            "--state",
            "open",
            "--json",
            "number,title,createdAt",
        ])
        .output()?;
    if !output.status.success() {
        eprintln!(
            "gh issue list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(()); // don't block CI on gh failures
    }

    let issues: Vec<Issue> = serde_json::from_slice(&output.stdout)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let mut stale = Vec::new();
    for i in &issues {
        // Parse RFC3339 timestamp without chrono — use a minimal parser.
        // Format: "2026-04-15T12:34:56Z"
        let created_secs = parse_rfc3339_to_unix_secs(&i.created_at).unwrap_or(0);
        let age_days = if created_secs == 0 {
            0
        } else {
            (now - created_secs) / 86_400
        };
        let status = if age_days > max_age_days {
            "STALE"
        } else {
            "ok   "
        };
        println!(
            "{status} #{:5} {} ({age_days} days old)",
            i.number, i.title,
        );
        if age_days > max_age_days {
            stale.push(i.number);
        }
    }

    if !stale.is_empty() {
        anyhow::bail!(
            "{} stale property-test issue(s) exceed {max_age_days}-day threshold: {stale:?}",
            stale.len(),
        );
    }
    Ok(())
}

fn parse_rfc3339_to_unix_secs(s: &str) -> Option<u64> {
    // Minimal parser: "YYYY-MM-DDTHH:MM:SSZ"
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i32 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: u32 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day: u32 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    let hour: u32 = std::str::from_utf8(&bytes[11..13]).ok()?.parse().ok()?;
    let minute: u32 = std::str::from_utf8(&bytes[14..16]).ok()?.parse().ok()?;
    let second: u32 = std::str::from_utf8(&bytes[17..19]).ok()?.parse().ok()?;

    // Days from 1970-01-01 to (year, month, day) using Howard Hinnant's algorithm.
    // All intermediate arithmetic is done in i64 to avoid sign-loss warnings.
    let y: i64 = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe: i64 = y - era * 400; // 0..=399
    let m: i64 = i64::from(if month > 2 { month - 3 } else { month + 9 }); // 0..=11
    let doy: i64 = (153 * m + 2) / 5 + i64::from(day) - 1; // 0..=365
    let doe: i64 = yoe * 365 + yoe / 4 - yoe / 100 + doy; // 0..=146096
    let days: i64 = era * 146_097 + doe - 719_468;
    let secs: i64 = days * 86_400
        + i64::from(hour) * 3_600
        + i64::from(minute) * 60
        + i64::from(second);
    if secs < 0 {
        None
    } else {
        // secs is non-negative, cast is safe
        #[allow(clippy::cast_sign_loss)]
        Some(secs as u64)
    }
}
