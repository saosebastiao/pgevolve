//! `TABLESPACE` IR — a cluster-level object (like `Role`). Only the SQL object
//! is modeled; filesystem layout is out of scope.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};

/// Normalize a tablespace `LOCATION` path so that source and live catalog
/// always agree even when a trailing slash is present in the source SQL.
///
/// `pg_tablespace_location()` never returns a trailing slash, so a source
/// `LOCATION '/data/ts/'` would forever mismatch the live `/data/ts`.
///
/// Rules:
/// - Trailing `/` characters are stripped.
/// - The root path `"/"` (and any string that reduces to empty after stripping)
///   is preserved as `"/"`.
/// - An empty string is returned as-is (the parser rejects empty locations
///   before normalizing, but we handle it for symmetry).
#[must_use]
pub(crate) fn normalize_location(s: &str) -> String {
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        // Either the string was empty or it was all slashes (e.g. "/" or "///").
        // Preserve a single "/" for root; empty stays empty.
        if s.is_empty() {
            String::new()
        } else {
            "/".to_string()
        }
    } else {
        trimmed.to_string()
    }
}

/// A `CREATE TABLESPACE` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tablespace {
    /// Cluster-global tablespace name.
    pub name: Identifier,
    /// `LOCATION '/path'` directory. Immutable in PG — a change surfaces as a
    /// lint, never an ALTER.
    pub location: String,
    /// Owner (`pg_tablespace.spcowner`). Lenient: `None` = unmanaged.
    pub owner: Option<Identifier>,
    /// Tablespace options (`seq_page_cost`, `random_page_cost`,
    /// `effective_io_concurrency`, `maintenance_io_concurrency`). Lenient.
    pub options: BTreeMap<String, String>,
    /// Optional comment (`pg_shdescription`).
    pub comment: Option<String>,
}

impl Diff for Tablespace {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let Self {
            name: _,
            location: _,
            owner: _,
            options: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(diff_field("name", &self.name, &other.name));
        out.extend(diff_field(
            "location",
            &format!("{:?}", self.location),
            &format!("{:?}", other.location),
        ));
        out.extend(diff_field(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(diff_field(
            "options",
            &format!("{:?}", self.options),
            &format!("{:?}", other.options),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

    // --- normalize_location ---

    #[test]
    fn normalize_location_strips_single_trailing_slash() {
        assert_eq!(normalize_location("/data/ts/"), "/data/ts");
    }

    #[test]
    fn normalize_location_strips_multiple_trailing_slashes() {
        assert_eq!(normalize_location("/data/ts///"), "/data/ts");
    }

    #[test]
    fn normalize_location_no_trailing_slash_unchanged() {
        assert_eq!(normalize_location("/data/ts"), "/data/ts");
    }

    #[test]
    fn normalize_location_root_preserved() {
        assert_eq!(normalize_location("/"), "/");
    }

    #[test]
    fn normalize_location_empty_preserved() {
        assert_eq!(normalize_location(""), "");
    }

    // --- Tablespace diff ---

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn base() -> Tablespace {
        Tablespace {
            name: id("fast_ssd"),
            location: "/mnt/ssd".to_string(),
            owner: None,
            options: BTreeMap::new(),
            comment: None,
        }
    }

    #[test]
    fn equal_tablespaces_have_no_diff() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn owner_change_diffs() {
        let mut b = base();
        b.owner = Some(id("dba"));
        assert!(base().diff(&b).iter().any(|x| x.path == "owner"));
    }

    #[test]
    fn options_change_diffs() {
        let mut b = base();
        b.options
            .insert("seq_page_cost".to_string(), "1.5".to_string());
        assert!(base().diff(&b).iter().any(|x| x.path == "options"));
    }

    #[test]
    fn location_change_diffs() {
        let mut b = base();
        b.location = "/mnt/nvme".to_string();
        assert!(base().diff(&b).iter().any(|x| x.path == "location"));
    }

    #[test]
    fn comment_change_diffs() {
        let mut b = base();
        b.comment = Some("fast storage".into());
        assert!(base().diff(&b).iter().any(|x| x.path == "comment"));
    }
}
