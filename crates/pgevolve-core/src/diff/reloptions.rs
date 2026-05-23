//! Sparse-delta diffing for storage reloptions. Lenient policy:
//! source `None` always means "skip"; catalog values not in source surface
//! as `unmanaged-reloption` lint, never RESET.

use crate::ir::reloptions::{AutovacuumOptions, IndexStorageOptions, TableStorageOptions};

/// Build the sparse delta for table (or materialized-view) storage options.
///
/// Only fields where source is `Some(_)` **and** target disagrees flow into
/// the returned value. Fields absent from source (`None`) are left as `None`
/// in the delta — the lenient policy treats them as unmanaged.
#[must_use]
pub fn table_delta(
    target: &TableStorageOptions,
    source: &TableStorageOptions,
) -> TableStorageOptions {
    let mut out = TableStorageOptions::default();

    macro_rules! diff_field {
        ($field:ident) => {
            if let Some(src) = &source.$field {
                if target.$field.as_ref() != Some(src) {
                    out.$field = Some(src.clone());
                }
            }
        };
    }

    diff_field!(fillfactor);
    diff_field!(parallel_workers);
    diff_field!(toast_tuple_target);
    diff_field!(user_catalog_table);
    diff_field!(vacuum_truncate);

    out.autovacuum = autovacuum_delta(&target.autovacuum, &source.autovacuum);

    // Extra bag: only keys present in source and (missing-or-different) in catalog.
    for (k, src_v) in &source.extra {
        if target.extra.get(k) != Some(src_v) {
            out.extra.insert(k.clone(), src_v.clone());
        }
    }

    out
}

/// Build the sparse delta for index storage options.
///
/// Same lenient-policy semantics as [`table_delta`].
#[must_use]
pub fn index_delta(
    target: &IndexStorageOptions,
    source: &IndexStorageOptions,
) -> IndexStorageOptions {
    let mut out = IndexStorageOptions::default();

    macro_rules! diff_field {
        ($field:ident) => {
            if let Some(src) = &source.$field {
                if target.$field.as_ref() != Some(src) {
                    out.$field = Some(src.clone());
                }
            }
        };
    }

    diff_field!(fillfactor);
    diff_field!(fastupdate);
    diff_field!(gin_pending_list_limit);
    diff_field!(buffering);
    diff_field!(deduplicate_items);
    diff_field!(pages_per_range);
    diff_field!(autosummarize);

    for (k, src_v) in &source.extra {
        if target.extra.get(k) != Some(src_v) {
            out.extra.insert(k.clone(), src_v.clone());
        }
    }

    out
}

fn autovacuum_delta(target: &AutovacuumOptions, source: &AutovacuumOptions) -> AutovacuumOptions {
    let mut out = AutovacuumOptions::default();

    macro_rules! diff_field {
        ($field:ident) => {
            if let Some(src) = &source.$field {
                if target.$field.as_ref() != Some(src) {
                    out.$field = Some(src.clone());
                }
            }
        };
    }

    diff_field!(enabled);
    diff_field!(vacuum_threshold);
    diff_field!(vacuum_scale_factor);
    diff_field!(vacuum_cost_delay);
    diff_field!(vacuum_cost_limit);
    diff_field!(analyze_threshold);
    diff_field!(analyze_scale_factor);
    diff_field!(freeze_max_age);
    diff_field!(freeze_min_age);
    diff_field!(freeze_table_age);
    diff_field!(multixact_freeze_max_age);
    diff_field!(multixact_freeze_min_age);
    diff_field!(multixact_freeze_table_age);
    diff_field!(vacuum_insert_threshold);
    diff_field!(vacuum_insert_scale_factor);
    diff_field!(log_min_duration);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_yields_empty_delta() {
        let t = TableStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        let s = TableStorageOptions::default();
        let delta = table_delta(&t, &s);
        assert!(delta.is_empty(), "lenient: source None means skip");
    }

    #[test]
    fn source_only_fillfactor_emits_one_field() {
        let t = TableStorageOptions::default();
        let s = TableStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        let delta = table_delta(&t, &s);
        assert_eq!(delta.fillfactor, Some(80));
        assert!(delta.autovacuum.is_empty());
    }

    #[test]
    fn matching_source_and_target_yields_empty_delta() {
        let t = TableStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        let s = TableStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        let delta = table_delta(&t, &s);
        assert!(delta.is_empty());
    }

    #[test]
    fn source_extra_key_not_in_target_emits() {
        let t = TableStorageOptions::default();
        let mut s = TableStorageOptions::default();
        s.extra.insert("pg_partman.foo".into(), "value".into());
        let delta = table_delta(&t, &s);
        assert_eq!(
            delta.extra.get("pg_partman.foo").map(String::as_str),
            Some("value")
        );
    }

    #[test]
    fn target_extra_key_not_in_source_does_not_emit() {
        let mut t = TableStorageOptions::default();
        t.extra.insert("pg_partman.foo".into(), "value".into());
        let s = TableStorageOptions::default();
        let delta = table_delta(&t, &s);
        assert!(
            delta.is_empty(),
            "lenient: unmanaged extra-bag keys ignored"
        );
    }

    #[test]
    fn index_delta_fillfactor_change() {
        let t = IndexStorageOptions {
            fillfactor: Some(70),
            ..Default::default()
        };
        let s = IndexStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        let delta = index_delta(&t, &s);
        assert_eq!(delta.fillfactor, Some(80));
    }
}
