//! Errors when source declares a `VIRTUAL` generated column but
//! `[managed].min_pg_version < 18`.
//!
//! `GENERATED ALWAYS AS (expr) VIRTUAL` arrived in Postgres 18. Declaring one on
//! a project targeting an earlier release fails at apply time; surface it at plan
//! time with a remediation the author can act on.
//!
//! # Why this is a lint and not a parse error
//!
//! The vendored parser is always the newest supported major, so the grammar
//! accepts the superset regardless of what any particular target runs. Rejecting
//! by version *at parse time* would make parse results depend on configuration
//! and destroy parse-once-plan-for-many-targets — one source tree could no
//! longer be planned against a PG 17 and a PG 18 server from the same parse.
//!
//! So the split is: the parser answers "is this valid Postgres?", and this lint
//! answers "can *your* server run it?".
//!
//! Plan-time gate — registered via
//! [`crate::lint::universal::check_plan_time_catalog`].

use crate::ir::catalog::Catalog;
use crate::ir::column::GeneratedKind;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "column-virtual-generated-requires-pg-18";

pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    if min_pg_version >= 18 {
        return Vec::new();
    }
    source
        .tables
        .iter()
        .flat_map(|table| {
            table.columns.iter().filter_map(move |column| {
                let generated = column.generated.as_ref()?;
                (generated.kind == GeneratedKind::Virtual).then(|| {
                    Finding::error(
                        RULE_ID,
                        format!(
                            "column {}.{}: GENERATED ... VIRTUAL requires Postgres 18 or \
                             later (min_pg_version = {min_pg_version}); raise \
                             [managed].min_pg_version to 18, or use STORED — note that \
                             STORED materialises the value on write, so the change is not \
                             purely cosmetic",
                            table.qname, column.name,
                        ),
                    )
                })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{RULE_ID, check};
    use crate::ir::catalog::Catalog;
    use crate::ir::column::{Column, Generated, GeneratedKind};
    use crate::ir::column_type::ColumnType;
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::table::Table;
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::{id, qn};

    fn catalog_with(kind: Option<GeneratedKind>) -> Catalog {
        let column = Column {
            name: id("c"),
            ty: ColumnType::Integer,
            nullable: true,
            default: None,
            identity: None,
            generated: kind.map(|kind| Generated {
                kind,
                expression: NormalizedExpr::from_text("(1 + 1)"),
            }),
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        };
        let table = Table {
            qname: qn("app", "t"),
            columns: vec![column],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
        };
        let mut catalog = Catalog::default();
        catalog.tables.push(table);
        catalog
    }

    #[test]
    fn fires_on_virtual_below_pg_18() {
        let findings = check(&catalog_with(Some(GeneratedKind::Virtual)), 16);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(
            findings[0].message.contains("app.t.c"),
            "message should name the column: {}",
            findings[0].message
        );
    }

    #[test]
    fn silent_at_pg_18_and_above() {
        assert!(check(&catalog_with(Some(GeneratedKind::Virtual)), 18).is_empty());
        assert!(check(&catalog_with(Some(GeneratedKind::Virtual)), 19).is_empty());
    }

    #[test]
    fn stored_never_fires() {
        // STORED has been available since PG 12; the floor does not apply to it.
        assert!(check(&catalog_with(Some(GeneratedKind::Stored)), 14).is_empty());
    }

    #[test]
    fn plain_columns_never_fire() {
        assert!(check(&catalog_with(None), 14).is_empty());
    }
}
