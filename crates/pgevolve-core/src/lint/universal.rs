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

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::ManagedConfig;
use super::finding::Finding;
use super::source_tree::{ObjectKey, SourceTree};
use crate::ir::catalog::Catalog;
use crate::ir::constraint::ConstraintKind;
use crate::ir::function::FunctionLanguage;
use crate::ir::index::IndexParent;
use crate::ir::user_type::UserTypeKind;
use crate::plan::edges::{DepEdge, DepSource, NodeId};

/// Built-in `PostgreSQL` schemas that are never managed by pgevolve but are
/// always valid targets for cross-schema references.
const BUILTIN_SCHEMAS: &[&str] = &["pg_catalog", "information_schema"];

/// Run every universal rule.
pub fn check_universal(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(managed_schemas_match(tree, managed));
    out.extend(no_duplicate_qnames(tree));
    out.extend(closed_world_references(tree));
    out.extend(view_shadows_table(tree));
    out.extend(mv_no_unique_index(tree));
    out.extend(view_body_references_unmanaged_schema(tree, managed));
    out.extend(type_shadows_table_rule(tree));
    out.extend(enum_value_collision_rule(tree));
    out.extend(composite_attribute_collision_rule(tree));
    out.extend(domain_check_references_unmanaged_type_rule(tree, managed));
    out.extend(pl_pgsql_dynamic_sql_rule(tree));
    out.extend(procedure_contains_commit_rule(tree));
    out.extend(function_references_unmanaged_schema_rule(tree, managed));
    out.extend(extension_version_unpinned(tree));
    out.extend(extension_references_unmanaged_schema(tree));
    out
}

fn managed_schemas_match(tree: &SourceTree, managed: &ManagedConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    let in_source: HashSet<_> = tree
        .catalog
        .schemas
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let in_config: HashSet<_> = managed.schemas.iter().cloned().collect();

    // managed.schemas may be empty (some projects don't list them and rely
    // on filter at apply time). Only flag mismatches when the user has
    // explicitly populated the list.
    if in_config.is_empty() {
        return out;
    }
    for s in &in_source {
        if !in_config.contains(s) {
            out.push(Finding::error(
                "managed_schemas_match",
                format!(
                    "schema `{s}` is declared in source but not listed in `[managed].schemas`",
                ),
            ));
        }
    }
    for s in &in_config {
        if !in_source.contains(s) {
            out.push(Finding::error(
                "managed_schemas_match",
                format!(
                    "schema `{s}` is listed in `[managed].schemas` but not declared in source",
                ),
            ));
        }
    }
    out
}

/// Duplicate qnames are already rejected by `parse_directory`; this is a
/// belt-and-suspenders check that confirms the same invariant on any
/// `SourceTree` regardless of how it was constructed.
fn no_duplicate_qnames(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen: HashSet<&ObjectKey> = HashSet::new();
    for key in tree.objects() {
        if !seen.insert(key) {
            out.push(Finding::error(
                "no_duplicate_qnames",
                format!("duplicate object: {key}"),
            ));
        }
    }
    out
}

fn closed_world_references(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let table_names: HashSet<_> = tree
        .catalog
        .tables
        .iter()
        .map(|t| t.qname.clone())
        .collect();

    // T10: also build an MV name set so that MV-parent indexes are accepted.
    let mv_names: HashSet<_> = tree
        .catalog
        .materialized_views
        .iter()
        .map(|mv| mv.qname.clone())
        .collect();

    for table in &tree.catalog.tables {
        for c in &table.constraints {
            if let ConstraintKind::ForeignKey(fk) = &c.kind
                && !table_names.contains(&fk.referenced_table)
            {
                let loc = tree
                    .object_locations
                    .get(&ObjectKey::Table(table.qname.clone()))
                    .cloned();
                let mut f = Finding::error(
                    "closed_world_references",
                    format!(
                        "FK `{constraint}` on `{owner}` references unknown table `{ref_table}`",
                        constraint = c.qname.name,
                        owner = table.qname,
                        ref_table = fk.referenced_table,
                    ),
                );
                if let Some(l) = loc {
                    f = f.at(l);
                }
                out.push(f);
            }
        }
    }

    // Indexes' parent references (table or MV). T10: branch on parent kind so
    // MV-parent indexes are validated against the MV set, not the table set,
    // closing the false-positive gap noted in the pre-T10 TODO.
    for idx in &tree.catalog.indexes {
        let parent_known = match &idx.on {
            IndexParent::Table(q) => table_names.contains(q),
            IndexParent::Mv(q) => mv_names.contains(q),
        };
        if !parent_known {
            let parent_kind = if idx.on.is_mv() {
                "materialized view"
            } else {
                "table"
            };
            let mut f = Finding::error(
                "closed_world_references",
                format!(
                    "index `{idx}` references unknown {parent_kind} `{tbl}`",
                    idx = idx.qname,
                    tbl = idx.on.qname(),
                ),
            );
            if let Some(loc) = tree
                .object_locations
                .get(&ObjectKey::Index(idx.qname.clone()))
            {
                f = f.at(loc.clone());
            }
            out.push(f);
        }
    }

    // Sequences' OWNED BY references.
    for seq in &tree.catalog.sequences {
        if let Some(owner) = &seq.owned_by
            && !table_names.contains(&owner.table)
        {
            let mut f = Finding::error(
                "closed_world_references",
                format!(
                    "sequence `{seq}` is OWNED BY unknown table `{tbl}`",
                    seq = seq.qname,
                    tbl = owner.table,
                ),
            );
            if let Some(loc) = tree
                .object_locations
                .get(&ObjectKey::Sequence(seq.qname.clone()))
            {
                f = f.at(loc.clone());
            }
            out.push(f);
        }
    }

    out
}

/// `view-shadows-table` — fires when a view or materialized view shares a
/// qname with a table in the same catalog. `PostgreSQL` itself would reject the
/// conflict at apply time; the lint catches it earlier.
fn view_shadows_table(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let table_names: HashSet<_> = tree
        .catalog
        .tables
        .iter()
        .map(|t| t.qname.clone())
        .collect();

    for v in &tree.catalog.views {
        if table_names.contains(&v.qname) {
            out.push(Finding::error(
                "view-shadows-table",
                format!(
                    "view `{q}` has the same name as a table — PostgreSQL would reject this",
                    q = v.qname,
                ),
            ));
        }
    }

    for mv in &tree.catalog.materialized_views {
        if table_names.contains(&mv.qname) {
            out.push(Finding::error(
                "view-shadows-table",
                format!(
                    "materialized view `{q}` has the same name as a table — PostgreSQL would reject this",
                    q = mv.qname,
                ),
            ));
        }
    }

    out
}

/// `mv-no-unique-index` — fires when a materialized view has no unique index,
/// making `REFRESH MATERIALIZED VIEW CONCURRENTLY` unavailable. Plain `REFRESH`
/// blocks reads for the duration of the refresh.
fn mv_no_unique_index(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for mv in &tree.catalog.materialized_views {
        let has_unique = tree
            .catalog
            .indexes
            .iter()
            .any(|idx| idx.unique && matches!(&idx.on, IndexParent::Mv(q) if q == &mv.qname));

        if !has_unique {
            out.push(Finding::warning(
                "mv-no-unique-index",
                format!(
                    "MV `{q}` has no unique index — REFRESH MATERIALIZED VIEW CONCURRENTLY is \
                     unavailable; plain REFRESH will block reads",
                    q = mv.qname,
                ),
            ));
        }
    }

    out
}

/// `view-body-references-unmanaged-schema` — fires when any dependency edge in
/// a view's `body_dependencies` targets a schema that is neither in
/// `[managed].schemas` nor a `PostgreSQL` built-in schema (`pg_catalog`,
/// `information_schema`).
fn view_body_references_unmanaged_schema(
    tree: &SourceTree,
    managed: &ManagedConfig,
) -> Vec<Finding> {
    // If the user has not populated [managed].schemas, we can't meaningfully
    // determine what is "unmanaged" — mirror managed_schemas_match's behaviour.
    if managed.schemas.is_empty() {
        return Vec::new();
    }

    let managed_set: HashSet<&str> = managed
        .schemas
        .iter()
        .map(crate::identifier::Identifier::as_str)
        .collect();

    let mut out = Vec::new();

    let check_deps = |view_qname: &crate::identifier::QualifiedName,
                      deps: &[crate::plan::edges::DepEdge],
                      out: &mut Vec<Finding>| {
        for edge in deps {
            let target_schema = match &edge.to {
                NodeId::Table(q)
                | NodeId::View(q)
                | NodeId::Mv(q)
                | NodeId::Index(q)
                | NodeId::Sequence(q)
                | NodeId::Type(q)
                | NodeId::Procedure(q)
                | NodeId::Function(q, _) => q.schema.as_str(),
                NodeId::Schema(s) | NodeId::Extension(s) => s.as_str(),
                NodeId::Constraint { table, .. } => table.schema.as_str(),
            };

            if BUILTIN_SCHEMAS.contains(&target_schema) {
                continue;
            }
            if managed_set.contains(target_schema) {
                continue;
            }

            out.push(Finding::warning(
                "view-body-references-unmanaged-schema",
                format!(
                    "view `{view_qname}` body depends on schema `{target_schema}` which is not \
                     in [managed].schemas",
                ),
            ));
        }
    };

    for v in &tree.catalog.views {
        check_deps(&v.qname, &v.body_dependencies, &mut out);
    }
    for mv in &tree.catalog.materialized_views {
        check_deps(&mv.qname, &mv.body_dependencies, &mut out);
    }

    out
}

// ── user-type rules ───────────────────────────────────────────────────────────

/// `type-shadows-table` — fires when a user-defined type's qname collides with
/// a table, view, or materialized-view qname. `PostgreSQL` uses one namespace for
/// relations and types, so the conflict would be rejected at apply time.
fn type_shadows_table_rule(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    // Build a set of all relation qnames (table + view + MV).
    let mut relation_names: HashSet<crate::identifier::QualifiedName> = HashSet::new();
    for t in &tree.catalog.tables {
        relation_names.insert(t.qname.clone());
    }
    for v in &tree.catalog.views {
        relation_names.insert(v.qname.clone());
    }
    for mv in &tree.catalog.materialized_views {
        relation_names.insert(mv.qname.clone());
    }

    for ty in &tree.catalog.types {
        if relation_names.contains(&ty.qname) {
            out.push(Finding::error(
                "type-shadows-table",
                format!(
                    "type `{q}` has the same qualified name as an existing relation \
                     (table, view, or materialized view) — PostgreSQL would reject this",
                    q = ty.qname,
                ),
            ));
        }
    }

    out
}

/// `enum-value-collision` — fires when an enum type has duplicate value labels.
///
/// The source parser rejects duplicates at parse time, so this is a
/// defense-in-depth check for catalogs constructed programmatically.
fn enum_value_collision_rule(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for ty in &tree.catalog.types {
        if let UserTypeKind::Enum { values } = &ty.kind {
            let mut seen: HashSet<&str> = HashSet::new();
            for v in values {
                if !seen.insert(v.name.as_str()) {
                    out.push(Finding::error(
                        "enum-value-collision",
                        format!(
                            "enum `{q}` has duplicate value `{label}`",
                            q = ty.qname,
                            label = v.name,
                        ),
                    ));
                }
            }
        }
    }

    out
}

/// `composite-attribute-collision` — fires when a composite type has duplicate
/// attribute names.
///
/// The source parser rejects duplicates at parse time, so this is a
/// defense-in-depth check for catalogs constructed programmatically.
fn composite_attribute_collision_rule(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for ty in &tree.catalog.types {
        if let UserTypeKind::Composite { attributes } = &ty.kind {
            let mut seen: HashSet<&str> = HashSet::new();
            for attr in attributes {
                if !seen.insert(attr.name.as_str()) {
                    out.push(Finding::error(
                        "composite-attribute-collision",
                        format!(
                            "composite type `{q}` has duplicate attribute `{attr}`",
                            q = ty.qname,
                            attr = attr.name.as_str(),
                        ),
                    ));
                }
            }
        }
    }

    out
}

/// Extract all `schema.name` qualified-identifier pairs from a SQL expression
/// text. Returns `(schema, name)` pairs for any token sequence of the form
/// `<identifier>.<identifier>` found in `text`.
fn extract_qualified_refs(text: &str) -> Vec<(String, String)> {
    // Tokenize: split on whitespace and punctuation, keeping only identifier
    // characters (letters, digits, underscore) and dots. Then scan for
    // consecutive tokens of the form `<word>.<word>`.
    let mut result = Vec::new();
    // Walk through the text looking for word.word patterns.
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // Skip non-identifier characters.
        if !is_id_start(bytes[i]) {
            i += 1;
            continue;
        }
        // Consume the first identifier.
        let start = i;
        while i < len && is_id_char(bytes[i]) {
            i += 1;
        }
        let first = &text[start..i];
        // Look for a dot immediately following.
        if i < len && bytes[i] == b'.' {
            i += 1; // consume dot
            if i < len && is_id_start(bytes[i]) {
                let start2 = i;
                while i < len && is_id_char(bytes[i]) {
                    i += 1;
                }
                let second = &text[start2..i];
                result.push((first.to_string(), second.to_string()));
            }
        }
    }
    result
}

#[inline]
const fn is_id_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

#[inline]
const fn is_id_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `domain-check-references-unmanaged-type` — fires (Warning) when a domain's
/// CHECK constraint expression text contains a `schema.name` reference where the
/// schema is neither in `[managed].schemas` nor a `PostgreSQL` built-in schema.
///
/// This is a forward-looking check (full resolution lands in v0.3 when
/// functions are supported), using simple text-based extraction of qualified
/// identifiers from the canonical expression text.
fn domain_check_references_unmanaged_type_rule(
    tree: &SourceTree,
    managed: &ManagedConfig,
) -> Vec<Finding> {
    // If [managed].schemas is empty we cannot determine what is "unmanaged".
    if managed.schemas.is_empty() {
        return Vec::new();
    }

    let managed_set: HashSet<&str> = managed
        .schemas
        .iter()
        .map(crate::identifier::Identifier::as_str)
        .collect();

    let mut out = Vec::new();

    for ty in &tree.catalog.types {
        let UserTypeKind::Domain {
            check_constraints, ..
        } = &ty.kind
        else {
            continue;
        };

        for check in check_constraints {
            let refs = extract_qualified_refs(&check.expression.canonical_text);
            for (schema, _name) in refs {
                if BUILTIN_SCHEMAS.contains(&schema.as_str()) {
                    continue;
                }
                if managed_set.contains(schema.as_str()) {
                    continue;
                }
                out.push(Finding::warning(
                    "domain-check-references-unmanaged-type",
                    format!(
                        "domain `{q}` CHECK constraint `{chk}` references schema `{schema}` \
                         which is not in [managed].schemas",
                        q = ty.qname,
                        chk = check.name.as_str(),
                    ),
                ));
                // One warning per check constraint per unmanaged schema is
                // sufficient — break after the first unmanaged reference.
                break;
            }
        }
    }

    out
}

// ── routine lint rules ────────────────────────────────────────────────────────

/// `pl-pgsql-dynamic-sql` — fires (Error) when a PL/pgSQL function or
/// procedure body contains `EXECUTE` (dynamic SQL) but has no
/// `-- @pgevolve dep: <qname>` directive (`DepSource::AstDeclared` edge).
///
/// Dynamic SQL bypasses static analysis. Developers must annotate every
/// dynamic reference with a directive so pgevolve can maintain the dependency
/// graph correctly.
fn pl_pgsql_dynamic_sql_rule(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for f in &tree.catalog.functions {
        if !matches!(f.language, FunctionLanguage::PlPgSql) {
            continue;
        }
        check_dynamic_sql_in_routine(
            f.body.canonical_text(),
            &f.body_dependencies,
            &f.qname.to_string(),
            "function",
            &mut out,
        );
    }

    for p in &tree.catalog.procedures {
        if !matches!(p.language, FunctionLanguage::PlPgSql) {
            continue;
        }
        check_dynamic_sql_in_routine(
            p.body.canonical_text(),
            &p.body_dependencies,
            &p.qname.to_string(),
            "procedure",
            &mut out,
        );
    }

    out
}

/// Shared inner check for `pl-pgsql-dynamic-sql` — called for both functions
/// and procedures.
fn check_dynamic_sql_in_routine(
    body_text: &str,
    deps: &[DepEdge],
    label: &str,
    kind: &str,
    out: &mut Vec<Finding>,
) {
    let text_lower = body_text.to_lowercase();
    let has_dynamic = text_lower.contains("execute ") || text_lower.contains("execute(");
    if !has_dynamic {
        return;
    }
    let has_directive = deps
        .iter()
        .any(|d| matches!(d.source, DepSource::AstDeclared));
    if !has_directive {
        out.push(Finding::error(
            "pl-pgsql-dynamic-sql",
            format!(
                "{kind} `{label}` contains dynamic SQL (EXECUTE) but has no \
                 `-- @pgevolve dep: <qname>` directive. Add at least one directive \
                 to declare what the dynamic SQL references.",
            ),
        ));
    }
}

/// `procedure-contains-commit` — fires (Warning) when a procedure's
/// `commits_in_body` flag is true. Informational: pgevolve will run the step
/// with `transactional=OutsideTransaction`. Surfaces in code review so
/// reviewers know the step cannot be rolled back as part of a larger migration
/// transaction.
fn procedure_contains_commit_rule(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();

    for p in &tree.catalog.procedures {
        if p.commits_in_body {
            out.push(Finding::warning(
                "procedure-contains-commit",
                format!(
                    "procedure `{}` body contains COMMIT/ROLLBACK; pgevolve will run \
                     this step with transactional=OutsideTransaction.",
                    p.qname,
                ),
            ));
        }
    }

    out
}

/// `function-references-unmanaged-schema` — fires (Warning) when any
/// dependency edge in a function or procedure `body_dependencies` targets a
/// schema that is neither in `[managed].schemas` nor a `PostgreSQL` built-in
/// schema (`pg_catalog`, `information_schema`).
///
/// Mirrors `view-body-references-unmanaged-schema` for routines.
fn function_references_unmanaged_schema_rule(
    tree: &SourceTree,
    managed: &ManagedConfig,
) -> Vec<Finding> {
    // If the user has not populated [managed].schemas we cannot determine
    // what is "unmanaged" — mirror managed_schemas_match's behaviour.
    if managed.schemas.is_empty() {
        return Vec::new();
    }

    let managed_set: HashSet<&str> = managed
        .schemas
        .iter()
        .map(crate::identifier::Identifier::as_str)
        .collect();

    let mut out = Vec::new();

    let check_deps = |routine_qname: &crate::identifier::QualifiedName,
                      kind: &str,
                      deps: &[DepEdge],
                      out: &mut Vec<Finding>| {
        for edge in deps {
            let target_schema = match &edge.to {
                NodeId::Table(q)
                | NodeId::View(q)
                | NodeId::Mv(q)
                | NodeId::Index(q)
                | NodeId::Sequence(q)
                | NodeId::Type(q)
                | NodeId::Procedure(q)
                | NodeId::Function(q, _) => q.schema.as_str(),
                NodeId::Schema(s) | NodeId::Extension(s) => s.as_str(),
                NodeId::Constraint { table, .. } => table.schema.as_str(),
            };

            if BUILTIN_SCHEMAS.contains(&target_schema) {
                continue;
            }
            if managed_set.contains(target_schema) {
                continue;
            }

            out.push(Finding::warning(
                "function-references-unmanaged-schema",
                format!(
                    "{kind} `{routine_qname}` body depends on schema `{target_schema}` \
                     which is not in [managed].schemas",
                ),
            ));
        }
    };

    for f in &tree.catalog.functions {
        check_deps(&f.qname, "function", &f.body_dependencies, &mut out);
    }
    for p in &tree.catalog.procedures {
        check_deps(&p.qname, "procedure", &p.body_dependencies, &mut out);
    }

    out
}

// ── extension lint rules ──────────────────────────────────────────────────────

/// `extension-version-unpinned` — fires when a source-declared extension
/// has no `VERSION` clause. Unpinned extensions can shift between
/// environments; pinning ensures dev and prod install the same version.
fn extension_version_unpinned(tree: &SourceTree) -> Vec<Finding> {
    let mut out = Vec::new();
    for e in &tree.catalog.extensions {
        if e.version.is_none() {
            out.push(Finding::warning(
                "extension-version-unpinned",
                format!(
                    "{}: extension is declared without a VERSION clause. Pinning the version \
                     ensures the same version is installed across environments.",
                    e.name,
                ),
            ));
        }
    }
    out
}

/// `extension-references-unmanaged-schema` — fires when a CREATE EXTENSION
/// references a schema not in the source catalog. Without the schema
/// being managed, the planner can't guarantee ordering.
fn extension_references_unmanaged_schema(tree: &SourceTree) -> Vec<Finding> {
    let managed_schemas: std::collections::BTreeSet<&str> =
        tree.catalog.schemas.iter().map(|s| s.name.as_str()).collect();
    let mut out = Vec::new();
    for e in &tree.catalog.extensions {
        if let Some(schema) = &e.schema
            && !managed_schemas.contains(schema.as_str())
        {
            out.push(Finding::error(
                "extension-references-unmanaged-schema",
                format!(
                    "{}: WITH SCHEMA {} references a schema not declared in source. \
                     Add a CREATE SCHEMA {} to the source or remove the WITH SCHEMA clause.",
                    e.name, schema, schema,
                ),
            ));
        }
    }
    out
}

/// All rule IDs that carry [`Severity::LintAtPlan`].
///
/// Preflight uses this to warn about waivers that reference unknown rule IDs
/// (typos, stale waivers for renamed rules). Add new `LintAtPlan` rule IDs
/// here when introducing them; remove stale ones when a rule is deleted.
pub const LINT_AT_PLAN_RULES: &[&str] = &["column-position-drift"];

/// Run all drift-detection rules that compare `source` against a `target`
/// catalog (e.g. the live database). Returns a list of [`Finding`]s.
///
/// This is the entry point for lint rules that need both the source and target
/// catalogs, as opposed to [`check_universal`] which only needs the source tree.
pub fn run_drift_lints(source: &Catalog, target: &Catalog) -> Vec<Finding> {
    let mut out = Vec::new();
    column_position_drift_rule(source, target, &mut out);
    out
}

/// `column-position-drift` — fires when a table's column order in source
/// disagrees with the target catalog's column order, with no other structural
/// change accompanying that column.
///
/// Source is canonical; the lint says "your DB has columns in a different
/// order." Severity is `LintAtPlan` — plan refuses unless the finding is
/// waived in `intent.toml` (waiver mechanism in Task 8).
fn column_position_drift_rule(source: &Catalog, target: &Catalog, out: &mut Vec<Finding>) {
    let target_tables: BTreeMap<_, _> =
        target.tables.iter().map(|t| (t.qname.clone(), t)).collect();

    for source_table in &source.tables {
        let Some(target_table) = target_tables.get(&source_table.qname) else {
            continue;
        };
        let source_names: Vec<_> = source_table
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let target_names: Vec<_> = target_table
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect();

        // Only compare columns that exist in both catalogs. Added or removed
        // columns do not constitute position drift — those are handled by the
        // planner.
        let source_set: BTreeSet<_> = source_names.iter().cloned().collect();
        let target_set: BTreeSet<_> = target_names.iter().cloned().collect();
        let common: BTreeSet<_> = source_set.intersection(&target_set).cloned().collect();

        let source_order: Vec<_> = source_names.iter().filter(|n| common.contains(n)).collect();
        let target_order: Vec<_> = target_names.iter().filter(|n| common.contains(n)).collect();

        if source_order != target_order {
            out.push(Finding::lint_at_plan(
                "column-position-drift",
                format!(
                    "{}: column position drift. source order [{}] vs catalog order [{}]",
                    source_table.qname,
                    source_order
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    target_order
                        .iter()
                        .map(|n| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ));
        }
    }
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
            comment: None,
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
            comment: None,
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
            comment: None,
        });
        // MV with the same qname as the table.
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "orders"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
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
            comment: None,
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
        });
        c.materialized_views.push(MaterializedView {
            qname: qn("app", "user_summary"),
            columns: vec![],
            body_canonical: NormalizedBody::from_sql("SELECT 1").unwrap(),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
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
            Index, IndexColumn, IndexColumnExpr, IndexMethod, NullsOrder, SortOrder,
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
            Index, IndexColumn, IndexColumnExpr, IndexMethod, NullsOrder, SortOrder,
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
            comment: None,
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
            comment: None,
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
            comment: None,
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
        let refs = super::extract_qualified_refs("value > 0 and external.validate_int(value)");
        assert!(
            refs.contains(&("external".to_string(), "validate_int".to_string())),
            "should extract external.validate_int: {refs:?}",
        );
    }

    #[test]
    fn extract_qualified_refs_empty_text() {
        let refs = super::extract_qualified_refs("value > 0");
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
}
