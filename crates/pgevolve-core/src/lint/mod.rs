//! Lint engine. Spec §12.
//!
//! Two-stage check:
//! 1. [`universal::check_universal`] — rules that apply to every profile.
//! 2. [`profile::check_profile`] — path-shape rules specific to one layout
//!    profile (schema-mirror, kind-grouped, feature-grouped, free-form,
//!    custom).
//!
//! Callers compose both via [`run`].

pub mod finding;
pub mod profile;
pub(crate) mod rules;
pub mod source_tree;
pub mod universal;

use std::path::Path;

pub use finding::{Finding, Severity};
pub use profile::{CustomProfile, PathPattern, Profile, check_profile};
pub use source_tree::{ObjectKey, SourceTree};
pub use universal::{LINT_AT_PLAN_RULES, check_changeset, check_universal};

/// Inputs for [`run`].
#[derive(Debug, Clone)]
pub struct LintInputs<'a> {
    /// Parsed source tree.
    pub tree: &'a SourceTree,
    /// `[managed]` config block.
    pub managed: &'a ManagedConfig,
    /// Profile to apply.
    pub profile: &'a Profile,
    /// Source-tree root, used to compute relative paths for profile checks.
    pub schema_dir: &'a Path,
}

/// `[managed]` view passed to the lint engine.
///
/// Mirrors the binary's `ManagedConfig` but lives in the core crate so the
/// lint engine doesn't depend on the binary.
#[derive(Debug, Clone, Default)]
pub struct ManagedConfig {
    /// Schemas under pgevolve's control.
    pub schemas: Vec<crate::identifier::Identifier>,
}

/// Run universal rules + the configured profile against `inputs`.
pub fn run(inputs: &LintInputs<'_>) -> Vec<Finding> {
    let mut out = check_universal(inputs.tree, inputs.managed);
    out.extend(check_profile(
        inputs.profile,
        inputs.tree,
        inputs.schema_dir,
    ));
    out
}

impl SourceTree {
    /// Parse `root` into a `SourceTree`.
    pub fn parse(root: &Path, ignores: &[glob::Pattern]) -> Result<Self, crate::parse::ParseError> {
        let (catalog, string_locations) =
            crate::parse::parse_directory_with_locations(root, ignores)?;
        let mut object_locations = std::collections::HashMap::new();
        for s in &catalog.schemas {
            if let Some(loc) = string_locations.get(&s.name.to_string()) {
                object_locations.insert(ObjectKey::Schema(s.name.clone()), loc.clone());
            }
        }
        for t in &catalog.tables {
            if let Some(loc) = string_locations.get(&t.qname.to_string()) {
                object_locations.insert(ObjectKey::Table(t.qname.clone()), loc.clone());
            }
        }
        for i in &catalog.indexes {
            if let Some(loc) = string_locations.get(&i.qname.to_string()) {
                object_locations.insert(ObjectKey::Index(i.qname.clone()), loc.clone());
            }
        }
        for seq in &catalog.sequences {
            if let Some(loc) = string_locations.get(&seq.qname.to_string()) {
                object_locations.insert(ObjectKey::Sequence(seq.qname.clone()), loc.clone());
            }
        }
        Ok(Self::new(catalog, object_locations))
    }
}
