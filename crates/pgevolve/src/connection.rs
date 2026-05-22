//! Connection options + DSN resolution. Spec §10.2.
//!
//! Precedence (mirrors `psql`):
//! 1. CLI `--url <dsn>` argument
//! 2. `[environments.<env>].url`
//! 3. `[environments.<env>].url_env` (read env var)
//! 4. `PGEVOLVE_DATABASE_URL` env var
//! 5. libpq-style env vars (`PGHOST`, `PGUSER`, etc.) — left for
//!    `tokio_postgres` to parse implicitly via an empty DSN.

use pgevolve_core::identifier::Identifier;
use pgevolve_core::plan::Strategy;

use crate::config::{ConfigError, ConfigStrategy, PgevolveConfig};

/// Resolved connection options for one command invocation.
#[derive(Debug, Clone)]
pub struct DbOptions {
    /// DSN string passed to `tokio_postgres::connect`. May be empty for the
    /// libpq-env fallback.
    pub dsn: String,
    /// Managed schemas from `[managed].schemas` as validated identifiers.
    pub managed_schemas: Vec<Identifier>,
    /// `[managed].ignore_objects` patterns, copied as-is.
    pub ignore_objects: Vec<String>,
    /// Resolved planner strategy after per-env override application.
    pub strategy: Strategy,
}

/// Resolve DSN and strategy for `env_name`.
pub fn resolve_db(
    cfg: &PgevolveConfig,
    env_name: &str,
    cli_url: Option<&str>,
) -> Result<DbOptions, ConfigError> {
    let env = cfg
        .environments
        .get(env_name)
        .ok_or_else(|| ConfigError::UnknownEnvironment(env_name.into()))?;

    let dsn = if let Some(u) = cli_url {
        u.to_string()
    } else if let Some(u) = &env.url {
        u.clone()
    } else if let Some(var) = &env.url_env {
        std::env::var(var).map_err(|_| ConfigError::EnvVarMissing(var.clone()))?
    } else {
        // Empty DSN: tokio_postgres reads PGHOST/PGUSER/... at connect time.
        std::env::var("PGEVOLVE_DATABASE_URL").unwrap_or_default()
    };

    let strategy: ConfigStrategy = env.strategy.unwrap_or(cfg.planner.strategy);
    let mut managed_schemas = Vec::with_capacity(cfg.managed.schemas.len());
    for s in &cfg.managed.schemas {
        managed_schemas.push(Identifier::from_unquoted(s).map_err(|e| {
            ConfigError::InvalidSchemaName {
                idx: 0,
                message: e.to_string(),
            }
        })?);
    }
    Ok(DbOptions {
        dsn,
        managed_schemas,
        ignore_objects: cfg.managed.ignore_objects.clone(),
        strategy: strategy.to_planner_strategy(),
    })
}

/// Open a `tokio_postgres::Client` from `opts`, spawning the background
/// connection task.
pub async fn connect(opts: &DbOptions) -> Result<tokio_postgres::Client, tokio_postgres::Error> {
    let (client, connection) = tokio_postgres::connect(&opts.dsn, tokio_postgres::NoTls).await?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(?err, "postgres connection task ended");
        }
    });
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cfg_with(envs: BTreeMap<String, crate::config::EnvironmentConfig>) -> PgevolveConfig {
        PgevolveConfig {
            project: crate::config::ProjectConfig {
                name: "x".into(),
                schema_dir: "schema".into(),
                plan_dir: "plans".into(),
                layout_profile: "schema-mirror".into(),
            },
            managed: crate::config::ManagedConfig::default(),
            planner: crate::config::PlannerConfig::default(),
            environments: envs,
            shadow: None,
            cluster: None,
        }
    }

    fn env(url: Option<&str>, url_env: Option<&str>) -> crate::config::EnvironmentConfig {
        crate::config::EnvironmentConfig {
            url: url.map(ToString::to_string),
            url_env: url_env.map(ToString::to_string),
            strategy: None,
        }
    }

    #[test]
    fn cli_url_wins_over_env_url() {
        let mut envs = BTreeMap::new();
        envs.insert("dev".into(), env(Some("env://dsn"), None));
        let cfg = cfg_with(envs);
        let r = resolve_db(&cfg, "dev", Some("cli://dsn")).unwrap();
        assert_eq!(r.dsn, "cli://dsn");
    }

    #[test]
    fn env_url_used_when_cli_missing() {
        let mut envs = BTreeMap::new();
        envs.insert("dev".into(), env(Some("env://dsn"), None));
        let cfg = cfg_with(envs);
        let r = resolve_db(&cfg, "dev", None).unwrap();
        assert_eq!(r.dsn, "env://dsn");
    }

    #[test]
    fn url_env_is_read_from_env() {
        let mut envs = BTreeMap::new();
        envs.insert(
            "dev".into(),
            env(None, Some("PGEVOLVE_TEST_DSN_FOR_RESOLVE")),
        );
        let cfg = cfg_with(envs);
        // SAFETY: env access is exclusive to this test by variable name; no
        // cross-test contention. Edition 2024 marks set_var/remove_var as
        // `unsafe` because they're racy across threads in the general case.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("PGEVOLVE_TEST_DSN_FOR_RESOLVE", "from_env_var");
        }
        let r = resolve_db(&cfg, "dev", None).unwrap();
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("PGEVOLVE_TEST_DSN_FOR_RESOLVE");
        }
        assert_eq!(r.dsn, "from_env_var");
    }

    #[test]
    fn unknown_env_errors() {
        let cfg = cfg_with(BTreeMap::new());
        assert!(matches!(
            resolve_db(&cfg, "missing", None),
            Err(ConfigError::UnknownEnvironment(_))
        ));
    }

    #[test]
    fn empty_dsn_fallback_for_libpq_env() {
        let mut envs = BTreeMap::new();
        envs.insert("dev".into(), env(None, None));
        let cfg = cfg_with(envs);
        // SAFETY: see `url_env_is_read_from_env` above — same reasoning.
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("PGEVOLVE_DATABASE_URL");
        }
        let r = resolve_db(&cfg, "dev", None).unwrap();
        assert!(r.dsn.is_empty());
    }
}
