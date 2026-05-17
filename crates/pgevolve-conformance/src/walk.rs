//! Multi-subtree fixture walker.

use std::path::Path;

use anyhow::Result;
use walkdir::WalkDir;

use crate::fixture::Fixture;

/// One discovered fixture plus its `authoring` routing key.
#[derive(Debug, Clone)]
pub struct DiscoveredFixture {
    /// The loaded fixture.
    pub fixture: Fixture,
    /// Authoring routing key derived from fixture metadata and path.
    pub authoring: Authoring,
}

/// Routing key that determines which assertion layers fire for a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authoring {
    /// One feature × one change-kind. Runs L1–L5, L7 (if destructive).
    Objects,
    /// Multi-feature workflows. L8 and L9 are opt-in.
    Scenarios,
    /// Primary contract is intent or lint-waiver surface.
    Intent,
    /// Primary contract is that pgevolve *refuses*.
    Failure,
    /// One-off captures from property-test failures.
    Regressions,
}

impl Authoring {
    /// Parse an authoring key from the `[meta].authoring` field.
    pub fn from_meta(s: &str) -> Result<Self> {
        Ok(match s {
            "objects" => Self::Objects,
            "scenarios" => Self::Scenarios,
            "intent" => Self::Intent,
            "failure" => Self::Failure,
            "regressions" => Self::Regressions,
            other => anyhow::bail!("unknown authoring key: {other}"),
        })
    }
}

/// Walk a cases root and return every fixture found, keyed by authoring.
///
/// Each discovered fixture is cross-checked: the declared `authoring` field in
/// `fixture.toml` must match the top-level subdirectory the fixture lives under.
/// A mismatch is an error — it prevents silently running the wrong layers.
pub fn discover(cases_root: &Path) -> Result<Vec<DiscoveredFixture>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(cases_root).into_iter().filter_map(std::result::Result::ok) {
        if entry.file_name() != "fixture.toml" {
            continue;
        }
        let dir = entry.path().parent().unwrap().to_path_buf();
        let fixture = Fixture::load(&dir)?;
        let authoring = Authoring::from_meta(&fixture.meta.authoring)
            .map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        // Cross-check: subtree location matches declared authoring key.
        let inferred = infer_authoring(&dir, cases_root)
            .ok_or_else(|| anyhow::anyhow!("{}: cannot infer authoring from path", dir.display()))?;
        if inferred != authoring {
            anyhow::bail!(
                "{}: declared authoring {:?} but lives under {:?} subtree",
                dir.display(),
                authoring,
                inferred,
            );
        }
        out.push(DiscoveredFixture { fixture, authoring });
    }
    out.sort_by_key(|f| f.fixture.dir.clone());
    Ok(out)
}

fn infer_authoring(dir: &Path, cases_root: &Path) -> Option<Authoring> {
    let rel = dir.strip_prefix(cases_root).ok()?;
    let top = rel.iter().next()?.to_str()?;
    Authoring::from_meta(top).ok()
}
