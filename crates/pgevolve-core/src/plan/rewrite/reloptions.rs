//! SQL rendering for reloption SET statements.

use crate::identifier::QualifiedName;
use crate::ir::reloptions::{IndexStorageOptions, TableStorageOptions};

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
        opts.extra
            .insert("autovacuum_enabled".into(), "false".into());
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
        opts.extra
            .insert("autovacuum_enabled".into(), "false".into());
        let sql = alter_mv_set_storage(&qn("app", "m"), &opts);
        assert_eq!(
            sql,
            "ALTER MATERIALIZED VIEW app.m SET (autovacuum_enabled = false);"
        );
    }

    #[test]
    fn renders_autovacuum_scale_factor_from_extra_verbatim() {
        let mut opts = TableStorageOptions::default();
        opts.extra
            .insert("autovacuum_vacuum_scale_factor".into(), "0.05".into());
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert_eq!(
            sql,
            "ALTER TABLE app.t SET (autovacuum_vacuum_scale_factor = 0.05);"
        );
    }

    #[test]
    fn extra_bag_keys_rendered() {
        let mut opts = TableStorageOptions::default();
        opts.extra.insert("pg_partman.foo".into(), "bar".into());
        let sql = alter_table_set_storage(&qn("app", "t"), &opts);
        assert!(sql.contains("pg_partman.foo = bar"));
    }
}
