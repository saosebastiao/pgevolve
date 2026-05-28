//! Per-rule modules for the universal lint engine.
//!
//! Each sub-module contains exactly one rule function (named `check`) plus its
//! inline `#[cfg(test)] mod tests` block.  Shared helpers live here in `mod.rs`.
//! This module is `pub(crate)` from `lint::` so all items are effectively
//! crate-internal despite being marked `pub`.

pub mod closed_world_references;
pub mod column_position_drift;
pub mod composite_attribute_collision;
pub mod compression_change_not_retroactive;
pub mod domain_check_references_unmanaged_type;
pub mod enum_value_collision;
pub mod extension_references_unmanaged_schema;
pub mod extension_version_unpinned;
pub mod force_rls_without_policies;
pub mod function_references_unmanaged_schema;
pub mod grant_references_unknown_role;
pub mod grants_to_unmanaged_role;
pub mod managed_schemas_match;
pub mod mv_no_unique_index;
pub mod no_duplicate_qnames;
pub mod partition_references_unmanaged_parent;
pub mod pl_pgsql_dynamic_sql;
pub mod procedure_contains_commit;
pub mod publication_captures_unmanaged_table;
pub mod publication_feature_requires_pg_version;
pub mod publication_row_filter_references_unmanaged_column;
pub mod revoke_from_owner;
pub mod role_loses_superuser;
pub mod role_membership_cycle;
pub mod storage_downgrade_not_retroactive;
pub mod subscription_feature_requires_pg_version;
pub mod subscription_password_in_source;
pub mod subscription_references_undeclared_publication;
pub mod trigger_references_unmanaged_function;
pub mod trigger_references_unmanaged_table;
pub mod type_shadows_table;
pub mod unmanaged_publication;
pub mod unmanaged_reloption;
pub mod unmanaged_statistic;
pub mod unmanaged_subscription;
pub mod view_body_references_unmanaged_schema;
pub mod view_shadows_table;

// ── shared helpers ─────────────────────────────────────────────────────────────

use crate::lint::finding::{Finding, Severity};
use std::fmt::Display;

/// Built-in `PostgreSQL` schemas that are never managed by pgevolve but are
/// always valid targets for cross-schema references.
pub const BUILTIN_SCHEMAS: &[&str] = &["pg_catalog", "information_schema"];

/// Shared helper for "unmanaged-X" lint rules.
///
/// Per the lenient drift policy, pgevolve does not auto-drop catalog
/// objects that source doesn't declare. This helper emits one Warning
/// per target-only object so operators can decide to bring it under
/// management or accept the drift.
///
/// Used by [`unmanaged_publication`], [`unmanaged_subscription`],
/// [`unmanaged_statistic`].
pub fn check_unmanaged_objects<T, K, F>(
    target: &[T],
    source: &[T],
    key: F,
    rule_id: &'static str,
    noun: &str,
) -> Vec<Finding>
where
    K: PartialEq + Display,
    F: Fn(&T) -> &K,
{
    target
        .iter()
        .filter(|t| !source.iter().any(|s| key(s) == key(t)))
        .map(|t| Finding {
            rule: rule_id,
            severity: Severity::Warning,
            message: format!(
                "{noun} {}: catalog has a {noun} not declared in source",
                key(t),
            ),
            location: None,
        })
        .collect()
}

/// Extract all `schema.name` qualified-identifier pairs from a SQL expression
/// text. Returns `(schema, name)` pairs for any token sequence of the form
/// `<identifier>.<identifier>` found in `text`.
pub fn extract_qualified_refs(text: &str) -> Vec<(String, String)> {
    // Tokenize: split on whitespace and punctuation, keeping only identifier
    // characters (letters, digits, underscore) and dots. Then scan for
    // consecutive tokens of the form `<word>.<word>`.
    let mut result = Vec::new();
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

pub const fn is_id_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub const fn is_id_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
