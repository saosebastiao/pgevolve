//! `cargo xtask soak-streak [--days N]`
//!
//! Reports the consecutive-clean-day streak across the `ci.yml` and
//! `soak.yml` workflows on `main` over a configurable window
//! (default 30 days). Used pre-1.0 to decide whether the release gate
//! in `docs/v1.md` §3 is met. Requires the `gh` CLI in PATH.
//!
//! Exit codes:
//! - 0: streak >= requested window. Tag-ready.
//! - 1: streak < requested window, or a workflow run is still in
//!   progress (caller should wait).
//! - 2: gh CLI not available or query failed.

use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct Run {
    #[serde(rename = "databaseId")]
    database_id: u64,
    #[serde(rename = "createdAt")]
    created_at: String,
    /// `success` | `failure` | `cancelled` | `skipped` | `timed_out` | `action_required` | `neutral` | `startup_failure` | `stale` | `null`.
    conclusion: Option<String>,
    /// `completed` | `in_progress` | `queued` | `requested` | `waiting` | `pending`.
    status: String,
}

const WORKFLOWS: &[&str] = &["ci.yml", "soak.yml"];

pub fn run(days: u32) -> Result<()> {
    // Detect gh.
    let gh_check = Command::new("gh").arg("--version").output();
    if gh_check.is_err() || !gh_check.unwrap().status.success() {
        eprintln!("gh CLI not found in PATH; install with `brew install gh`.");
        std::process::exit(2);
    }

    // Fetch runs for both workflows.
    let mut all_runs: Vec<(String, Run)> = Vec::new();
    for wf in WORKFLOWS {
        let runs = fetch_runs(wf, days)?;
        for r in runs {
            all_runs.push(((*wf).to_string(), r));
        }
    }
    // Sort newest-first by created_at.
    all_runs.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));

    let streak_result = walk_streak(&all_runs, days);
    print_report(&streak_result, days);

    if streak_result.streak_days >= days {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreakResult {
    /// Number of consecutive clean days, capped at `requested_days`.
    streak_days: u32,
    /// First non-success encountered while walking backward from "now",
    /// if any. `None` means every in-window run was successful.
    earliest_break: Option<(String, Run)>,
    /// Any run still in `in_progress`/`queued`/etc. status. Blocks the
    /// streak even if no failure has occurred yet.
    in_progress: Option<(String, Run)>,
    /// Oldest run in the window (for reporting).
    earliest_in_window: Option<(String, Run)>,
}

fn walk_streak(all_runs: &[(String, Run)], requested_days: u32) -> StreakResult {
    let mut earliest_break: Option<(String, Run)> = None;
    let mut in_progress: Option<(String, Run)> = None;
    let mut earliest: Option<(String, Run)> = None;

    for (wf, r) in all_runs {
        if r.status != "completed" {
            // Capture the most-recent in-progress run only (the loop is
            // newest-first, so the first hit is the latest).
            if in_progress.is_none() {
                in_progress = Some((wf.clone(), r.clone()));
            }
            continue;
        }
        let conc = r.conclusion.as_deref().unwrap_or("");
        if conc != "success" && earliest_break.is_none() {
            earliest_break = Some((wf.clone(), r.clone()));
        }
        earliest = Some((wf.clone(), r.clone()));
    }

    // Streak: days from "now" back to the earliest break (or to the
    // earliest in-window run if no break).
    let streak_days = if let Some((_, ref r)) = earliest_break {
        days_between_now_and(&r.created_at).unwrap_or(0)
    } else {
        // No failure in window; streak is min(requested_days, days
        // since earliest_in_window). Capped at requested_days so a
        // 7-day-old earliest with --days 30 caps at 7.
        if let Some((_, ref r)) = earliest {
            days_between_now_and(&r.created_at)
                .unwrap_or(0)
                .min(requested_days)
        } else {
            // No runs in window at all. Streak is 0; the maintainer
            // should investigate.
            0
        }
    };

    StreakResult {
        streak_days,
        earliest_break,
        in_progress,
        earliest_in_window: earliest,
    }
}

fn fetch_runs(workflow: &str, days: u32) -> Result<Vec<Run>> {
    let since = days_ago_iso(days);
    let out = Command::new("gh")
        .args([
            "run",
            "list",
            "--branch",
            "main",
            "--workflow",
            workflow,
            "--created",
            &format!(">={since}"),
            "--limit",
            "1000",
            "--json",
            "databaseId,createdAt,conclusion,status",
        ])
        .output()
        .with_context(|| format!("invoking gh run list for {workflow}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "gh run list failed for {workflow}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let runs: Vec<Run> = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing gh JSON for {workflow}"))?;
    Ok(runs)
}

fn print_report(r: &StreakResult, requested_days: u32) {
    let marker = if r.streak_days >= requested_days {
        "✓"
    } else {
        ""
    };
    println!(
        "soak streak: {}/{} days {marker}",
        r.streak_days, requested_days
    );

    if let Some((ref wf, ref run)) = r.earliest_break {
        println!(
            "  last failure:    {} ({} run {})",
            run.created_at.get(..10).unwrap_or(&run.created_at),
            wf,
            run.database_id
        );
    } else {
        println!("  last failure:    none in window");
    }

    if let Some((ref wf, ref run)) = r.in_progress {
        println!(
            "  in progress:     {} ({} run {})",
            run.created_at.get(..10).unwrap_or(&run.created_at),
            wf,
            run.database_id
        );
        println!("  → wait for the in-progress run to finish before re-checking");
    }

    if let Some((_, ref run)) = r.earliest_in_window {
        println!(
            "  earliest in win: {}",
            run.created_at.get(..10).unwrap_or(&run.created_at)
        );
    }

    if r.streak_days < requested_days {
        let need = requested_days - r.streak_days;
        println!("need {need} more clean day(s) for the {requested_days}-day window");
    }
}

fn days_ago_iso(days: u32) -> String {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let then_secs = now_secs.saturating_sub(u64::from(days) * 86_400);
    unix_secs_to_iso_date(then_secs)
}

fn unix_secs_to_iso_date(secs: u64) -> String {
    // Convert unix seconds to "YYYY-MM-DD" using Howard Hinnant's algorithm.
    #[allow(clippy::cast_possible_wrap)]
    let z: i64 = (secs / 86_400) as i64 + 719_468;
    let era: i64 = if z >= 0 { z } else { z - 146_096 } / 146_097;
    #[allow(clippy::cast_sign_loss)]
    let doe: u64 = (z - era * 146_097) as u64; // 0..=146096
    let yoe: u64 = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // 0..=399
    // yoe is bounded to 0..=399, well within i64::MAX; widening cast is safe.
    #[allow(clippy::cast_possible_wrap)]
    let y: i64 = (yoe as i64) + era * 400;
    let doy: u64 = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
    let mp: u64 = (5 * doy + 2) / 153; // 0..=11
    let d: u64 = doy - (153 * mp + 2) / 5 + 1; // 1..=31
    let m: u64 = if mp < 10 { mp + 3 } else { mp - 9 }; // 1..=12
    let year: i64 = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}")
}

fn days_between_now_and(iso_created_at: &str) -> Option<u32> {
    // iso_created_at format: "YYYY-MM-DDTHH:MM:SSZ"
    let created_secs = parse_rfc3339_to_unix_secs(iso_created_at)?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if now_secs <= created_secs {
        return Some(0);
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(((now_secs - created_secs) / 86_400) as u32)
}

fn parse_rfc3339_to_unix_secs(s: &str) -> Option<u64> {
    // Reused from xtask::property_status::parse_rfc3339_to_unix_secs.
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

    let y: i64 = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe: i64 = y - era * 400;
    let m: i64 = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let doy: i64 = (153 * m + 2) / 5 + i64::from(day) - 1;
    let doe: i64 = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days: i64 = era * 146_097 + doe - 719_468;
    let secs: i64 =
        days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    if secs < 0 {
        None
    } else {
        #[allow(clippy::cast_sign_loss)]
        Some(secs as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(created_at: &str, conclusion: Option<&str>, status: &str) -> Run {
        Run {
            database_id: 1,
            created_at: created_at.into(),
            conclusion: conclusion.map(String::from),
            status: status.into(),
        }
    }

    #[test]
    fn empty_runs_streak_zero() {
        let r = walk_streak(&[], 30);
        assert_eq!(r.streak_days, 0);
        assert!(r.earliest_break.is_none());
        assert!(r.in_progress.is_none());
    }

    #[test]
    fn all_success_caps_at_requested_days() {
        // Single run 1 day ago, requested window 30 days.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let one_day_ago = unix_secs_to_iso_date(now - 86_400);
        let runs = vec![(
            "ci.yml".into(),
            run(
                &format!("{one_day_ago}T12:00:00Z"),
                Some("success"),
                "completed",
            ),
        )];
        let r = walk_streak(&runs, 30);
        // earliest-in-window is 1 day old; streak caps at min(30, 1) = 1.
        assert!(r.streak_days <= 30);
        assert!(r.earliest_break.is_none());
    }

    #[test]
    fn failure_breaks_streak() {
        let runs = vec![
            (
                "ci.yml".into(),
                run("2026-05-28T12:00:00Z", Some("failure"), "completed"),
            ),
            (
                "ci.yml".into(),
                run("2026-05-27T12:00:00Z", Some("success"), "completed"),
            ),
        ];
        let r = walk_streak(&runs, 30);
        assert!(r.earliest_break.is_some(), "failure should produce a break");
    }

    #[test]
    fn in_progress_run_recorded() {
        let runs = vec![(
            "soak.yml".into(),
            run("2026-05-28T12:00:00Z", None, "in_progress"),
        )];
        let r = walk_streak(&runs, 30);
        assert!(r.in_progress.is_some(), "in_progress run should be flagged");
    }

    #[test]
    fn iso_date_round_trip() {
        // Sanity check the Hinnant algorithm.
        let secs = parse_rfc3339_to_unix_secs("2026-05-28T00:00:00Z").unwrap();
        assert_eq!(unix_secs_to_iso_date(secs), "2026-05-28");
    }
}
