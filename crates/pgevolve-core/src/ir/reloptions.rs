//! Storage parameters / reloptions for Table, Index, `MaterializedView`.
//!
//! Typed fields for well-known keys + `extra: BTreeMap<String, String>` for
//! extension-registered or otherwise-unknown options. The `autovacuum_*` key
//! family is routed through `extra` as raw strings rather than typed fields:
//! the key set is large and rarely diffed, so the generic map keeps the IR
//! surface small while still round-tripping every key faithfully.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Storage options for tables. MV reuses via type alias since PG documents
/// identical reloptions for both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct TableStorageOptions {
    /// `fillfactor` — target heap page density (10..=100).
    pub fillfactor: Option<u32>,
    /// `parallel_workers` — number of parallel workers (0..=1024).
    pub parallel_workers: Option<u32>,
    /// `toast_tuple_target` — TOAST compression threshold in bytes (128..=8160).
    pub toast_tuple_target: Option<u32>,
    /// `user_catalog_table` — treat as a catalog table for logical replication.
    pub user_catalog_table: Option<bool>,
    /// `vacuum_truncate` — allow VACUUM to truncate trailing empty pages (PG 12+).
    pub vacuum_truncate: Option<bool>,
    /// Unknown / extension-registered options, plus the `autovacuum_*` key
    /// family. Stored as raw `key = value` strings, always sorted by key
    /// (`BTreeMap`).
    pub extra: BTreeMap<String, String>,
}

impl TableStorageOptions {
    /// `true` iff every typed field is `None` and `extra` is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fillfactor.is_none()
            && self.parallel_workers.is_none()
            && self.toast_tuple_target.is_none()
            && self.user_catalog_table.is_none()
            && self.vacuum_truncate.is_none()
            && self.extra.is_empty()
    }
}

/// MVs share table reloption semantics in PG.
pub type MaterializedViewStorageOptions = TableStorageOptions;

/// Storage options for indexes. Valid keys depend on access method;
/// parse-time validation enforces per-AM rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct IndexStorageOptions {
    /// `fillfactor` — target index page density; valid range depends on access method.
    pub fillfactor: Option<u32>,
    /// `fastupdate` — GIN: defer index updates via a pending list.
    pub fastupdate: Option<bool>,
    /// `gin_pending_list_limit` — GIN: max size of pending list before cleanup (bytes).
    pub gin_pending_list_limit: Option<u64>,
    /// `buffering` — `GiST` / `SP-GiST`: controls buffered build strategy.
    pub buffering: Option<BufferingMode>,
    /// `deduplicate_items` — B-tree (PG 13+): enable deduplication of posting lists.
    pub deduplicate_items: Option<bool>,
    /// `pages_per_range` — BRIN: number of heap pages per BRIN range.
    pub pages_per_range: Option<u32>,
    /// `autosummarize` — BRIN: auto-summarize when `pages_per_range` fills.
    pub autosummarize: Option<bool>,
    /// Unknown / extension-registered options. Always sorted by key (`BTreeMap`).
    pub extra: BTreeMap<String, String>,
}

impl IndexStorageOptions {
    /// `true` iff every typed field is `None` and `extra` is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fillfactor.is_none()
            && self.fastupdate.is_none()
            && self.gin_pending_list_limit.is_none()
            && self.buffering.is_none()
            && self.deduplicate_items.is_none()
            && self.pages_per_range.is_none()
            && self.autosummarize.is_none()
            && self.extra.is_empty()
    }
}

/// `buffering` setting for GiST/SP-GiST index builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferingMode {
    /// Enable buffered index build.
    On,
    /// Disable buffered index build.
    Off,
    /// Let PG decide based on available memory (default).
    Auto,
}

impl BufferingMode {
    /// Returns the lowercase SQL keyword for this mode.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Auto => "auto",
        }
    }
}

impl FromStr for BufferingMode {
    type Err = ();

    /// Parse from `pg_class.reloptions` text or source SQL value.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_storage_is_empty() {
        assert!(TableStorageOptions::default().is_empty());
        assert!(IndexStorageOptions::default().is_empty());
    }

    #[test]
    fn autovacuum_extra_key_makes_non_empty() {
        let mut s = TableStorageOptions::default();
        s.extra
            .insert("autovacuum_enabled".into(), "false".into());
        assert!(!s.is_empty());
    }

    #[test]
    fn non_empty_storage_detected() {
        let s = TableStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        assert!(!s.is_empty());
    }

    #[test]
    fn buffering_mode_roundtrips() {
        for m in [BufferingMode::On, BufferingMode::Off, BufferingMode::Auto] {
            assert_eq!(m.sql_keyword().parse::<BufferingMode>(), Ok(m));
        }
        assert!("bogus".parse::<BufferingMode>().is_err());
    }

    #[test]
    fn extra_is_sorted_via_btreemap() {
        let mut s = TableStorageOptions::default();
        s.extra.insert("zebra".into(), "1".into());
        s.extra.insert("alpha".into(), "2".into());
        let keys: Vec<_> = s.extra.keys().cloned().collect();
        assert_eq!(keys, vec!["alpha", "zebra"]);
    }
}
