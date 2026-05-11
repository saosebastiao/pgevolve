//! Tier-4 migration-fixture loader.
//!
//! A fixture under `crates/pgevolve-core/tests/fixtures/e2e/<n>-<name>/` has
//! the layout:
//!
//! ```text
//! initial_source/    # source-of-truth dir at step 1
//! final_source/      # source-of-truth dir at step 2
//! README.md          # human notes
//! data_assertions.sql  (optional)
//! ```
//!
//! The runtime harness (plan & apply against [`EphemeralPostgres`](crate::EphemeralPostgres))
//! lands in Phase 9 once the CLI has a `plan` command. This module ships
//! just the loader so fixtures have a stable shape.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A loaded migration fixture.
#[derive(Debug, Clone)]
pub struct MigrationFixture {
    /// Path to the fixture directory.
    pub root: PathBuf,
    /// Path to the `initial_source/` directory.
    pub initial_source_dir: PathBuf,
    /// Path to the `final_source/` directory.
    pub final_source_dir: PathBuf,
    /// Optional `data_assertions.sql` body, if the file exists.
    pub data_assertions_sql: Option<String>,
}

impl MigrationFixture {
    /// Load a fixture rooted at `root`. Fails if `initial_source/` or
    /// `final_source/` is missing.
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let initial = root.join("initial_source");
        let final_ = root.join("final_source");
        if !initial.is_dir() {
            anyhow::bail!("missing initial_source at {}", initial.display());
        }
        if !final_.is_dir() {
            anyhow::bail!("missing final_source at {}", final_.display());
        }
        let data_assertions_path = root.join("data_assertions.sql");
        let data_assertions_sql = if data_assertions_path.is_file() {
            Some(
                std::fs::read_to_string(&data_assertions_path)
                    .with_context(|| format!("reading {}", data_assertions_path.display()))?,
            )
        } else {
            None
        };
        Ok(Self {
            root,
            initial_source_dir: initial,
            final_source_dir: final_,
            data_assertions_sql,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_seed_fixture() {
        let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("pgevolve-core/tests/fixtures/e2e/0001-add-column");
        let f = MigrationFixture::load(&fixture_root).expect("seed fixture loads");
        assert!(f.initial_source_dir.ends_with("initial_source"));
        assert!(f.final_source_dir.ends_with("final_source"));
        assert!(f.data_assertions_sql.is_none());
    }

    #[test]
    fn rejects_missing_initial_source() {
        let dir = tempfile::tempdir().unwrap();
        let err = MigrationFixture::load(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing initial_source"));
    }
}
