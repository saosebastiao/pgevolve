//! SQL rendering for reloption SET statements.

use crate::identifier::QualifiedName;
use crate::ir::reloptions::{
    AutovacuumOptions, IndexStorageOptions, NotNanF64, TableStorageOptions,
};

/// `ALTER TABLE qname SET (key = value, ...);`
#[must_use]
pub fn alter_table_set_storage(qname: &QualifiedName, opts: &TableStorageOptions) -> String {
    format!(
        "ALTER TABLE {} SET ({});",
        qname.render_sql(),
        render_table_options(opts)
    )
}

/// `ALTER INDEX qname SET (key = value, ...);`
#[must_use]
pub fn alter_index_set_storage(qname: &QualifiedName, opts: &IndexStorageOptions) -> String {
    format!(
        "ALTER INDEX {} SET ({});",
        qname.render_sql(),
        render_index_options(opts)
    )
}

/// `ALTER MATERIALIZED VIEW qname SET (key = value, ...);`
#[must_use]
pub fn alter_mv_set_storage(qname: &QualifiedName, opts: &TableStorageOptions) -> String {
    format!(
        "ALTER MATERIALIZED VIEW {} SET ({});",
        qname.render_sql(),
        render_table_options(opts)
    )
}

fn render_table_options(opts: &TableStorageOptions) -> String {
    let mut parts = Vec::new();
    if let Some(v) = opts.fillfactor {
        parts.push(format!("fillfactor = {v}"));
    }
    render_autovacuum(&opts.autovacuum, &mut parts);
    if let Some(v) = opts.parallel_workers {
        parts.push(format!("parallel_workers = {v}"));
    }
    if let Some(v) = opts.toast_tuple_target {
        parts.push(format!("toast_tuple_target = {v}"));
    }
    if let Some(v) = opts.user_catalog_table {
        parts.push(format!("user_catalog_table = {v}"));
    }
    if let Some(v) = opts.vacuum_truncate {
        parts.push(format!("vacuum_truncate = {v}"));
    }
    for (k, v) in &opts.extra {
        parts.push(format!("{k} = {v}"));
    }
    parts.join(", ")
}

pub(crate) fn render_index_options(opts: &IndexStorageOptions) -> String {
    let mut parts = Vec::new();
    if let Some(v) = opts.fillfactor {
        parts.push(format!("fillfactor = {v}"));
    }
    if let Some(v) = opts.fastupdate {
        parts.push(format!("fastupdate = {v}"));
    }
    if let Some(v) = opts.gin_pending_list_limit {
        parts.push(format!("gin_pending_list_limit = {v}"));
    }
    if let Some(v) = opts.buffering {
        parts.push(format!("buffering = {}", v.sql_keyword()));
    }
    if let Some(v) = opts.deduplicate_items {
        parts.push(format!("deduplicate_items = {v}"));
    }
    if let Some(v) = opts.pages_per_range {
        parts.push(format!("pages_per_range = {v}"));
    }
    if let Some(v) = opts.autosummarize {
        parts.push(format!("autosummarize = {v}"));
    }
    for (k, v) in &opts.extra {
        parts.push(format!("{k} = {v}"));
    }
    parts.join(", ")
}

fn render_autovacuum(opts: &AutovacuumOptions, parts: &mut Vec<String>) {
    if let Some(v) = opts.enabled {
        parts.push(format!("autovacuum_enabled = {v}"));
    }
    if let Some(v) = opts.vacuum_threshold {
        parts.push(format!("autovacuum_vacuum_threshold = {v}"));
    }
    if let Some(v) = opts.vacuum_scale_factor {
        parts.push(format!(
            "autovacuum_vacuum_scale_factor = {}",
            render_f64(v)
        ));
    }
    if let Some(v) = opts.vacuum_cost_delay {
        parts.push(format!("autovacuum_vacuum_cost_delay = {v}"));
    }
    if let Some(v) = opts.vacuum_cost_limit {
        parts.push(format!("autovacuum_vacuum_cost_limit = {v}"));
    }
    if let Some(v) = opts.analyze_threshold {
        parts.push(format!("autovacuum_analyze_threshold = {v}"));
    }
    if let Some(v) = opts.analyze_scale_factor {
        parts.push(format!(
            "autovacuum_analyze_scale_factor = {}",
            render_f64(v)
        ));
    }
    if let Some(v) = opts.freeze_max_age {
        parts.push(format!("autovacuum_freeze_max_age = {v}"));
    }
    if let Some(v) = opts.freeze_min_age {
        parts.push(format!("autovacuum_freeze_min_age = {v}"));
    }
    if let Some(v) = opts.freeze_table_age {
        parts.push(format!("autovacuum_freeze_table_age = {v}"));
    }
    if let Some(v) = opts.multixact_freeze_max_age {
        parts.push(format!("autovacuum_multixact_freeze_max_age = {v}"));
    }
    if let Some(v) = opts.multixact_freeze_min_age {
        parts.push(format!("autovacuum_multixact_freeze_min_age = {v}"));
    }
    if let Some(v) = opts.multixact_freeze_table_age {
        parts.push(format!("autovacuum_multixact_freeze_table_age = {v}"));
    }
    if let Some(v) = opts.vacuum_insert_threshold {
        parts.push(format!("autovacuum_vacuum_insert_threshold = {v}"));
    }
    if let Some(v) = opts.vacuum_insert_scale_factor {
        parts.push(format!(
            "autovacuum_vacuum_insert_scale_factor = {}",
            render_f64(v)
        ));
    }
    if let Some(v) = opts.log_min_duration {
        parts.push(format!("log_autovacuum_min_duration = {v}"));
    }
}

fn render_f64(v: NotNanF64) -> String {
    // PG accepts both 0.05 and 5e-2; render the most readable form.
    // Avoid trailing zeros, but always show at least one decimal place
    // so PG doesn't interpret as an integer.
    let f = v.get();
    // `fract() == 0.0` via bit comparison: NotNanF64 guarantees non-NaN, so the
    // fractional part is either 0.0 or negative-zero (both safe to bit-compare).
    if f.fract().to_bits() == 0_f64.to_bits() {
        format!("{f}.0")
    } else {
        format!("{f}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::reloptions::BufferingMode;

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(
            Identifier::from_unquoted(schema).unwrap(),
            Identifier::from_unquoted(name).unwrap(),
        )
    }

    #[test]
    fn renders_alter_table_fillfactor() {
        let opts = TableStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert_eq!(sql, "ALTER TABLE app.t SET (fillfactor = 80);");
    }

    #[test]
    fn renders_alter_table_multiple_keys() {
        let mut opts = TableStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        opts.autovacuum.enabled = Some(false);
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert!(sql.contains("fillfactor = 80"));
        assert!(sql.contains("autovacuum_enabled = false"));
    }

    #[test]
    fn renders_alter_index_buffering() {
        let opts = IndexStorageOptions {
            buffering: Some(BufferingMode::Auto),
            ..Default::default()
        };
        let sql = alter_index_set_storage(&qn("app", "i"), &opts);
        assert_eq!(sql, "ALTER INDEX app.i SET (buffering = auto);");
    }

    #[test]
    fn renders_alter_mv_autovacuum() {
        let mut opts = TableStorageOptions::default();
        opts.autovacuum.enabled = Some(false);
        let sql = alter_mv_set_storage(&qn("app", "m"), &opts);
        assert_eq!(
            sql,
            "ALTER MATERIALIZED VIEW app.m SET (autovacuum_enabled = false);"
        );
    }

    #[test]
    fn f64_with_integer_value_renders_with_decimal() {
        let v = NotNanF64::new(5.0).unwrap();
        assert_eq!(render_f64(v), "5.0");
    }

    #[test]
    fn f64_with_decimal_renders_compactly() {
        let v = NotNanF64::new(0.05).unwrap();
        assert_eq!(render_f64(v), "0.05");
    }

    #[test]
    fn extra_bag_keys_rendered() {
        let mut opts = TableStorageOptions::default();
        opts.extra.insert("pg_partman.foo".into(), "bar".into());
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert!(sql.contains("pg_partman.foo = bar"));
    }
}
