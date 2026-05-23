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
/// Currently includes:
/// - **`force-rls-without-policies`** — fires when a table has FORCE ROW LEVEL
///   SECURITY but no policies defined.
pub fn check_plan_time_catalog(source: &Catalog) -> Vec<Finding> {
    rules::force_rls_without_policies::check(source)
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
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::catalog::Catalog;
    use crate::ir::constraint::{
        Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
    };
    use crate::ir::schema::Schema;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(s: &str, n: &str) -> QualifiedName {
        QualifiedName::new(id(s), id(n))
    }

    fn empty_tree(catalog: Catalog) -> SourceTree {
        SourceTree::new(catalog, std::collections::HashMap::new())
    }

    #[test]
    fn managed_schemas_match_passes_when_lists_align() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        let tree = empty_tree(c);
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check_universal(&tree, &managed);
        assert!(f.is_empty(), "got findings: {f:?}");
    }

    #[test]
    fn managed_schemas_match_flags_missing_source_schema() {
        let tree = empty_tree(Catalog::empty());
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check_universal(&tree, &managed);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "managed_schemas_match");
    }

    #[test]
    fn managed_schemas_match_flags_unlisted_source_schema() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("audit")));
        let tree = empty_tree(c);
        let managed = ManagedConfig {
            schemas: vec![id("app")],
        };
        let f = check_universal(&tree, &managed);
        // Two findings: app missing in source, audit not listed in managed.
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn managed_schemas_match_skips_when_managed_is_empty() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("any")));
        let tree = empty_tree(c);
        let managed = ManagedConfig::default();
        let f = check_universal(&tree, &managed);
        // Other universal rules may still fire, but managed_schemas_match
        // must be silent.
        assert!(f.iter().all(|x| x.rule != "managed_schemas_match"));
    }

    #[test]
    fn closed_world_references_flags_dangling_fk() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![],
            constraints: vec![Constraint {
                qname: qn("app", "users_fk"),
                kind: ConstraintKind::ForeignKey(ForeignKey {
                    columns: vec![id("orgs_id")],
                    referenced_table: qn("app", "orgs"), // doesn't exist
                    referenced_columns: vec![id("id")],
                    on_update: ReferentialAction::NoAction,
                    on_delete: ReferentialAction::NoAction,
                    match_type: FkMatchType::Simple,
                }),
                deferrable: Deferrable::NotDeferrable,
                comment: None,
            }],
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
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count_cwr = findings
            .iter()
            .filter(|f| f.rule == "closed_world_references")
            .count();
        assert_eq!(count_cwr, 1);
    }

    // ── view-shadows-table ────────────────────────────────────────────────────

    #[test]
    fn view_shadows_table_fires_when_view_collides_with_table() {
        use crate::ir::view::View;
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
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
        // View with the same qname as the table.
        c.views.push(View {
            qname: qn("app", "users"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-shadows-table")
            .count();
        assert_eq!(count, 1, "expected exactly one view-shadows-table finding");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "view-shadows-table")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn view_shadows_table_fires_when_mv_collides_with_table() {
        use crate::ir::view::MaterializedView;
        use crate::parse::normalize_body::NormalizedBody;

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
        // MV with the same qname as the table.
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "orders"),
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
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-shadows-table")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one view-shadows-table finding for MV"
        );
    }

    #[test]
    fn view_shadows_table_clean_catalog_passes() {
        use crate::ir::view::{MaterializedView, View};
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
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
        c.views.push(View {
            qname: qn("app", "active_users"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "user_summary"),
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
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-shadows-table")
            .count();
        assert_eq!(
            count, 0,
            "expected no view-shadows-table findings on clean catalog"
        );
    }

    // ── mv-no-unique-index ────────────────────────────────────────────────────

    #[test]
    fn mv_no_unique_index_fires_when_mv_has_no_unique_index() {
        use crate::ir::index::{
            Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
        };
        use crate::ir::view::MaterializedView;
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        // Non-unique index on the MV — should still trigger the warning.
        c.indexes.push(Index {
            qname: qn("app", "summary_idx"),
            on: IndexParent::Mv(qn("app", "summary")),
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
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "mv-no-unique-index")
            .count();
        assert_eq!(count, 1, "expected one mv-no-unique-index warning");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "mv-no-unique-index")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn mv_no_unique_index_passes_when_unique_index_present() {
        use crate::ir::index::{
            Index, IndexColumn, IndexColumnExpr, IndexMethod, IndexParent, NullsOrder, SortOrder,
        };
        use crate::ir::view::MaterializedView;
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        });
        // Unique index on the MV — rule must NOT fire.
        c.indexes.push(Index {
            qname: qn("app", "summary_id_uidx"),
            on: IndexParent::Mv(qn("app", "summary")),
            method: IndexMethod::BTree,
            columns: vec![IndexColumn {
                expr: IndexColumnExpr::Column(id("id")),
                collation: None,
                opclass: None,
                sort_order: SortOrder::Asc,
                nulls_order: NullsOrder::NullsLast,
            }],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
            storage: crate::ir::reloptions::IndexStorageOptions::default(),
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "mv-no-unique-index")
            .count();
        assert_eq!(
            count, 0,
            "expected no mv-no-unique-index findings when unique index present"
        );
    }

    #[test]
    fn mv_no_unique_index_clean_catalog_no_mvs() {
        // Catalog with only tables — rule must stay silent.
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
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
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        assert!(
            findings.iter().all(|f| f.rule != "mv-no-unique-index"),
            "mv-no-unique-index must not fire on a catalog with no MVs",
        );
    }

    // ── view-body-references-unmanaged-schema ─────────────────────────────────

    #[test]
    fn view_body_references_unmanaged_schema_fires_when_dep_in_unmanaged_schema() {
        use crate::ir::view::View;
        use crate::parse::normalize_body::NormalizedBody;
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::View(qn("app", "my_view")),
                // references table in "external" schema — not managed
                to: NodeId::Table(qn("external", "data")),
                source: DepSource::AstExtracted,
            }],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        // managed only has "app" — "external" is unmanaged
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "view-body-references-unmanaged-schema")
            .count();
        assert_eq!(
            count, 1,
            "expected one view-body-references-unmanaged-schema warning"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "view-body-references-unmanaged-schema")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn view_body_references_unmanaged_schema_silent_on_managed_dep() {
        use crate::ir::view::View;
        use crate::parse::normalize_body::NormalizedBody;
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::View(qn("app", "my_view")),
                // references table in the managed "app" schema — fine
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstExtracted,
            }],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "view-body-references-unmanaged-schema"),
            "rule must not fire when dep is in a managed schema",
        );
    }

    #[test]
    fn view_body_references_unmanaged_schema_silent_on_builtin_schemas() {
        use crate::ir::view::View;
        use crate::parse::normalize_body::NormalizedBody;
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![
                DepEdge {
                    from: NodeId::View(qn("app", "my_view")),
                    to: NodeId::Table(qn("pg_catalog", "pg_type")),
                    source: DepSource::AstExtracted,
                },
                DepEdge {
                    from: NodeId::View(qn("app", "my_view")),
                    to: NodeId::Table(qn("information_schema", "columns")),
                    source: DepSource::AstExtracted,
                },
            ],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "view-body-references-unmanaged-schema"),
            "rule must not fire for pg_catalog / information_schema references",
        );
    }

    #[test]
    fn view_body_references_unmanaged_schema_silent_when_managed_is_empty() {
        use crate::ir::view::View;
        use crate::parse::normalize_body::NormalizedBody;
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.views.push(View {
            qname: qn("app", "my_view"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![DepEdge {
                from: NodeId::View(qn("app", "my_view")),
                to: NodeId::Table(qn("anywhere", "stuff")),
                source: DepSource::AstExtracted,
            }],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        // Empty managed config — rule must stay silent (mirrors managed_schemas_match).
        let findings = check_universal(&tree, &ManagedConfig::default());
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "view-body-references-unmanaged-schema"),
            "rule must be silent when [managed].schemas is empty",
        );
    }

    // ── type-shadows-table ────────────────────────────────────────────────────

    #[test]
    fn type_shadows_table_fires_on_collision() {
        use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
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
        // An enum type that collides with the table.
        c.types.push(UserType {
            qname: qn("app", "users"),
            kind: UserTypeKind::Enum {
                values: vec![EnumValue {
                    name: "active".into(),
                    sort_order: 1.0,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "type-shadows-table")
            .count();
        assert_eq!(count, 1, "expected exactly one type-shadows-table finding");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "type-shadows-table")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn type_shadows_table_silent_when_no_collision() {
        use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.tables.push(Table {
            qname: qn("app", "users"),
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
        c.types.push(UserType {
            qname: qn("app", "user_status"),
            kind: UserTypeKind::Enum {
                values: vec![EnumValue {
                    name: "active".into(),
                    sort_order: 1.0,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        assert!(
            findings.iter().all(|f| f.rule != "type-shadows-table"),
            "type-shadows-table must not fire when names are distinct",
        );
    }

    // ── enum-value-collision ──────────────────────────────────────────────────

    #[test]
    fn enum_value_collision_fires() {
        use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "status"),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue {
                        name: "active".into(),
                        sort_order: 1.0,
                    },
                    EnumValue {
                        name: "active".into(), // duplicate label
                        sort_order: 2.0,
                    },
                    EnumValue {
                        name: "inactive".into(),
                        sort_order: 3.0,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "enum-value-collision")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one enum-value-collision finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "enum-value-collision")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn enum_value_collision_silent_on_distinct_values() {
        use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "status"),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue {
                        name: "pending".into(),
                        sort_order: 1.0,
                    },
                    EnumValue {
                        name: "active".into(),
                        sort_order: 2.0,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        assert!(
            findings.iter().all(|f| f.rule != "enum-value-collision"),
            "enum-value-collision must not fire on distinct values",
        );
    }

    // ── composite-attribute-collision ─────────────────────────────────────────

    #[test]
    fn composite_attribute_collision_fires() {
        use crate::ir::column_type::ColumnType;
        use crate::ir::user_type::{CompositeAttribute, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "address"),
            kind: UserTypeKind::Composite {
                attributes: vec![
                    CompositeAttribute {
                        name: id("street"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: id("street"), // duplicate attribute
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: id("city"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "composite-attribute-collision")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one composite-attribute-collision finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "composite-attribute-collision")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn composite_attribute_collision_silent_on_distinct_attributes() {
        use crate::ir::column_type::ColumnType;
        use crate::ir::user_type::{CompositeAttribute, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "address"),
            kind: UserTypeKind::Composite {
                attributes: vec![
                    CompositeAttribute {
                        name: id("street"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: id("city"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "composite-attribute-collision"),
            "composite-attribute-collision must not fire on distinct attributes",
        );
    }

    // ── domain-check-references-unmanaged-type ────────────────────────────────

    #[test]
    fn domain_check_references_unmanaged_type_fires() {
        use crate::ir::column_type::ColumnType;
        use crate::ir::default_expr::NormalizedExpr;
        use crate::ir::user_type::{DomainCheck, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "positive_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: false,
                default: None,
                check_constraints: vec![DomainCheck {
                    name: id("positive_int_check"),
                    // references external.validate_int — schema "external" is not managed
                    expression: NormalizedExpr::from_text(
                        "value > 0 and external.validate_int(value)",
                    ),
                }],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "domain-check-references-unmanaged-type")
            .count();
        assert_eq!(
            count, 1,
            "expected one domain-check-references-unmanaged-type warning"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "domain-check-references-unmanaged-type")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn domain_check_references_unmanaged_type_silent_on_managed_schema() {
        use crate::ir::column_type::ColumnType;
        use crate::ir::default_expr::NormalizedExpr;
        use crate::ir::user_type::{DomainCheck, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "positive_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: false,
                default: None,
                check_constraints: vec![DomainCheck {
                    name: id("positive_int_check"),
                    // references app.validate_int — "app" is managed
                    expression: NormalizedExpr::from_text("app.validate_int(value)"),
                }],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "domain-check-references-unmanaged-type"),
            "rule must not fire when referenced schema is managed",
        );
    }

    #[test]
    fn domain_check_references_unmanaged_type_silent_for_pg_catalog() {
        use crate::ir::column_type::ColumnType;
        use crate::ir::default_expr::NormalizedExpr;
        use crate::ir::user_type::{DomainCheck, UserType, UserTypeKind};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.types.push(UserType {
            qname: qn("app", "text_domain"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Text,
                nullable: false,
                default: None,
                check_constraints: vec![DomainCheck {
                    name: id("not_empty"),
                    // references pg_catalog — built-in, always exempt
                    expression: NormalizedExpr::from_text("pg_catalog.char_length(value) > 0"),
                }],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "domain-check-references-unmanaged-type"),
            "rule must not fire for pg_catalog references",
        );
    }

    #[test]
    fn extract_qualified_refs_basic() {
        let refs = rules::extract_qualified_refs("value > 0 and external.validate_int(value)");
        assert!(
            refs.contains(&("external".to_string(), "validate_int".to_string())),
            "should extract external.validate_int: {refs:?}",
        );
    }

    #[test]
    fn extract_qualified_refs_empty_text() {
        let refs = rules::extract_qualified_refs("value > 0");
        assert!(
            refs.is_empty(),
            "no qualified refs in simple expression: {refs:?}",
        );
    }

    // ── pl-pgsql-dynamic-sql ──────────────────────────────────────────────────

    fn make_plpgsql_function(
        schema: &str,
        name: &str,
        body_text: &str,
        deps: Vec<crate::plan::edges::DepEdge>,
    ) -> crate::ir::function::Function {
        use crate::ir::function::{
            FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType, SecurityMode,
            Volatility,
        };
        use crate::parse::normalize_body::NormalizedBody;
        let args = vec![];
        let arg_types_normalized = NormalizedArgTypes::from_args(&args);
        crate::ir::function::Function {
            qname: qn(schema, name),
            args,
            arg_types_normalized,
            return_type: ReturnType::Void,
            language: FunctionLanguage::PlPgSql,
            // PL/pgSQL bodies can't be parsed by pg_query — use from_raw_canonical.
            body: NormalizedBody::from_raw_canonical(body_text.to_string()),
            body_dependencies: deps,
            volatility: Volatility::Volatile,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Unsafe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    /// Build a zero-arg `NormalizedArgTypes` for use in test `NodeId::Function` variants.
    fn empty_arg_types() -> crate::ir::function::NormalizedArgTypes {
        crate::ir::function::NormalizedArgTypes::from_args(&[])
    }

    fn make_procedure(
        schema: &str,
        name: &str,
        body_text: &str,
        commits_in_body: bool,
        deps: Vec<crate::plan::edges::DepEdge>,
    ) -> crate::ir::procedure::Procedure {
        use crate::ir::function::{FunctionLanguage, SecurityMode};
        use crate::parse::normalize_body::NormalizedBody;
        crate::ir::procedure::Procedure {
            qname: qn(schema, name),
            args: vec![],
            language: FunctionLanguage::PlPgSql,
            // PL/pgSQL bodies can't be parsed by pg_query — use from_raw_canonical.
            body: NormalizedBody::from_raw_canonical(body_text.to_string()),
            body_dependencies: deps,
            security: SecurityMode::Invoker,
            commits_in_body,
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    #[test]
    fn pl_pgsql_dynamic_sql_fires_when_execute_without_directive() {
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Body contains EXECUTE but no AstDeclared dep edge.
        c.functions.push(make_plpgsql_function(
            "app",
            "dyn_fn",
            "BEGIN EXECUTE 'SELECT 1'; END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "dyn_fn"), empty_arg_types()),
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstExtracted, // NOT AstDeclared
            }],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "pl-pgsql-dynamic-sql")
            .count();
        assert_eq!(count, 1, "expected one pl-pgsql-dynamic-sql finding");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "pl-pgsql-dynamic-sql")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn pl_pgsql_dynamic_sql_silent_when_directive_present() {
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Body has EXECUTE + an AstDeclared dep — should be silent.
        c.functions.push(make_plpgsql_function(
            "app",
            "dyn_fn_ok",
            "BEGIN EXECUTE 'SELECT 1'; END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "dyn_fn_ok"), empty_arg_types()),
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstDeclared,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings.iter().all(|f| f.rule != "pl-pgsql-dynamic-sql"),
            "pl-pgsql-dynamic-sql must not fire when directive present",
        );
    }

    #[test]
    fn pl_pgsql_dynamic_sql_fires_for_procedure_without_directive() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Procedure with EXECUTE but no AstDeclared dep.
        c.procedures.push(make_procedure(
            "app",
            "dyn_proc",
            "BEGIN EXECUTE 'DELETE FROM users'; END",
            false,
            vec![],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "pl-pgsql-dynamic-sql")
            .count();
        assert_eq!(
            count, 1,
            "expected one pl-pgsql-dynamic-sql finding for procedure"
        );
    }

    // ── procedure-contains-commit ─────────────────────────────────────────────

    #[test]
    fn procedure_contains_commit_fires_when_commits_in_body_true() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.procedures.push(make_procedure(
            "app",
            "commit_proc",
            "BEGIN COMMIT; END",
            true, // commits_in_body
            vec![],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "procedure-contains-commit")
            .count();
        assert_eq!(count, 1, "expected one procedure-contains-commit warning");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "procedure-contains-commit")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn procedure_contains_commit_silent_when_false() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.procedures.push(make_procedure(
            "app",
            "normal_proc",
            "BEGIN NULL; END",
            false, // no COMMIT
            vec![],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "procedure-contains-commit"),
            "procedure-contains-commit must not fire when commits_in_body=false",
        );
    }

    // ── function-references-unmanaged-schema ──────────────────────────────────

    #[test]
    fn function_references_unmanaged_schema_fires_on_cross_schema_dep() {
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.functions.push(make_plpgsql_function(
            "app",
            "cross_fn",
            "BEGIN RETURN external.helper(); END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "cross_fn"), empty_arg_types()),
                to: NodeId::Function(qn("external", "helper"), empty_arg_types()),
                source: DepSource::AstExtracted,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        let count = findings
            .iter()
            .filter(|f| f.rule == "function-references-unmanaged-schema")
            .count();
        assert_eq!(
            count, 1,
            "expected one function-references-unmanaged-schema warning"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "function-references-unmanaged-schema")
                .unwrap()
                .severity,
            crate::lint::Severity::Warning,
        );
    }

    #[test]
    fn function_references_unmanaged_schema_silent_on_managed_dep() {
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.functions.push(make_plpgsql_function(
            "app",
            "managed_fn",
            "BEGIN RETURN app.helper(); END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "managed_fn"), empty_arg_types()),
                to: NodeId::Function(qn("app", "helper"), empty_arg_types()),
                source: DepSource::AstExtracted,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "function-references-unmanaged-schema"),
            "function-references-unmanaged-schema must not fire when dep is in managed schema",
        );
    }

    #[test]
    fn function_references_unmanaged_schema_silent_on_builtin_schema() {
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.functions.push(make_plpgsql_function(
            "app",
            "catalog_fn",
            "BEGIN RETURN pg_catalog.now(); END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "catalog_fn"), empty_arg_types()),
                to: NodeId::Function(qn("pg_catalog", "now"), empty_arg_types()),
                source: DepSource::AstExtracted,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(
            &tree,
            &ManagedConfig {
                schemas: vec![id("app")],
            },
        );
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "function-references-unmanaged-schema"),
            "function-references-unmanaged-schema must not fire for pg_catalog",
        );
    }

    #[test]
    fn function_references_unmanaged_schema_silent_when_managed_is_empty() {
        use crate::plan::edges::{DepEdge, DepSource, NodeId};

        let mut c = Catalog::empty();
        c.functions.push(make_plpgsql_function(
            "app",
            "any_fn",
            "BEGIN NULL; END",
            vec![DepEdge {
                from: NodeId::Function(qn("app", "any_fn"), empty_arg_types()),
                to: NodeId::Table(qn("external", "data")),
                source: DepSource::AstExtracted,
            }],
        ));
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        assert!(
            findings
                .iter()
                .all(|f| f.rule != "function-references-unmanaged-schema"),
            "function-references-unmanaged-schema must be silent when managed.schemas is empty",
        );
    }

    // ── extension-version-unpinned ────────────────────────────────────────────

    #[test]
    fn extension_version_unpinned_fires_on_unpinned() {
        use crate::ir::extension::Extension;

        let mut c = Catalog::empty();
        c.extensions.push(Extension {
            name: id("pgcrypto"),
            schema: None,
            version: None,
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-version-unpinned")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn extension_version_unpinned_silent_when_pinned() {
        use crate::ir::extension::Extension;

        let mut c = Catalog::empty();
        c.extensions.push(Extension {
            name: id("pgcrypto"),
            schema: None,
            version: Some("1.3".into()),
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-version-unpinned")
            .count();
        assert_eq!(count, 0);
    }

    // ── extension-references-unmanaged-schema ─────────────────────────────────

    #[test]
    fn extension_references_unmanaged_schema_fires() {
        use crate::ir::extension::Extension;

        let mut c = Catalog::empty();
        c.extensions.push(Extension {
            name: id("pg_trgm"),
            schema: Some(id("missing")),
            version: None,
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-references-unmanaged-schema")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn extension_references_managed_schema_silent() {
        use crate::ir::extension::Extension;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        c.extensions.push(Extension {
            name: id("pg_trgm"),
            schema: Some(id("app")),
            version: None,
            comment: None,
        });
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "extension-references-unmanaged-schema")
            .count();
        assert_eq!(count, 0);
    }

    // ── trigger-references-unmanaged-table / trigger-references-unmanaged-function

    fn make_trigger(
        schema: &str,
        name: &str,
        table_schema: &str,
        table_name: &str,
        fn_schema: &str,
        fn_name: &str,
    ) -> crate::ir::trigger::Trigger {
        use crate::ir::constraint::Deferrable;
        use crate::ir::trigger::{TriggerEvent, TriggerLevel, TriggerTiming};
        crate::ir::trigger::Trigger {
            qname: qn(schema, name),
            table: qn(table_schema, table_name),
            timing: TriggerTiming::Before,
            events: vec![TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_clause: None,
            transition_tables: vec![],
            function_qname: qn(fn_schema, fn_name),
            function_args: vec![],
            is_constraint: false,
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn make_function_bare(schema: &str, name: &str) -> crate::ir::function::Function {
        make_plpgsql_function(schema, name, "BEGIN NULL; END", vec![])
    }

    #[test]
    fn trigger_references_managed_table_and_function_no_findings() {
        use crate::ir::view::{MaterializedView, View};
        use crate::parse::normalize_body::NormalizedBody;

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

    #[test]
    fn trigger_references_unmanaged_table_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Function IS managed but the table is not in the catalog.
        c.functions.push(make_function_bare("app", "audit_fn"));
        c.triggers.push(make_trigger(
            "app",
            "trg_missing",
            "app",
            "ghost_table", // not in catalog
            "app",
            "audit_fn",
        ));
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-table")
            .count();
        assert_eq!(
            count, 1,
            "expected one trigger-references-unmanaged-table finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "trigger-references-unmanaged-table")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn trigger_references_unmanaged_function_fires() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // Table IS managed but the function is not.
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
        c.triggers.push(make_trigger(
            "app",
            "trg_no_fn",
            "app",
            "orders",
            "app",
            "missing_fn", // not in catalog
        ));
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-function")
            .count();
        assert_eq!(
            count, 1,
            "expected one trigger-references-unmanaged-function finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "trigger-references-unmanaged-function")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn trigger_on_managed_view_no_finding() {
        use crate::ir::view::View;
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
        // INSTEAD OF triggers fire on views.
        c.views.push(View {
            qname: qn("app", "editable_orders"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        });
        c.functions.push(make_function_bare("app", "audit_fn"));
        c.triggers.push(make_trigger(
            "app",
            "trg_view",
            "app",
            "editable_orders", // view, not a table
            "app",
            "audit_fn",
        ));
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-table")
            .count();
        assert_eq!(
            count, 0,
            "trigger-references-unmanaged-table must not fire when target is a managed view",
        );
    }

    #[test]
    fn trigger_on_managed_mv_no_finding() {
        use crate::ir::view::MaterializedView;
        use crate::parse::normalize_body::NormalizedBody;

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));
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
        c.functions.push(make_function_bare("app", "audit_fn"));
        c.triggers.push(make_trigger(
            "app",
            "trg_mv",
            "app",
            "order_summary", // materialized view
            "app",
            "audit_fn",
        ));
        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "trigger-references-unmanaged-table")
            .count();
        assert_eq!(
            count, 0,
            "trigger-references-unmanaged-table must not fire when target is a managed MV",
        );
    }

    // ── partition-references-unmanaged-parent

    #[test]
    fn partition_with_managed_parent_no_finding() {
        use crate::ir::partition::{
            PartitionBounds, PartitionBy, PartitionColumn, PartitionColumnKind, PartitionOf,
            PartitionStrategy,
        };

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));

        // Parent table with PARTITION BY clause.
        c.tables.push(Table {
            qname: qn("app", "orders"),
            columns: vec![],
            constraints: vec![],
            partition_by: Some(PartitionBy {
                strategy: PartitionStrategy::Range,
                columns: vec![PartitionColumn {
                    kind: PartitionColumnKind::Column(id("id")),
                    collation: None,
                    opclass: None,
                }],
            }),
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        // Child partition referencing the parent.
        c.tables.push(Table {
            qname: qn("app", "orders_p1"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: Some(PartitionOf {
                parent: qn("app", "orders"),
                bounds: PartitionBounds::Default,
            }),
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "partition-references-unmanaged-parent")
            .count();
        assert_eq!(
            count, 0,
            "expected no partition-references-unmanaged-parent finding when parent is managed"
        );
    }

    #[test]
    fn partition_with_unmanaged_parent_fires() {
        use crate::ir::partition::{PartitionBounds, PartitionOf};

        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));

        // Child partition references a parent NOT in the catalog.
        c.tables.push(Table {
            qname: qn("app", "orders_p1"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: Some(PartitionOf {
                parent: qn("app", "ghost_orders"),
                bounds: PartitionBounds::Default,
            }),
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
        });

        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "partition-references-unmanaged-parent")
            .count();
        assert_eq!(
            count, 1,
            "expected one partition-references-unmanaged-parent finding"
        );
        assert_eq!(
            findings
                .iter()
                .find(|f| f.rule == "partition-references-unmanaged-parent")
                .unwrap()
                .severity,
            crate::lint::Severity::Error,
        );
    }

    #[test]
    fn non_partition_tables_ignored() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema::new(id("app")));

        // Regular table without partition_of.
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

        let tree = empty_tree(c);
        let findings = check_universal(&tree, &ManagedConfig::default());
        let count = findings
            .iter()
            .filter(|f| f.rule == "partition-references-unmanaged-parent")
            .count();
        assert_eq!(
            count, 0,
            "partition-references-unmanaged-parent must not fire for regular tables"
        );
    }
}
