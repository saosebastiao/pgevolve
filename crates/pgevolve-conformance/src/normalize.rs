//! Normalize a rendered `plan.sql` for golden comparison.
//!
//! Strips fields whose values are intentionally non-deterministic across
//! runs even when planner *logic* is byte-stable. Currently the only
//! such field is the `created=<rfc3339>` timestamp in the plan header
//! line (`-- @pgevolve plan ...`). The normalizer is intentionally
//! conservative: it only touches well-known header tokens, leaving
//! every SQL line untouched.

/// Apply all normalization passes. Idempotent.
pub fn normalize(plan_sql: &str) -> String {
    plan_sql
        .lines()
        .map(normalize_line)
        .collect::<Vec<_>>()
        .join("\n")
        // `lines()` does not emit a trailing empty element when the
        // input ends with a single LF, so re-add it to preserve the
        // final newline `write_plan_sql` emits.
        + if plan_sql.ends_with('\n') { "\n" } else { "" }
}

fn normalize_line(line: &str) -> String {
    if !line.starts_with("-- @pgevolve plan ") {
        return line.to_string();
    }
    strip_token(line, " created=")
}

/// Remove `<prefix><value>` from `line`, where `<value>` is the run of
/// non-space characters immediately after `<prefix>`. Returns the line
/// unchanged when the prefix is absent.
fn strip_token(line: &str, prefix: &str) -> String {
    line.find(prefix).map_or_else(
        || line.to_string(),
        |start| {
            let after = &line[start + prefix.len()..];
            let end = after.find(' ').unwrap_or(after.len());
            format!("{}{}", &line[..start], &after[end..])
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_created_timestamp() {
        let s = "-- @pgevolve plan id=abc created=2026-05-11T12:00:00Z foo=bar\nSELECT 1;\n";
        let n = normalize(s);
        assert_eq!(
            n, "-- @pgevolve plan id=abc foo=bar\nSELECT 1;\n",
            "only the created= token is stripped"
        );
    }

    #[test]
    fn handles_created_at_end_of_line() {
        let s = "-- @pgevolve plan id=abc created=2026-05-11T12:00:00Z\nSELECT 1;\n";
        let n = normalize(s);
        assert_eq!(n, "-- @pgevolve plan id=abc\nSELECT 1;\n");
    }

    #[test]
    fn leaves_lines_without_plan_prefix_alone() {
        let s = "-- some other comment created=2026-05-11\nCREATE TABLE t (id int);\n";
        assert_eq!(normalize(s), s);
    }

    #[test]
    fn idempotent() {
        let s = "-- @pgevolve plan id=abc created=2026-05-11T12:00:00Z\nSELECT 1;\n";
        let once = normalize(s);
        let twice = normalize(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_trailing_newline() {
        let with_lf = "-- @pgevolve plan id=abc created=now\n";
        assert!(normalize(with_lf).ends_with('\n'));
        let no_lf = "-- @pgevolve plan id=abc created=now";
        assert!(!normalize(no_lf).ends_with('\n'));
    }

    #[test]
    fn empty_input() {
        assert_eq!(normalize(""), "");
    }
}
