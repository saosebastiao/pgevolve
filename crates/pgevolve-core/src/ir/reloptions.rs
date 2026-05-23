//! Storage parameters / reloptions for Table, Index, `MaterializedView`.
//!
//! Typed fields for well-known keys + `extra: BTreeMap<String, String>` for
//! extension-registered or otherwise-unknown options. Tables and MVs share
//! the autovacuum substruct because PG documents identical key sets.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// `f64` wrapper that excludes NaN — required so `Option<f64>` reloptions
/// can participate in `Eq` / `Hash` / `Ord` derived implementations.
///
/// The catalog reader and source parser both reject NaN values explicitly.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NotNanF64(f64);

impl NotNanF64 {
    /// Construct, rejecting NaN.
    ///
    /// # Errors
    ///
    /// Returns the input value when NaN.
    pub const fn new(v: f64) -> Result<Self, f64> {
        if v.is_nan() { Err(v) } else { Ok(Self(v)) }
    }

    /// Return the inner `f64` value.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for NotNanF64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for NotNanF64 {}
impl Hash for NotNanF64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}
impl PartialOrd for NotNanF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for NotNanF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        // Safe because NaN is excluded by construction.
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

/// Shared autovacuum options — apply to both `Table` and `MaterializedView`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct AutovacuumOptions {
    /// `autovacuum_enabled` — disable autovacuum for this relation.
    pub enabled: Option<bool>,
    /// `autovacuum_vacuum_threshold` — minimum tuple updates/deletes before vacuum.
    pub vacuum_threshold: Option<u64>,
    /// `autovacuum_vacuum_scale_factor` — fraction of table size added to threshold.
    pub vacuum_scale_factor: Option<NotNanF64>,
    /// `autovacuum_vacuum_cost_delay` — cost delay in milliseconds.
    pub vacuum_cost_delay: Option<u64>,
    /// `autovacuum_vacuum_cost_limit` — cost limit before napping.
    pub vacuum_cost_limit: Option<u64>,
    /// `autovacuum_analyze_threshold` — minimum tuple inserts/updates/deletes before analyze.
    pub analyze_threshold: Option<u64>,
    /// `autovacuum_analyze_scale_factor` — fraction of table size added to analyze threshold.
    pub analyze_scale_factor: Option<NotNanF64>,
    /// `autovacuum_freeze_max_age` — max age before forced vacuum to prevent wraparound.
    pub freeze_max_age: Option<u64>,
    /// `autovacuum_freeze_min_age` — min age before freezing tuples.
    pub freeze_min_age: Option<u64>,
    /// `autovacuum_freeze_table_age` — table age at which whole-table freeze scan runs.
    pub freeze_table_age: Option<u64>,
    /// `autovacuum_multixact_freeze_max_age` — max multixact age before forced vacuum.
    pub multixact_freeze_max_age: Option<u64>,
    /// `autovacuum_multixact_freeze_min_age` — min multixact age before freezing.
    pub multixact_freeze_min_age: Option<u64>,
    /// `autovacuum_multixact_freeze_table_age` — table multixact age before whole-table scan.
    pub multixact_freeze_table_age: Option<u64>,
    /// `autovacuum_vacuum_insert_threshold` (PG 13+).
    pub vacuum_insert_threshold: Option<u64>,
    /// `autovacuum_vacuum_insert_scale_factor` (PG 13+).
    pub vacuum_insert_scale_factor: Option<NotNanF64>,
    /// `log_autovacuum_min_duration` — `-1` disables. Stored as i64.
    pub log_min_duration: Option<i64>,
}

impl AutovacuumOptions {
    /// `true` iff every field is `None`. Used by the differ to short-circuit.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.vacuum_threshold.is_none()
            && self.vacuum_scale_factor.is_none()
            && self.vacuum_cost_delay.is_none()
            && self.vacuum_cost_limit.is_none()
            && self.analyze_threshold.is_none()
            && self.analyze_scale_factor.is_none()
            && self.freeze_max_age.is_none()
            && self.freeze_min_age.is_none()
            && self.freeze_table_age.is_none()
            && self.multixact_freeze_max_age.is_none()
            && self.multixact_freeze_min_age.is_none()
            && self.multixact_freeze_table_age.is_none()
            && self.vacuum_insert_threshold.is_none()
            && self.vacuum_insert_scale_factor.is_none()
            && self.log_min_duration.is_none()
    }
}

/// Storage options for tables. MV reuses via type alias since PG documents
/// identical reloptions for both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct TableStorageOptions {
    /// `fillfactor` — target heap page density (10..=100).
    pub fillfactor: Option<u32>,
    /// Autovacuum parameters. All keys are prefixed `autovacuum_` in PG.
    pub autovacuum: AutovacuumOptions,
    /// `parallel_workers` — number of parallel workers (0..=1024).
    pub parallel_workers: Option<u32>,
    /// `toast_tuple_target` — TOAST compression threshold in bytes (128..=8160).
    pub toast_tuple_target: Option<u32>,
    /// `user_catalog_table` — treat as a catalog table for logical replication.
    pub user_catalog_table: Option<bool>,
    /// `vacuum_truncate` — allow VACUUM to truncate trailing empty pages (PG 12+).
    pub vacuum_truncate: Option<bool>,
    /// Unknown / extension-registered options. Always sorted by key (`BTreeMap`).
    pub extra: BTreeMap<String, String>,
}

impl TableStorageOptions {
    /// `true` iff every typed field is `None` and `extra` is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fillfactor.is_none()
            && self.autovacuum.is_empty()
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
    fn not_nan_rejects_nan() {
        assert!(NotNanF64::new(f64::NAN).is_err());
        assert!(NotNanF64::new(1.5).is_ok());
        assert!(NotNanF64::new(0.0).is_ok());
        assert!(NotNanF64::new(f64::INFINITY).is_ok());
    }

    #[test]
    fn not_nan_equality_is_bit_exact() {
        let a = NotNanF64::new(0.1).unwrap();
        let b = NotNanF64::new(0.1).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn default_storage_is_empty() {
        assert!(TableStorageOptions::default().is_empty());
        assert!(IndexStorageOptions::default().is_empty());
        assert!(AutovacuumOptions::default().is_empty());
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
