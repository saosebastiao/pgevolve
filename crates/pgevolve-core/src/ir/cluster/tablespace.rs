//! `TABLESPACE` IR — a cluster-level object (like `Role`). Only the SQL object
//! is modeled; filesystem layout is out of scope.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::eq::DiffMacro;

/// A `CREATE TABLESPACE` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Tablespace {
    /// Cluster-global tablespace name.
    pub name: Identifier,
    /// `LOCATION '/path'` directory. Immutable in PG — a change surfaces as a
    /// lint, never an ALTER.
    #[diff(via_debug)]
    pub location: String,
    /// Owner (`pg_tablespace.spcowner`). Lenient: `None` = unmanaged.
    #[diff(via_debug)]
    pub owner: Option<Identifier>,
    /// Tablespace options (`seq_page_cost`, `random_page_cost`,
    /// `effective_io_concurrency`, `maintenance_io_concurrency`). Lenient.
    #[diff(via_debug)]
    pub options: BTreeMap<String, String>,
    /// Optional comment (`pg_shdescription`).
    #[diff(via_debug)]
    pub comment: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

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
