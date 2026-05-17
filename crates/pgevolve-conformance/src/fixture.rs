//! Fixture loading and validation.
//!
//! A fixture directory contains:
//! - `before.sql` — IR baseline ("what's in the DB already")
//! - `after.sql` — IR target ("desired state")
//! - `fixture.toml` — metadata, version range, expected assertions
//! - `expected/diff.txt` — diff substrings (one per line; informational)
//! - `expected/plan.sql` — golden plan SQL (optional; default-on)
//! - `per-pg/pg<N>/plan.sql` — per-version golden override (optional)

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Errors loading or validating a fixture directory.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// IO error reading a required file.
    #[error("io error in {path}: {source}")]
    Io {
        /// Path the error happened on.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// TOML parse error in `fixture.toml`.
    #[error("invalid fixture.toml at {path}: {source}")]
    Toml {
        /// Path to fixture.toml.
        path: PathBuf,
        /// Parse error.
        source: toml::de::Error,
    },
    /// A required file is missing.
    #[error("fixture {dir} missing required file {file}")]
    Missing {
        /// Fixture root.
        dir: PathBuf,
        /// Relative path of the missing file.
        file: String,
    },
    /// `pg.min` greater than `pg.max`.
    #[error("fixture {dir}: pg.min ({min}) > pg.max ({max})")]
    BadVersionRange {
        /// Fixture root.
        dir: PathBuf,
        /// min field.
        min: u32,
        /// max field.
        max: u32,
    },
}

/// `[meta]` block.
#[derive(Debug, Clone, Deserialize)]
pub struct FixtureMeta {
    /// Human-readable title shown in failure output.
    pub title: String,
    /// References to `docs/spec/` capability entries this fixture covers.
    #[serde(default)]
    pub spec_refs: Vec<String>,
    /// Optional issue URL when this fixture is a regression capture.
    #[serde(default)]
    pub issue: Option<String>,
    /// One of: "objects" | "scenarios" | "intent" | "failure" | "regressions".
    /// Drives which assertion layers fire. Defaults to "objects" for
    /// backward compatibility.
    #[serde(default = "default_authoring")]
    pub authoring: String,
}

fn default_authoring() -> String {
    "objects".to_string()
}

/// `[pg]` block.
#[derive(Debug, Clone, Deserialize)]
pub struct FixturePg {
    /// Inclusive minimum supported PG major. Defaults to 14.
    #[serde(default = "default_pg_min")]
    pub min: u32,
    /// Inclusive maximum supported PG major. Defaults to 17.
    #[serde(default = "default_pg_max")]
    pub max: u32,
}

const fn default_pg_min() -> u32 {
    14
}
const fn default_pg_max() -> u32 {
    17
}

impl Default for FixturePg {
    fn default() -> Self {
        Self {
            min: default_pg_min(),
            max: default_pg_max(),
        }
    }
}

/// `[expect.failure]` block — declares which pipeline stage should
/// fail and what the error message should contain.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectFailure {
    /// Stage: `"parse"` | `"ast_resolution"` | `"order"` | `"lint_at_plan"`.
    pub stage: String,
    /// Substrings that must appear in the error message.
    #[serde(default)]
    pub stderr_contains: Vec<String>,
}

/// One `[[expect.intent]]` row.
///
/// Matches against a generated [`DestructiveIntent`] row in the plan.
/// The assertion is mandatory for destructive fixtures (see L7).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectIntentRow {
    /// Intent kind to match (e.g., `"drop_column"`). Case-insensitive substring match.
    pub kind: String,
    /// Fully-qualified target to match (e.g., `"app.users.legacy_id"`).
    pub target: String,
    /// Each string must appear as a substring in the generated intent's reason.
    #[serde(default)]
    pub reason_contains: Vec<String>,
}

/// `[expect]` block.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FixtureExpect {
    /// `[expect.diff]`.
    #[serde(default)]
    pub diff: ExpectDiff,
    /// `[expect.plan]`.
    #[serde(default)]
    pub plan: ExpectPlan,
    /// `[expect.apply]`.
    #[serde(default)]
    pub apply: ExpectApply,
    /// `[expect.dep_graph]` — L8 dep-graph golden. Default-on.
    #[serde(default)]
    pub dep_graph: ExpectDepGraph,
    /// `[[expect.intent]]` rows — L7 intent-shape assertion.
    ///
    /// Note: the top-level `[intent]` table in fixture.toml is a passthrough
    /// written into the plan's `intent.toml`. These `[[expect.intent]]` rows
    /// live under `[expect]` and are at a different TOML nesting level — no
    /// collision.
    #[serde(default, rename = "intent")]
    pub intent: Vec<ExpectIntentRow>,
    /// `[expect.failure]` — failure-fixture contract declaration.
    #[serde(default)]
    pub failure: Option<ExpectFailure>,
}

/// `[expect.dep_graph]` — L8 dep-graph golden.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectDepGraph {
    /// Default true; opt out for trivial fixtures.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Golden file path; default `expected/dep-graph.dot`.
    #[serde(default = "default_dep_graph_golden")]
    pub golden: String,
}

fn default_dep_graph_golden() -> String {
    "expected/dep-graph.dot".to_string()
}

impl Default for ExpectDepGraph {
    fn default() -> Self {
        Self {
            enabled: true,
            golden: default_dep_graph_golden(),
        }
    }
}

/// `[expect.diff]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectDiff {
    /// Substrings that must appear in the rendered diff.
    #[serde(default)]
    pub contains: Vec<String>,
}

/// `[expect.plan]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectPlan {
    /// Expected number of plan steps.
    #[serde(default)]
    pub steps: Option<usize>,
    /// Rewrite identifiers expected in the plan.
    #[serde(default)]
    pub rewrites_used: Vec<String>,
    /// Golden file path. Accepts a string (custom path), `true`
    /// (default `expected/plan.sql`), or `false` (opt out). Missing
    /// key → default-on. See `deserialize_golden`.
    #[serde(default = "default_golden", deserialize_with = "deserialize_golden")]
    pub golden: Option<String>,
    /// L5 opt-out. Default true. Set to false for fixtures whose change
    /// is itself a no-op (rare).
    #[serde(default = "default_true")]
    pub minimality: bool,
    /// L6 input. Absent / empty = layer skipped.
    #[serde(default)]
    pub touches_only: Vec<String>,
    /// L9 input. Each entry is `"A < B"` — when both targets appear in the
    /// plan, A must precede B. Empty = layer skipped.
    #[serde(default)]
    pub order: Vec<String>,
}

fn default_golden() -> Option<String> {
    Some("expected/plan.sql".to_string())
}

fn deserialize_golden<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct GoldenVisitor;
    impl Visitor<'_> for GoldenVisitor {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string path, `true`, or `false`")
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Option<String>, E> {
            Ok(if v { default_golden() } else { None })
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Option<String>, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<Option<String>, E> {
            Ok(Some(v))
        }
    }
    d.deserialize_any(GoldenVisitor)
}

impl Default for ExpectPlan {
    fn default() -> Self {
        Self {
            steps: None,
            rewrites_used: Vec::new(),
            golden: default_golden(),
            minimality: default_true(),
            touches_only: Vec::new(),
            order: Vec::new(),
        }
    }
}

/// `[expect.apply]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExpectApply {
    /// Whether the apply phase is expected to succeed. Defaults to true.
    #[serde(default = "default_true")]
    pub succeeds: bool,
    /// File whose parsed IR is compared against post-apply introspection.
    /// Defaults to `"after.sql"`.
    #[serde(default = "default_post_apply")]
    pub post_apply_equals_to: String,
    /// When `succeeds = false`, every substring here must appear in the
    /// error message from `pgevolve plan` or `pgevolve apply`.
    #[serde(default)]
    pub error_contains: Vec<String>,
}

const fn default_true() -> bool {
    true
}
fn default_post_apply() -> String {
    "after.sql".to_string()
}

impl Default for ExpectApply {
    fn default() -> Self {
        Self {
            succeeds: default_true(),
            post_apply_equals_to: default_post_apply(),
            error_contains: Vec::new(),
        }
    }
}

/// Passthrough tables forwarded verbatim to the planner.
///
/// Deserialized as `toml::Table` so the runner can write them straight to
/// `intent.toml` / merge into config without tracking every key here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FixturePassthrough {
    /// `[intent]` table written into `intent.toml`.
    #[serde(default)]
    pub intent: toml::Table,
    /// `[planner]` table merged into the test config.
    #[serde(default)]
    pub planner: toml::Table,
}

/// A loaded fixture, ready for the runner to operate on.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// Absolute path to the fixture directory.
    pub dir: PathBuf,
    /// `before.sql` contents.
    pub before_sql: String,
    /// `after.sql` contents.
    pub after_sql: String,
    /// `[meta]`.
    pub meta: FixtureMeta,
    /// `[pg]`.
    pub pg: FixturePg,
    /// `[intent]` + `[planner]` passthroughs.
    pub passthrough: FixturePassthrough,
    /// `[expect]`.
    pub expect: FixtureExpect,
}

#[derive(Debug, Deserialize)]
struct RawFixtureToml {
    meta: FixtureMeta,
    #[serde(default)]
    pg: FixturePg,
    #[serde(default)]
    intent: toml::Table,
    #[serde(default)]
    planner: toml::Table,
    #[serde(default)]
    expect: FixtureExpect,
}

impl Fixture {
    /// Load a fixture from its directory.
    pub fn load(dir: &Path) -> Result<Self, FixtureError> {
        let toml_path = dir.join("fixture.toml");
        let toml_bytes = std::fs::read_to_string(&toml_path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                FixtureError::Missing {
                    dir: dir.to_path_buf(),
                    file: "fixture.toml".to_string(),
                }
            } else {
                FixtureError::Io {
                    path: toml_path.clone(),
                    source,
                }
            }
        })?;
        let raw: RawFixtureToml = toml::from_str(&toml_bytes).map_err(|source| {
            FixtureError::Toml {
                path: toml_path,
                source,
            }
        })?;

        if raw.pg.min > raw.pg.max {
            return Err(FixtureError::BadVersionRange {
                dir: dir.to_path_buf(),
                min: raw.pg.min,
                max: raw.pg.max,
            });
        }

        let before_sql = read_required(dir, "before.sql")?;
        let after_sql = read_required(dir, "after.sql")?;

        Ok(Self {
            dir: dir.to_path_buf(),
            before_sql,
            after_sql,
            meta: raw.meta,
            pg: raw.pg,
            passthrough: FixturePassthrough {
                intent: raw.intent,
                planner: raw.planner,
            },
            expect: raw.expect,
        })
    }

    /// Returns the path to the plan-SQL golden for the given PG major.
    /// Resolves `per-pg/pg<N>/plan.sql` first, falling back to
    /// `expected/plan.sql`. Returns `None` when goldening is opted out.
    pub fn golden_path(&self, pg_major: u32) -> Option<PathBuf> {
        let rel = self.expect.plan.golden.as_ref()?;
        let per_pg = self
            .dir
            .join("per-pg")
            .join(format!("pg{pg_major}"))
            .join("plan.sql");
        if per_pg.exists() {
            Some(per_pg)
        } else {
            Some(self.dir.join(rel))
        }
    }

    /// Whether this fixture is supposed to run on the given PG major.
    pub const fn applies_to(&self, pg_major: u32) -> bool {
        pg_major >= self.pg.min && pg_major <= self.pg.max
    }
}

fn read_required(dir: &Path, rel: &str) -> Result<String, FixtureError> {
    let path = dir.join(rel);
    std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            FixtureError::Missing {
                dir: dir.to_path_buf(),
                file: rel.to_string(),
            }
        } else {
            FixtureError::Io { path, source }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(dir: &Path, toml_body: &str, before: &str, after: &str) {
        std::fs::write(dir.join("fixture.toml"), toml_body).unwrap();
        std::fs::write(dir.join("before.sql"), before).unwrap();
        std::fs::write(dir.join("after.sql"), after).unwrap();
    }

    #[test]
    fn loads_minimal_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "trivial"
"#,
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
            "-- @pgevolve schema=app\nCREATE SCHEMA app;\n",
        );
        let f = Fixture::load(tmp.path()).unwrap();
        assert_eq!(f.meta.title, "trivial");
        assert_eq!(f.pg.min, 14);
        assert_eq!(f.pg.max, 17);
        assert_eq!(
            f.expect.plan.golden.as_deref(),
            Some("expected/plan.sql"),
            "golden defaults to expected/plan.sql"
        );
        assert!(f.expect.apply.succeeds);
        assert!(f.applies_to(14));
        assert!(f.applies_to(17));
        assert!(!f.applies_to(13));
        assert!(!f.applies_to(18));
    }

    #[test]
    fn rejects_inverted_version_range() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "bad-range"
[pg]
min = 17
max = 14
"#,
            "",
            "",
        );
        let err = Fixture::load(tmp.path()).unwrap_err();
        match err {
            FixtureError::BadVersionRange { min: 17, max: 14, .. } => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn missing_before_sql_is_diagnosed_clearly() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("fixture.toml"),
            r#"
[meta]
title = "no-before"
"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("after.sql"), "").unwrap();
        let err = Fixture::load(tmp.path()).unwrap_err();
        match err {
            FixtureError::Missing { file, .. } if file == "before.sql" => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn golden_opt_out_via_false() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "opt-out"
[expect.plan]
golden = false
"#,
            "",
            "",
        );
        let f = Fixture::load(tmp.path()).unwrap();
        assert!(f.expect.plan.golden.is_none(), "false → None");
    }

    #[test]
    fn golden_custom_path() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "custom"
[expect.plan]
golden = "expected/custom.sql"
"#,
            "",
            "",
        );
        let f = Fixture::load(tmp.path()).unwrap();
        assert_eq!(f.expect.plan.golden.as_deref(), Some("expected/custom.sql"));
    }

    #[test]
    fn golden_true_uses_default_path() {
        let tmp = tempfile::tempdir().unwrap();
        write_fixture(
            tmp.path(),
            r#"
[meta]
title = "true-default"
[expect.plan]
golden = true
"#,
            "",
            "",
        );
        let f = Fixture::load(tmp.path()).unwrap();
        assert_eq!(f.expect.plan.golden.as_deref(), Some("expected/plan.sql"));
    }
}
