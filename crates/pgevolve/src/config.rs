//! `pgevolve.toml` schema and loader. Spec §11.
//!
//! Sections: `[project]`, `[managed]`, `[planner]`, `[environments.<name>]`,
//! `[shadow]`. The loader validates every parsed config eagerly so the
//! commands can assume well-formed input.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use pgevolve_core::identifier::Identifier;

/// Errors raised by [`load`] and [`PgevolveConfig::validate`].
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Filesystem error reading the config file.
    #[error("i/o reading {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    /// TOML parse failure.
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
    /// A `managed.schemas` entry is not a valid Postgres identifier.
    #[error("managed.schemas[{idx}]: {message}")]
    InvalidSchemaName {
        /// Index of the offending entry.
        idx: usize,
        /// Reason from `Identifier::from_unquoted`.
        message: String,
    },
    /// `planner.strategy` is not a recognized value.
    #[error("planner.strategy: expected `atomic` or `online`, got `{0}`")]
    InvalidStrategy(String),
    /// Unknown environment.
    #[error("unknown environment: `{0}`")]
    UnknownEnvironment(String),
    /// A required environment variable is missing.
    #[error("environment variable `{0}` is not set")]
    EnvVarMissing(String),
    /// An environment specifies neither `url` nor `url_env`.
    #[error("environment `{0}`: must set either `url` or `url_env`")]
    EnvMissingDsn(String),
    /// An environment specifies both `url` and `url_env`.
    #[error("environment `{0}`: set only one of `url` or `url_env`")]
    EnvDoubleDsn(String),
    /// A path-typed setting could not be resolved.
    #[error("path not found: {0}")]
    PathNotFound(PathBuf),
}

/// Strategy values accepted by `[planner].strategy` and `[environments.<n>].strategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigStrategy {
    /// All operations run inside one transaction; no online rewrites.
    Atomic,
    /// Apply online rewrites (default).
    #[default]
    Online,
}

impl ConfigStrategy {
    /// Project this onto the planner's [`Strategy`](pgevolve_core::plan::Strategy).
    pub const fn to_planner_strategy(self) -> pgevolve_core::plan::Strategy {
        match self {
            Self::Atomic => pgevolve_core::plan::Strategy::Atomic,
            Self::Online => pgevolve_core::plan::Strategy::Online,
        }
    }
}

/// Top-level `pgevolve.toml`.
#[derive(Debug, Deserialize)]
pub struct PgevolveConfig {
    /// `[project]` section. Required.
    pub project: ProjectConfig,
    /// `[managed]` section. Optional; defaults to empty.
    #[serde(default)]
    pub managed: ManagedConfig,
    /// `[planner]` section. Optional; defaults to `online`.
    #[serde(default)]
    pub planner: PlannerConfig,
    /// `[environments.<name>]` map. Required to have at least one entry for
    /// most subcommands.
    #[serde(default)]
    pub environments: BTreeMap<String, EnvironmentConfig>,
    /// `[shadow]` section. Optional; used by `pgevolve validate --shadow`.
    #[serde(default)]
    pub shadow: Option<ShadowConfig>,
    /// `[cluster]` section. Optional; links this DB project to a cluster
    /// project that manages roles. When present, cluster-aware lints
    /// (e.g. `grant-references-unknown-role`) cross-check grantee role names
    /// against the linked cluster project's declared roles.
    #[serde(default)]
    pub cluster: Option<ClusterLink>,
}

/// `[cluster]` section — optional link to the cluster project that manages
/// roles for this database.
#[derive(Debug, Deserialize)]
pub struct ClusterLink {
    /// Path to the cluster project directory this DB belongs to. Relative
    /// paths resolve against `pgevolve.toml`'s directory.
    pub project: String,
}

/// `[project]` section.
#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    /// Project name; informational.
    pub name: String,
    /// Source-tree root, relative to the config file. Defaults to `schema`.
    #[serde(default = "default_schema_dir")]
    pub schema_dir: PathBuf,
    /// Plan output directory, relative to the config file. Defaults to `plans`.
    #[serde(default = "default_plan_dir")]
    pub plan_dir: PathBuf,
    /// Built-in profile name or path to a custom profile. Defaults to `schema-mirror`.
    #[serde(default = "default_layout_profile")]
    pub layout_profile: String,
}

fn default_schema_dir() -> PathBuf {
    PathBuf::from("schema")
}
fn default_plan_dir() -> PathBuf {
    PathBuf::from("plans")
}
fn default_layout_profile() -> String {
    "schema-mirror".into()
}
const fn default_min_pg_version() -> u32 {
    14
}

/// `[managed]` section.
#[derive(Debug, Default, Deserialize)]
pub struct ManagedConfig {
    /// Schemas under pgevolve's control.
    #[serde(default)]
    pub schemas: Vec<String>,
    /// Glob patterns of objects to ignore even within managed schemas.
    #[serde(default)]
    pub ignore_objects: Vec<String>,
    /// Minimum Postgres major version the project targets. Default 14.
    /// Used to gate PG-version-specific source features (e.g., publication
    /// row filters require PG 15+). When source uses a feature newer than
    /// `min_pg_version`, lint fires `publication-feature-requires-pg-version`
    /// (Error) instead of letting the apply hit a Postgres syntax error.
    #[serde(default = "default_min_pg_version")]
    pub min_pg_version: u32,
}

/// `[planner]` section.
#[derive(Debug, Default, Deserialize)]
pub struct PlannerConfig {
    /// Top-level strategy. Defaults to `online`.
    #[serde(default = "default_strategy")]
    pub strategy: ConfigStrategy,
    /// Per-rewrite switches under `[planner.online_rewrites]`.
    #[serde(default)]
    pub online_rewrites: PlannerOnlineRewrites,
}

const fn default_strategy() -> ConfigStrategy {
    ConfigStrategy::Online
}

/// `[planner.online_rewrites]` switches. Each defaults to `true`.
#[allow(clippy::struct_excessive_bools)] // independent on/off switches by design
#[derive(Debug, Deserialize)]
pub struct PlannerOnlineRewrites {
    /// Rewrite non-unique `CreateIndex` on existing tables to CONCURRENTLY.
    #[serde(default = "default_true")]
    pub create_index_concurrent: bool,
    /// Rewrite FK adds on existing tables to NOT VALID + VALIDATE.
    #[serde(default = "default_true")]
    pub fk_not_valid_then_validate: bool,
    /// Rewrite CHECK adds on existing tables to NOT VALID + VALIDATE.
    #[serde(default = "default_true")]
    pub check_not_valid_then_validate: bool,
    /// Rewrite SET NOT NULL on populated columns via the CHECK pattern.
    #[serde(default = "default_true")]
    pub not_null_via_check_pattern: bool,
    /// Upgrade REFRESH MATERIALIZED VIEW to REFRESH CONCURRENTLY when the MV
    /// has at least one unique index (online strategy only). Default `true`.
    #[serde(default = "default_true")]
    pub refresh_mv_concurrently: bool,
    /// Walk transitively-affected views and emit explicit DROP + CREATE steps
    /// instead of relying on CASCADE. When `false`, the planner errors on any
    /// change that would force dependent view recreations. Default `true`.
    #[serde(default = "default_true")]
    pub view_drop_create_dependents: bool,
}

impl Default for PlannerOnlineRewrites {
    fn default() -> Self {
        Self {
            create_index_concurrent: true,
            fk_not_valid_then_validate: true,
            check_not_valid_then_validate: true,
            not_null_via_check_pattern: true,
            refresh_mv_concurrently: true,
            view_drop_create_dependents: true,
        }
    }
}

const fn default_true() -> bool {
    true
}

/// `[environments.<name>]` section.
#[derive(Debug, Deserialize)]
pub struct EnvironmentConfig {
    /// Explicit DSN. Mutually exclusive with `url_env`.
    pub url: Option<String>,
    /// Name of the environment variable holding the DSN.
    /// Mutually exclusive with `url`.
    pub url_env: Option<String>,
    /// Per-environment strategy override.
    pub strategy: Option<ConfigStrategy>,
}

/// `[shadow]` section. Used by `validate --shadow`.
///
/// Backend selection (in order of precedence):
/// - `backend = "testcontainers"` — always use a Docker container.
/// - `backend = "dsn"` — always use the supplied DSN.
/// - `backend = "auto"` (default) — prefer `dsn` if `url`/`url_env` is set,
///   otherwise fall back to `testcontainers` when Docker is available.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShadowConfig {
    /// Backend name: `"auto"` | `"testcontainers"` | `"dsn"`.
    pub backend: Option<String>,
    /// Postgres major version to use with the `testcontainers` backend
    /// (e.g. `"17"`).  Ignored by the `dsn` backend.
    pub postgres_version: Option<String>,
    /// Literal DSN for the `dsn` backend.
    pub url: Option<String>,
    /// Name of the environment variable holding the DSN for the `dsn` backend.
    pub url_env: Option<String>,
    /// Reset policy between checkouts: `"drop_schema_cascade"` (default) or
    /// `"none"`.
    pub reset: Option<String>,
    /// Extensions to install in the shadow DB before applying the source IR.
    #[serde(default)]
    pub extensions: Vec<String>,
}

impl PgevolveConfig {
    /// Validate the parsed config. Returns the first failure.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (i, s) in self.managed.schemas.iter().enumerate() {
            Identifier::from_unquoted(s).map_err(|e| ConfigError::InvalidSchemaName {
                idx: i,
                message: e.to_string(),
            })?;
        }
        // glob compilation: spec §11 mentions globs; defer compile to use site
        // since the `glob` crate compiles per-pattern at runtime cheaply.
        for (name, env) in &self.environments {
            if env.url.is_some() && env.url_env.is_some() {
                return Err(ConfigError::EnvDoubleDsn(name.clone()));
            }
            // url XOR url_env XOR neither: any single-source or no-source is fine.
        }
        Ok(())
    }
}

/// Read and validate `pgevolve.toml` at `path`.
pub fn load(path: &Path) -> Result<PgevolveConfig, ConfigError> {
    let bytes = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.into(), e))?;
    let cfg: PgevolveConfig = toml::from_str(&bytes)?;
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(s: &str) -> tempfile::NamedTempFile {
        use std::io::Write as _;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f
    }

    #[test]
    fn loads_minimal_config() {
        let f = write_tmp(
            "[project]\nname = \"x\"\n[environments.dev]\nurl = \"postgres://localhost/x\"\n",
        );
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.project.name, "x");
        assert_eq!(cfg.project.schema_dir, PathBuf::from("schema"));
        assert_eq!(cfg.planner.strategy, ConfigStrategy::Online);
        assert_eq!(
            cfg.environments["dev"].url.as_deref(),
            Some("postgres://localhost/x")
        );
    }

    #[test]
    fn rejects_invalid_strategy_via_toml() {
        let f = write_tmp("[project]\nname=\"x\"\n[planner]\nstrategy=\"bogus\"\n");
        assert!(matches!(load(f.path()), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn rejects_double_dsn() {
        let f = write_tmp(
            "[project]\nname=\"x\"\n\
             [environments.dev]\nurl=\"a\"\nurl_env=\"B\"\n",
        );
        assert!(matches!(load(f.path()), Err(ConfigError::EnvDoubleDsn(_))));
    }

    #[test]
    fn rejects_invalid_schema_name() {
        let f = write_tmp(
            "[project]\nname=\"x\"\n\
             [managed]\nschemas=[\"ok\", \"contains space\"]\n",
        );
        match load(f.path()) {
            Err(ConfigError::InvalidSchemaName { idx, .. }) => assert_eq!(idx, 1),
            other => panic!("expected InvalidSchemaName, got {other:?}"),
        }
    }

    #[test]
    fn missing_dsn_is_allowed_for_libpq_fallback() {
        let f = write_tmp(
            "[project]\nname=\"x\"\n\
             [environments.dev]\nstrategy=\"atomic\"\n",
        );
        let cfg = load(f.path()).unwrap();
        let env = &cfg.environments["dev"];
        assert!(env.url.is_none() && env.url_env.is_none());
        assert_eq!(env.strategy, Some(ConfigStrategy::Atomic));
    }

    #[test]
    fn parses_pgevolve_toml_with_cluster_block() {
        let f = write_tmp(
            "[project]\nname=\"x\"\n\
             [cluster]\nproject = \"../my-cluster\"\n",
        );
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.cluster.unwrap().project, "../my-cluster");
    }

    #[test]
    fn parses_pgevolve_toml_without_cluster_block() {
        let f = write_tmp("[project]\nname=\"x\"\n");
        let cfg = load(f.path()).unwrap();
        assert!(cfg.cluster.is_none());
    }

    #[test]
    fn min_pg_version_defaults_to_14() {
        let f = write_tmp(
            "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n\
             [managed]\nschemas=[\"app\"]\n\
             [environments.dev]\nurl=\"postgres://localhost\"\n",
        );
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.managed.min_pg_version, 14);
    }

    #[test]
    fn min_pg_version_can_be_raised() {
        let f = write_tmp(
            "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n\
             [managed]\nschemas=[\"app\"]\nmin_pg_version=16\n\
             [environments.dev]\nurl=\"postgres://localhost\"\n",
        );
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.managed.min_pg_version, 16);
    }

    #[test]
    fn min_pg_version_accepts_18() {
        let f = write_tmp(
            "[project]\nname=\"t\"\nschema_dir=\"schema\"\nplan_dir=\"plans\"\n\
             [managed]\nschemas=[\"app\"]\nmin_pg_version=18\n\
             [environments.dev]\nurl=\"postgres://localhost\"\n",
        );
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.managed.min_pg_version, 18);
    }
}
