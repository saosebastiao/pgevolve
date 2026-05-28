//! Universal lint rules. Spec §12.1.
//!
//! Most of §12.1 is already enforced by the parser (unsupported kinds,
//! duplicate qnames, schema qualification, ALTER outside the FK whitelist).
//! Rules that the parser can't enforce:
//!
//! - **`managed_schemas_match`** — every schema declared in source has a
//!   matching `managed.schemas` entry, and vice versa.
//! - **`closed_world_references`** — every FK target table exists in source.
//! - **`view_shadows_table`** — a view or MV must not share a qname with a
//!   table (PG would reject the conflict at apply time).
//! - **`mv_no_unique_index`** — an MV without a unique index cannot be
//!   refreshed concurrently.
//! - **`view_body_references_unmanaged_schema`** — a view body dependency
//!   targets a schema outside `[managed].schemas` and outside built-ins.
//! - **`type-shadows-table`** — a user-defined type's qname collides with a
//!   table, view, or MV qname (PG uses one namespace for relations and types).
//! - **`enum-value-collision`** — an enum has duplicate value labels.
//! - **`composite-attribute-collision`** — a composite has duplicate attribute
//!   names.
//! - **`domain-check-references-unmanaged-type`** — a domain's CHECK expression
//!   references a schema not in `[managed].schemas` and not a PG built-in.
//! - **`pl-pgsql-dynamic-sql`** — a PL/pgSQL function or procedure uses
//!   `EXECUTE` (dynamic SQL) without any `-- @pgevolve dep: <qname>` directive.
//! - **`procedure-contains-commit`** — a procedure body contains
//!   `COMMIT`/`ROLLBACK`; pgevolve will run it outside a transaction.
//! - **`function-references-unmanaged-schema`** — a function or procedure body
//!   dependency targets a schema outside `[managed].schemas` and outside
//!   built-ins.
//! - **`extension-version-unpinned`** — fires when a source-declared
//!   extension lacks a `VERSION` clause.
//! - **`extension-references-unmanaged-schema`** — fires when
//!   `CREATE EXTENSION ... WITH SCHEMA s` references a schema not in
//!   the source catalog.
//! - **`trigger-references-unmanaged-table`** — fires when a trigger's target
//!   table is not declared in source as a table, view, or materialized view.
//! - **`trigger-references-unmanaged-function`** — fires when a trigger's
//!   execute function is not declared in source.
//! - **`force-rls-without-policies`** — fires (Warning) when a table has
//!   `FORCE ROW LEVEL SECURITY` enabled but no policies defined. PG denies
//!   every row in that state — almost always a configuration mistake.
//! - **`publication-feature-requires-pg-version`** — fires (Error) when source
//!   uses a PG 15+ publication feature (`FOR TABLES IN SCHEMA`, row filters,
//!   column lists) and `[managed].min_pg_version < 15`. Run via
//!   [`check_plan_time_catalog`].
//! - **`column-references-unmanaged-collation`** — fires (Warning) when a
//!   column, domain, range, or composite attribute references a collation that
//!   is neither a Postgres built-in nor declared in `source.collations`. Run
//!   via [`check_plan_time_catalog`].
//! - **`range-type-references-unmanaged-subtype`** — fires (Warning) when a
//!   `CREATE TYPE … AS RANGE` declares a subtype that is neither a known
//!   built-in scalar nor a managed user-defined type. Run via
//!   [`check_plan_time_catalog`].
//! - **`nondeterministic-collation-requires-pg-12`** — fires (Error) when a
//!   source collation has `deterministic = false` and
//!   `[managed].min_pg_version < 12`. Run via [`check_plan_time_catalog`].
//! - **`builtin-provider-requires-pg-17`** — fires (Error) when a source
//!   collation uses `provider = builtin` and `[managed].min_pg_version < 17`.
//!   Run via [`check_plan_time_catalog`].
//!
//! Drift-detection rules (comparing source vs live catalog, run via [`run_drift_lints`]):
//!
//! - **`unmanaged-publication`** — catalog reports a publication source doesn't
//!   declare. Standard v0.3.x lenient-drift pattern.
//! - **`unmanaged-collation`** — catalog reports a collation source doesn't
//!   declare. Standard v0.3.x lenient-drift pattern.
//! - **`publication-captures-unmanaged-table`** — a `FOR ALL TABLES` or
//!   `FOR TABLES IN SCHEMA` publication implicitly captures tables not in source.
//! - **`publication-row-filter-references-unmanaged-column`** — a row filter
//!   expression references a column not declared on the target table in source.
//!
//! Changeset-level rules (inspecting the diff, run via [`check_changeset`]):
//!
//! - **`storage-downgrade-not-retroactive`** — fires when a `SET STORAGE`
//!   change reduces toastability (e.g., EXTERNAL → PLAIN). Existing `TOASTed`
//!   values keep their current placement until rewritten by UPDATE or VACUUM
//!   FULL.
//! - **`compression-change-not-retroactive`** — fires on any `SET COMPRESSION`
//!   change. Existing `TOASTed` values keep their original codec until rewritten
//!   by UPDATE or VACUUM FULL.
//! - **`grants-to-unmanaged-role`** — fires (Warning) when the catalog has
//!   grants to roles not declared in source. The differ already filtered these
//!   out of REVOKE (lenient drift policy); the lint surfaces them so operators
//!   can decide whether to manage them or accept the drift.
//! - **`revoke-from-owner`** — fires (Error) when a REVOKE step targets the
//!   object's owner. PG silently rejects (owner has implicit privileges); we
//!   pre-empt with a clear plan-time error.
//!
//! Cluster changeset-level rules (run via [`check_cluster_changeset`]):
//!
//! - **`role-loses-superuser`** — fires when `AlterRoleAttributes` flips
//!   `SUPERUSER` from `true` to `false`. Losing superuser is rarely routine;
//!   surfacing it lets operators catch unintended downgrades.
//! - **`role-membership-cycle`** — builds the post-apply membership graph from
//!   source IR + pending grants/revokes and checks that no pending grant creates
//!   a cycle. Postgres rejects cycles at apply time; pre-plan detection gives a
//!   better error.
//!
//! Cluster-aware source-tree rules (run via [`check_universal_with_cluster`]):
//!
//! - **`grant-references-unknown-role`** — fires when a grantee role name (on
//!   any object grant, owner field, or default-privilege grant) is not declared
//!   in the linked cluster project's roles. Silently no-ops when no
//!   `[cluster].project` is configured.

use super::ManagedConfig;
use super::finding::Finding;
use super::rules;
use super::source_tree::SourceTree;
use crate::diff::changeset::ChangeSet;
use crate::ir::catalog::Catalog;

/// All rule IDs that carry [`crate::lint::Severity::LintAtPlan`].
///
/// Preflight uses this to warn about waivers that reference unknown rule IDs
/// (typos, stale waivers for renamed rules). Add new `LintAtPlan` rule IDs
/// here when introducing them; remove stale ones when a rule is deleted.
pub const LINT_AT_PLAN_RULES: &[&str] = &["column-position-drift"];

/// Run every universal rule.
pub fn check_universal(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(rules::managed_schemas_match::check(tree, managed));
    out.extend(rules::no_duplicate_qnames::check(tree));
    out.extend(rules::closed_world_references::check(tree));
    out.extend(rules::view_shadows_table::check(tree));
    out.extend(rules::mv_no_unique_index::check(tree));
    out.extend(rules::view_body_references_unmanaged_schema::check(
        tree, managed,
    ));
    out.extend(rules::type_shadows_table::check(tree));
    out.extend(rules::enum_value_collision::check(tree));
    out.extend(rules::composite_attribute_collision::check(tree));
    out.extend(rules::domain_check_references_unmanaged_type::check(
        tree, managed,
    ));
    out.extend(rules::pl_pgsql_dynamic_sql::check(tree));
    out.extend(rules::procedure_contains_commit::check(tree));
    out.extend(rules::function_references_unmanaged_schema::check(
        tree, managed,
    ));
    out.extend(rules::extension_version_unpinned::check(tree));
    out.extend(rules::extension_references_unmanaged_schema::check(tree));
    out.extend(rules::trigger_references_unmanaged_table::check(tree));
    out.extend(rules::trigger_references_unmanaged_function::check(tree));
    out.extend(rules::partition_references_unmanaged_parent::check(tree));
    out.extend(rules::force_rls_without_policies::check(&tree.catalog));
    out
}

/// Run all drift-detection rules that compare `source` against a `target`
/// catalog (e.g. the live database). Returns a list of [`Finding`]s.
///
/// This is the entry point for lint rules that need both the source and target
/// catalogs, as opposed to [`check_universal`] which only needs the source tree.
pub fn run_drift_lints(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut out = Vec::new();
    rules::column_position_drift::check(source, target, &mut out);
    out.extend(rules::unmanaged_reloption::check(source, target));
    out.extend(rules::unmanaged_publication::check(source, target));
    out.extend(rules::publication_captures_unmanaged_table::check(
        source, target,
    ));
    out.extend(rules::publication_row_filter_references_unmanaged_column::check(source));
    out.extend(rules::unmanaged_statistic::check(source, target));
    out.extend(rules::unmanaged_subscription::check(source, target));
    out.extend(rules::unmanaged_collation::check(source, target));
    out
}

/// Run all changeset-level lint rules against `cs`.
///
/// These rules inspect the diff produced by the differ — they fire on
/// structural changes (e.g., `SetColumnStorage`, `SetColumnCompression`) rather
/// than on the source catalog directly.
pub fn check_changeset(cs: &ChangeSet) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(rules::storage_downgrade_not_retroactive::check(cs));
    out.extend(rules::compression_change_not_retroactive::check(cs));
    out.extend(rules::grants_to_unmanaged_role::check(cs));
    out.extend(rules::revoke_from_owner::check(cs));
    out
}

/// Run catalog-level lint rules that fire at plan time.
///
/// This is a subset of [`check_universal`] — rules that inspect the source
/// catalog (desired state) and do not require the full `SourceTree` context
/// (managed config, file locations, etc.). Intended for callers such as the
/// conformance pipeline that have a `Catalog` but not a full `SourceTree`.
///
/// `min_pg_version` is the minimum Postgres major version the project targets
/// (from `[managed].min_pg_version`; default `14`). Used to gate PG-version-
/// specific source features at lint time.
///
/// Currently includes:
/// - **`force-rls-without-policies`** — fires when a table has FORCE ROW LEVEL
///   SECURITY but no policies defined.
/// - **`publication-feature-requires-pg-version`** — fires (Error) when source
///   uses a PG 15+ publication feature with `min_pg_version < 15`.
/// - **`subscription-references-undeclared-publication`** — fires (Warning) when
///   a source subscription's PUBLICATION list names a publication not declared
///   in source.
/// - **`subscription-feature-requires-pg-version`** — fires (Error) when source
///   uses a PG-version-gated subscription option below `min_pg_version`.
/// - **`subscription-password-in-source`** — fires (Error) when a source
///   subscription CONNECTION contains a plaintext `password=` value instead of
///   a `${ENV_VAR}` reference.
/// - **`column-references-unmanaged-collation`** — fires (Warning) when a
///   column / domain / range / composite-attribute references a collation that
///   is neither a Postgres built-in nor declared in source.
/// - **`range-type-references-unmanaged-subtype`** — fires (Warning) when a
///   `CREATE TYPE … AS RANGE` declares a subtype that is neither a known
///   Postgres built-in scalar nor a managed user-defined type.
/// - **`nondeterministic-collation-requires-pg-12`** — fires (Error) when a
///   source collation has `deterministic = false` and `min_pg_version < 12`.
/// - **`builtin-provider-requires-pg-17`** — fires (Error) when a source
///   collation uses `provider = builtin` and `min_pg_version < 17`.
pub fn check_plan_time_catalog(source: &Catalog, min_pg_version: u32) -> Vec<Finding> {
    let mut out = rules::force_rls_without_policies::check(source);
    out.extend(rules::publication_feature_requires_pg_version::check(
        source,
        min_pg_version,
    ));
    out.extend(rules::subscription_references_undeclared_publication::check(source));
    out.extend(rules::subscription_feature_requires_pg_version::check(
        source,
        min_pg_version,
    ));
    out.extend(rules::subscription_password_in_source::check(source));
    out.extend(rules::column_references_unmanaged_collation::check(source));
    out.extend(rules::range_type_references_unmanaged_subtype::check(
        source,
    ));
    out.extend(rules::nondeterministic_collation_requires_pg_12::check(
        source,
        min_pg_version,
    ));
    out.extend(rules::builtin_provider_requires_pg_17::check(
        source,
        min_pg_version,
    ));
    out
}

/// Like [`check_universal`] but also runs cluster-aware source-tree lints.
///
/// `cluster_role_names`: the set of role names declared in the linked cluster
/// project's roles/*.sql, or `None` if no `[cluster].project` is configured.
/// When `None`, cluster-aware rules silently no-op so per-DB independence is
/// preserved.
pub fn check_universal_with_cluster(
    tree: &SourceTree,
    managed: &ManagedConfig,
    cluster_role_names: Option<&std::collections::BTreeSet<crate::identifier::Identifier>>,
) -> Vec<Finding> {
    let mut out = check_universal(tree, managed);
    out.extend(rules::grant_references_unknown_role::check(
        &tree.catalog,
        cluster_role_names,
    ));
    out
}

/// Run all cluster changeset-level lint rules. Mirrors [`check_changeset`]
/// for per-DB lints. Takes both `source` (for membership graph context) and
/// `cs` (the pending cluster changeset).
pub fn check_cluster_changeset(
    source: &crate::ir::cluster::catalog::ClusterCatalog,
    cs: &crate::diff::cluster::ClusterChangeSet,
) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(rules::role_loses_superuser::check(cs));
    out.extend(rules::role_membership_cycle::check(source, cs));
    out
}

#[cfg(test)]
mod tests {
    //! Orchestrator-level integration tests. Per-rule tests live in their
    //! respective `crate::lint::rules::<rule>` modules. Tests here exercise
    //! `check_universal` itself — typically asserting that multiple rules
    //! interact correctly (e.g., trigger checks coordinating table + function
    //! presence) or that the orchestrator combines findings as expected.

    use super::*;
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;
    use crate::ir::view::{MaterializedView, View};
    use crate::lint::test_helpers::{empty_tree, id, make_function_bare, make_trigger, qn};
    use crate::parse::normalize_body::NormalizedBody;

    #[test]
    fn trigger_references_managed_table_and_function_no_findings() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "orders"),
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
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });
        c.functions.push(make_function_bare("app", "audit_fn"));
        c.triggers.push(make_trigger(
            "app",
            "trg_orders",
            "app",
            "orders",
            "app",
            "audit_fn",
        ));
        // Extra unrelated objects — just to confirm no false positives.
        c.views.push(View {
            qname: qn("app", "active_orders"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "order_summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let trg_findings: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.rule == "trigger-references-unmanaged-table"
                    || f.rule == "trigger-references-unmanaged-function"
            })
            .collect();
        assert!(
            trg_findings.is_empty(),
            "expected no trigger lint findings when table and function are managed: {trg_findings:?}",
        );
    }
}
