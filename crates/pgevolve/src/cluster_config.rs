//! `pgevolve-cluster.toml` schema + loader. Sibling of `pgevolve.toml` for
//! per-DB projects. Configures connection (superuser DSN) and bootstrap
//! roles (PG-owned roles that pgevolve never diffs in/out).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Root config struct.
#[derive(Debug, Deserialize)]
pub struct ClusterConfig {
    /// `[project]` section. Required.
    pub project: ClusterProject,
    /// `[connection]` section. Required.
    pub connection: ClusterConnection,
    /// `[bootstrap]` section. Optional; defaults to `["postgres"]`.
    #[serde(default)]
    pub bootstrap: Bootstrap,
}

/// `[project]` section of `pgevolve-cluster.toml`.
#[derive(Debug, Deserialize)]
pub struct ClusterProject {
    /// Display name for the project.
    pub name: String,
}

/// `[connection]` section of `pgevolve-cluster.toml`.
#[derive(Debug, Deserialize)]
pub struct ClusterConnection {
    /// Postgres DSN. Must connect with sufficient privileges to read
    /// `pg_authid` (typically superuser).
    pub dsn: String,
}

/// `[bootstrap]` section of `pgevolve-cluster.toml`.
#[derive(Debug, Deserialize)]
pub struct Bootstrap {
    /// Roles that pgevolve treats as PG-owned and never diffs in/out.
    /// Defaults to `["postgres"]`. Cloud Postgres providers typically
    /// need additional entries, e.g. `["postgres", "cloudsqlsuperuser"]`.
    #[serde(default = "default_bootstrap_roles")]
    pub roles: Vec<String>,
}

fn default_bootstrap_roles() -> Vec<String> {
    vec!["postgres".into()]
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            roles: default_bootstrap_roles(),
        }
    }
}

/// Errors raised by [`load`].
#[derive(Debug, Error)]
pub enum ClusterConfigError {
    /// Filesystem error reading the config file.
    #[error("i/o reading {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    /// TOML parse failure.
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Load `pgevolve-cluster.toml` from disk.
pub fn load(path: &Path) -> Result<ClusterConfig, ClusterConfigError> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| ClusterConfigError::Io(path.to_path_buf(), e))?;
    let cfg: ClusterConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal() {
        let toml_text = r#"
            [project]
            name = "my-cluster"
            [connection]
            dsn = "postgresql://superuser@localhost:5432/postgres"
        "#;
        let cfg: ClusterConfig = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.project.name, "my-cluster");
        assert_eq!(cfg.bootstrap.roles, vec!["postgres".to_string()]);
    }

    #[test]
    fn parses_with_custom_bootstrap_roles() {
        let toml_text = r#"
            [project]
            name = "x"
            [connection]
            dsn = "postgres://x"
            [bootstrap]
            roles = ["postgres", "cloudsqlsuperuser"]
        "#;
        let cfg: ClusterConfig = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.bootstrap.roles.len(), 2);
        assert_eq!(cfg.bootstrap.roles[1], "cloudsqlsuperuser");
    }

    #[test]
    fn empty_bootstrap_section_uses_default() {
        let toml_text = r#"
            [project]
            name = "x"
            [connection]
            dsn = "postgres://x"
            [bootstrap]
        "#;
        let cfg: ClusterConfig = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.bootstrap.roles, vec!["postgres".to_string()]);
    }

    #[test]
    fn missing_connection_errors() {
        let toml_text = r#"
            [project]
            name = "x"
        "#;
        let err = toml::from_str::<ClusterConfig>(toml_text).unwrap_err();
        assert!(err.to_string().contains("connection"), "got: {err}");
    }

    #[test]
    fn load_from_disk() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("pgevolve-cluster.toml");
        std::fs::write(
            &path,
            r#"
            [project]
            name = "test"
            [connection]
            dsn = "postgres://test"
        "#,
        )
        .unwrap();
        let cfg = load(&path).unwrap();
        assert_eq!(cfg.project.name, "test");
    }
}
