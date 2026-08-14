//! Errors when source declares a `NOT ENFORCED` constraint but
//! `[managed].min_pg_version < 18`.
//!
//! `NOT ENFORCED` arrived in Postgres 18. Declaring one on a project targeting
//! an earlier release fails at apply time.
//!
//! See [`super::column_virtual_generated_requires_pg_18`] for why version
//! rejection lives here and not in the parser.
//!
//! Plan-time gate — registered via
//! [`crate::lint::universal::check_plan_time_catalog`].

use crate::ir::catalog::Catalog;
use crate::ir::constraint::Enforcement;
use crate::lint::finding::Finding;

pub const RULE_ID: &str = "constraint-not-enforced-requires-pg-18";

pub fn check(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    if min_pg_version >= 18 {
        return Vec::new();
    }
    source
        .tables
        .iter()
        .flat_map(|table| {
            table
                .constraints
                .iter()
                .filter(|c| c.enforcement == Enforcement::NotEnforced)
                .map(move |c| {
                    Finding::error(
                        RULE_ID,
                        format!(
                            "constraint {} on {}: NOT ENFORCED requires Postgres 18 or later \
                             (min_pg_version = {min_pg_version}); raise \
                             [managed].min_pg_version to 18, or drop the NOT ENFORCED clause \
                             — note that doing so makes Postgres start checking the \
                             constraint, which can fail on existing data",
                            c.qname.name, table.qname,
                        ),
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{RULE_ID, check};
    use crate::ir::catalog::Catalog;
    use crate::ir::constraint::{Constraint, ConstraintKind, Deferrable, Enforcement};
    use crate::ir::default_expr::NormalizedExpr;
    use crate::ir::table::Table;
    use crate::lint::finding::Severity;
    use crate::lint::test_helpers::qn;

    fn catalog_with(enforcement: Enforcement) -> Catalog {
        let constraint = Constraint {
            qname: qn("app", "ck_amount"),
            kind: ConstraintKind::Check {
                expression: NormalizedExpr::from_text("(amount > 0)"),
                no_inherit: false,
            },
            deferrable: Deferrable::NotDeferrable,
            enforcement,
            comment: None,
        };
        let table = Table {
            qname: qn("app", "t"),
            columns: vec![],
            constraints: vec![constraint],
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
    fn fires_on_not_enforced_below_pg_18() {
        let findings = check(&catalog_with(Enforcement::NotEnforced), 17);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(
            findings[0].message.contains("ck_amount"),
            "message should name the constraint: {}",
            findings[0].message
        );
    }

    #[test]
    fn silent_at_pg_18_and_above() {
        assert!(check(&catalog_with(Enforcement::NotEnforced), 18).is_empty());
    }

    #[test]
    fn enforced_never_fires() {
        assert!(check(&catalog_with(Enforcement::Enforced), 14).is_empty());
    }
}
