//! Layout-profile lint rules. Spec §12.
//!
//! Each profile expresses *where* an object should live on disk. The
//! universal rules don't care about paths; this module does.

pub mod custom;
pub mod feature_grouped;
pub mod free_form;
pub mod kind_grouped;
pub mod schema_mirror;

use std::path::Path;

pub use custom::{Assertion, CustomProfile, PathPattern};

use super::finding::Finding;
use super::source_tree::SourceTree;

/// One of the five layout profiles.
#[derive(Debug, Clone)]
pub enum Profile {
    /// `schema/<schema>/<kind>/<name>.sql` — strictest.
    SchemaMirror,
    /// `schema/<kind>/<schema>.<name>.sql`.
    KindGrouped,
    /// `schema/<feature>/*.sql` — files grouped by feature.
    FeatureGrouped,
    /// No path constraints; universal rules only.
    FreeForm,
    /// User-defined regex + assertion rules.
    Custom(CustomProfile),
}

impl Profile {
    /// Resolve a profile name (built-in keyword or path to a custom TOML).
    ///
    /// Recognized keywords: `schema-mirror`, `kind-grouped`,
    /// `feature-grouped`, `free-form`. Any other string is treated as a path
    /// to a custom-profile TOML file.
    pub fn from_name(name: &str) -> Result<Self, ProfileLoadError> {
        match name {
            "schema-mirror" => Ok(Self::SchemaMirror),
            "kind-grouped" => Ok(Self::KindGrouped),
            "feature-grouped" => Ok(Self::FeatureGrouped),
            "free-form" => Ok(Self::FreeForm),
            other => {
                let path = Path::new(other);
                let content = std::fs::read_to_string(path)
                    .map_err(|e| ProfileLoadError::Io(path.to_path_buf(), e.to_string()))?;
                let custom: CustomProfile = toml::from_str(&content)
                    .map_err(|e| ProfileLoadError::Parse(path.to_path_buf(), e.to_string()))?;
                Ok(Self::Custom(custom))
            }
        }
    }
}

/// Failure mode for [`Profile::from_name`].
#[derive(Debug, thiserror::Error)]
pub enum ProfileLoadError {
    /// I/O reading the custom profile TOML.
    #[error("reading custom profile {0}: {1}")]
    Io(std::path::PathBuf, String),
    /// Custom profile TOML didn't parse.
    #[error("parsing custom profile {0}: {1}")]
    Parse(std::path::PathBuf, String),
}

/// Dispatch to the chosen profile's rules.
pub fn check_profile(profile: &Profile, tree: &SourceTree, schema_dir: &Path) -> Vec<Finding> {
    match profile {
        Profile::SchemaMirror => schema_mirror::check(tree, schema_dir),
        Profile::KindGrouped => kind_grouped::check(tree, schema_dir),
        Profile::FeatureGrouped => feature_grouped::check(tree, schema_dir),
        Profile::FreeForm => free_form::check(tree, schema_dir),
        Profile::Custom(c) => custom::check(c, tree, schema_dir),
    }
}
