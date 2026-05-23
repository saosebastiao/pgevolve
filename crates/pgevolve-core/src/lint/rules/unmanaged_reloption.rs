//! Warns when the catalog has reloptions not declared in source.
//!
//! Per the lenient drift policy in `diff::reloptions`, catalog reloptions that
//! don't appear in source are NOT reset by the differ. This lint surfaces them
//! so operators can decide whether to bring under management or accept the drift.

use crate::ir::catalog::Catalog;
use crate::ir::reloptions::{AutovacuumOptions, IndexStorageOptions, TableStorageOptions};
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "unmanaged-reloption";

/// Fire a warning when `tgt` is `Some` but `src` is `None`.
macro_rules! check_field {
    ($findings:expr, $kind:expr, $qname:expr, $label:expr, $src:expr, $tgt:expr) => {
        if $src.is_none() && $tgt.is_some() {
            $findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Warning,
                message: format!(
                    "{} {}: catalog has reloption {} not declared in source",
                    $kind, $qname, $label
                ),
                location: None,
            });
        }
    };
}

/// Check the autovacuum substruct; fired for both Table and MV.
//
// `too_many_lines`: `AutovacuumOptions` has 16 typed fields; each check_field!
// invocation occupies ~6 lines. This is mechanical enumeration, not complexity.
#[allow(clippy::too_many_lines)]
fn check_autovacuum(
    kind: &str,
    qname: &dyn std::fmt::Display,
    src: &AutovacuumOptions,
    tgt: &AutovacuumOptions,
    findings: &mut Vec<Finding>,
) {
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_enabled",
        src.enabled,
        tgt.enabled
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_vacuum_threshold",
        src.vacuum_threshold,
        tgt.vacuum_threshold
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_vacuum_scale_factor",
        src.vacuum_scale_factor,
        tgt.vacuum_scale_factor
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_vacuum_cost_delay",
        src.vacuum_cost_delay,
        tgt.vacuum_cost_delay
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_vacuum_cost_limit",
        src.vacuum_cost_limit,
        tgt.vacuum_cost_limit
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_analyze_threshold",
        src.analyze_threshold,
        tgt.analyze_threshold
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_analyze_scale_factor",
        src.analyze_scale_factor,
        tgt.analyze_scale_factor
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_freeze_max_age",
        src.freeze_max_age,
        tgt.freeze_max_age
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_freeze_min_age",
        src.freeze_min_age,
        tgt.freeze_min_age
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_freeze_table_age",
        src.freeze_table_age,
        tgt.freeze_table_age
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_multixact_freeze_max_age",
        src.multixact_freeze_max_age,
        tgt.multixact_freeze_max_age
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_multixact_freeze_min_age",
        src.multixact_freeze_min_age,
        tgt.multixact_freeze_min_age
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_multixact_freeze_table_age",
        src.multixact_freeze_table_age,
        tgt.multixact_freeze_table_age
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_vacuum_insert_threshold",
        src.vacuum_insert_threshold,
        tgt.vacuum_insert_threshold
    );
    check_field!(
        findings,
        kind,
        qname,
        "autovacuum_vacuum_insert_scale_factor",
        src.vacuum_insert_scale_factor,
        tgt.vacuum_insert_scale_factor
    );
    check_field!(
        findings,
        kind,
        qname,
        "log_autovacuum_min_duration",
        src.log_min_duration,
        tgt.log_min_duration
    );
}

/// Check table/MV storage options (typed fields + extra bag + autovacuum).
fn check_table_storage(
    kind: &str,
    qname: &dyn std::fmt::Display,
    src: &TableStorageOptions,
    tgt: &TableStorageOptions,
    findings: &mut Vec<Finding>,
) {
    check_field!(
        findings,
        kind,
        qname,
        "fillfactor",
        src.fillfactor,
        tgt.fillfactor
    );
    check_field!(
        findings,
        kind,
        qname,
        "parallel_workers",
        src.parallel_workers,
        tgt.parallel_workers
    );
    check_field!(
        findings,
        kind,
        qname,
        "toast_tuple_target",
        src.toast_tuple_target,
        tgt.toast_tuple_target
    );
    check_field!(
        findings,
        kind,
        qname,
        "user_catalog_table",
        src.user_catalog_table,
        tgt.user_catalog_table
    );
    check_field!(
        findings,
        kind,
        qname,
        "vacuum_truncate",
        src.vacuum_truncate,
        tgt.vacuum_truncate
    );

    // Extra bag: warn for each catalog key not present in source.
    for key in tgt.extra.keys() {
        if !src.extra.contains_key(key) {
            findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Warning,
                message: format!(
                    "{kind} {qname}: catalog has reloption {key} not declared in source"
                ),
                location: None,
            });
        }
    }

    // Autovacuum substruct.
    check_autovacuum(kind, qname, &src.autovacuum, &tgt.autovacuum, findings);
}

/// Check index storage options (typed fields + extra bag; no autovacuum).
fn check_index_storage(
    qname: &dyn std::fmt::Display,
    src: &IndexStorageOptions,
    tgt: &IndexStorageOptions,
    findings: &mut Vec<Finding>,
) {
    check_field!(
        findings,
        "index",
        qname,
        "fillfactor",
        src.fillfactor,
        tgt.fillfactor
    );
    check_field!(
        findings,
        "index",
        qname,
        "fastupdate",
        src.fastupdate,
        tgt.fastupdate
    );
    check_field!(
        findings,
        "index",
        qname,
        "gin_pending_list_limit",
        src.gin_pending_list_limit,
        tgt.gin_pending_list_limit
    );
    check_field!(
        findings,
        "index",
        qname,
        "buffering",
        src.buffering,
        tgt.buffering
    );
    check_field!(
        findings,
        "index",
        qname,
        "deduplicate_items",
        src.deduplicate_items,
        tgt.deduplicate_items
    );
    check_field!(
        findings,
        "index",
        qname,
        "pages_per_range",
        src.pages_per_range,
        tgt.pages_per_range
    );
    check_field!(
        findings,
        "index",
        qname,
        "autosummarize",
        src.autosummarize,
        tgt.autosummarize
    );

    // Extra bag: warn for each catalog key not present in source.
    for key in tgt.extra.keys() {
        if !src.extra.contains_key(key) {
            findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Warning,
                message: format!(
                    "index {qname}: catalog has reloption {key} not declared in source"
                ),
                location: None,
            });
        }
    }
}

/// Run the `unmanaged-reloption` rule against a (source, target) catalog pair.
pub fn check(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();

    for src_t in &source.tables {
        let Some(tgt_t) = target.tables.iter().find(|t| t.qname == src_t.qname) else {
            continue;
        };
        check_table_storage(
            "table",
            &src_t.qname,
            &src_t.storage,
            &tgt_t.storage,
            &mut findings,
        );
    }

    for src_i in &source.indexes {
        let Some(tgt_i) = target.indexes.iter().find(|i| i.qname == src_i.qname) else {
            continue;
        };
        check_index_storage(&src_i.qname, &src_i.storage, &tgt_i.storage, &mut findings);
    }

    for src_m in &source.materialized_views {
        let Some(tgt_m) = target
            .materialized_views
            .iter()
            .find(|m| m.qname == src_m.qname)
        else {
            continue;
        };
        check_table_storage(
            "materialized view",
            &src_m.qname,
            &src_m.storage,
            &tgt_m.storage,
            &mut findings,
        );
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::index::{
        Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
    };
    use crate::ir::reloptions::{
        IndexStorageOptions, MaterializedViewStorageOptions, TableStorageOptions,
    };
    use crate::ir::table::Table;
    use crate::ir::view::MaterializedView;
    use crate::parse::normalize_body::NormalizedBody;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn empty_table(qname: QualifiedName) -> Table {
        Table {
            qname,
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: TableStorageOptions::default(),
        }
    }

    fn empty_mv(qname: QualifiedName) -> MaterializedView {
        MaterializedView {
            qname,
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: MaterializedViewStorageOptions::default(),
        }
    }

    fn empty_index(qname: QualifiedName, on: QualifiedName) -> Index {
        Index {
            qname,
            on: IndexParent::Table(on),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: false,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: IndexStorageOptions::default(),
        }
    }

    #[test]
    fn empty_catalogs_silent() {
        let source = Catalog::empty();
        let target = Catalog::empty();
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn unmanaged_fillfactor_fires() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();

        // Source table has no fillfactor declared.
        source.tables.push(empty_table(qn("app", "orders")));

        // Target (live DB) has fillfactor = 80 set.
        let mut tgt_table = empty_table(qn("app", "orders"));
        tgt_table.storage.fillfactor = Some(80);
        target.tables.push(tgt_table);

        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1, "expected exactly one finding");
        assert_eq!(findings[0].severity, Severity::Warning);
        assert_eq!(findings[0].rule, RULE_ID);
        assert!(
            findings[0].message.contains("fillfactor"),
            "message should mention 'fillfactor': {}",
            findings[0].message
        );
    }

    #[test]
    fn matching_storage_silent() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();

        // Both source and target declare fillfactor = 80 — no drift.
        let mut src_table = empty_table(qn("app", "orders"));
        src_table.storage.fillfactor = Some(80);
        source.tables.push(src_table);

        let mut tgt_table = empty_table(qn("app", "orders"));
        tgt_table.storage.fillfactor = Some(80);
        target.tables.push(tgt_table);

        let findings = check(&source, &target);
        assert!(
            findings.is_empty(),
            "expected no findings when both sides agree: {findings:?}"
        );
    }

    #[test]
    fn unmanaged_extra_bag_key_fires() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();

        // Source table has no extra keys.
        source.tables.push(empty_table(qn("app", "events")));

        // Target has an extension-registered key in its extra bag.
        let mut tgt_table = empty_table(qn("app", "events"));
        tgt_table
            .storage
            .extra
            .insert("pg_partman.x".to_owned(), "y".to_owned());
        target.tables.push(tgt_table);

        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1, "expected exactly one finding");
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("pg_partman.x"),
            "message should mention the key: {}",
            findings[0].message
        );
    }

    #[test]
    fn index_unmanaged_fillfactor_fires() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();

        source
            .indexes
            .push(empty_index(qn("app", "orders_idx"), qn("app", "orders")));

        let mut tgt_idx = empty_index(qn("app", "orders_idx"), qn("app", "orders"));
        tgt_idx.storage.fillfactor = Some(70);
        target.indexes.push(tgt_idx);

        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("fillfactor"));
        assert!(findings[0].message.contains("index"));
    }

    #[test]
    fn mv_unmanaged_fillfactor_fires() {
        let mut source = Catalog::empty();
        let mut target = Catalog::empty();

        source
            .materialized_views
            .push(empty_mv(qn("app", "summary")));

        let mut tgt_mv = empty_mv(qn("app", "summary"));
        tgt_mv.storage.fillfactor = Some(90);
        target.materialized_views.push(tgt_mv);

        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("fillfactor"));
        assert!(findings[0].message.contains("materialized view"));
    }

    #[test]
    fn source_only_table_skipped() {
        // A table in source that has no match in target is silently skipped
        // (it doesn't exist in the DB yet — nothing to surface as drift).
        let mut source = Catalog::empty();
        let target = Catalog::empty();

        let mut src_table = empty_table(qn("app", "new_table"));
        src_table.storage.fillfactor = Some(80);
        source.tables.push(src_table);

        let findings = check(&source, &target);
        assert!(findings.is_empty());
    }
}
