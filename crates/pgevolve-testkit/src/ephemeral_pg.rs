//! Ephemeral Postgres provisioning via `testcontainers`.
//!
//! [`EphemeralPostgres`] starts a `postgres:<major>-alpine` container, waits
//! for the database to accept connections, and exposes a connection-string DSN.
//!
//! Tests that depend on this module check [`docker_available`] first to skip
//! gracefully on hosts without Docker (and on CI runners that opt out via the
//! `PGEVOLVE_DISABLE_DOCKER_TESTS` env var).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio_postgres::{Client, NoTls};

use pgevolve_core::catalog::PgVersion;

/// `postgres:<major>-alpine` running in a one-shot container.
///
/// The container is removed when this struct drops (via `testcontainers`).
pub struct EphemeralPostgres {
    _container: ContainerAsync<GenericImage>,
    dsn: String,
    version: PgVersion,
}

impl std::fmt::Debug for EphemeralPostgres {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EphemeralPostgres")
            .field("version", &self.version)
            .field("dsn", &self.dsn)
            .finish_non_exhaustive()
    }
}

impl EphemeralPostgres {
    /// Start a fresh Postgres container of the given major version.
    pub async fn start(version: PgVersion) -> Result<Self> {
        let tag = match version {
            PgVersion::Pg14 => "14-alpine",
            PgVersion::Pg15 => "15-alpine",
            PgVersion::Pg16 => "16-alpine",
            PgVersion::Pg17 => "17-alpine",
        };

        let image = GenericImage::new("postgres", tag)
            .with_exposed_port(5432.tcp())
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_PASSWORD", "pgevolve")
            .with_env_var("POSTGRES_USER", "pgevolve")
            .with_env_var("POSTGRES_DB", "pgevolve");

        let container = image
            .start()
            .await
            .with_context(|| format!("failed to start postgres:{tag}"))?;

        let host = container
            .get_host()
            .await
            .with_context(|| "could not get container host")?;
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .with_context(|| "could not get mapped 5432 port")?;
        let dsn =
            format!("postgresql://pgevolve:pgevolve@{host}:{port}/pgevolve?connect_timeout=5");

        // The wait-for-message strategy can fire while the postmaster is
        // restarting after init; poll for a successful connection too.
        wait_until_ready(&dsn).await?;

        Ok(Self {
            _container: container,
            dsn,
            version,
        })
    }

    /// DSN string consumable by `tokio_postgres::connect`.
    #[must_use]
    pub fn dsn(&self) -> &str {
        &self.dsn
    }

    /// Major version of the running container.
    #[must_use]
    pub const fn version(&self) -> PgVersion {
        self.version
    }

    /// Open a fresh `tokio_postgres` client.
    pub async fn connect(&self) -> Result<Client> {
        let (client, connection) = tokio_postgres::connect(&self.dsn, NoTls)
            .await
            .with_context(|| format!("connecting to {}", self.dsn))?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                tracing::debug!(?err, "postgres connection task ended");
            }
        });
        Ok(client)
    }

    /// Execute a SQL script via the simple-query protocol (`batch_execute`).
    pub async fn exec_sql(&self, sql: &str) -> Result<()> {
        let client = self.connect().await?;
        client
            .batch_execute(sql)
            .await
            .with_context(|| format!("executing SQL: {sql}"))?;
        Ok(())
    }
}

/// Whether tests that need Docker should run.
///
/// Returns `false` if `PGEVOLVE_DISABLE_DOCKER_TESTS` is set or `docker info`
/// returns a non-zero exit code.
#[must_use]
pub fn docker_available() -> bool {
    if std::env::var_os("PGEVOLVE_DISABLE_DOCKER_TESTS").is_some() {
        return false;
    }
    std::process::Command::new("docker")
        .arg("info")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Major Postgres version that ephemeral-container tests should run against.
///
/// Reads `PGEVOLVE_TEST_PG_VERSION` (one of `14`, `15`, `16`, `17`).
/// Defaults to [`PgVersion::Pg16`] when unset so a bare `cargo test` keeps
/// running the workload it always did. The CI matrix in
/// `.github/workflows/ci.yml` sets this per row so each job exercises a
/// different major version.
///
/// # Panics
/// Panics on an unrecognized value to surface a typo in CI config loudly
/// rather than silently falling back to the default.
#[must_use]
pub fn default_pg_version() -> PgVersion {
    parse_pg_version_env(std::env::var("PGEVOLVE_TEST_PG_VERSION").ok().as_deref())
}

fn parse_pg_version_env(value: Option<&str>) -> PgVersion {
    match value {
        Some("14") => PgVersion::Pg14,
        Some("15") => PgVersion::Pg15,
        None | Some("16") => PgVersion::Pg16,
        Some("17") => PgVersion::Pg17,
        Some(other) => panic!("PGEVOLVE_TEST_PG_VERSION must be one of 14|15|16|17, got {other:?}"),
    }
}

async fn wait_until_ready(dsn: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_mins(1);
    let mut last_err: Option<anyhow::Error> = None;
    while std::time::Instant::now() < deadline {
        match tokio_postgres::connect(dsn, NoTls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                match client.simple_query("SELECT 1").await {
                    Err(e) => {
                        last_err = Some(anyhow!("smoke query failed: {e}"));
                    }
                    _ => {
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                last_err = Some(anyhow!("connect failed: {e}"));
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow!("postgres did not become ready before deadline")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn smoke_runs_against_default_pg_version() {
        if !docker_available() {
            return;
        }
        let version = default_pg_version();
        let pg = EphemeralPostgres::start(version)
            .await
            .expect("postgres starts");
        pg.exec_sql("CREATE TABLE foo (id integer);")
            .await
            .expect("ddl applies");
        let client = pg.connect().await.expect("connects");
        let row = client
            .query_one("SELECT current_setting('server_version_num')::int", &[])
            .await
            .expect("queries");
        let v: i32 = row.get(0);
        let major = i32::try_from(version.major()).expect("major fits in i32");
        let lo = major * 10_000;
        let hi = (major + 1) * 10_000;
        assert!((lo..hi).contains(&v), "expected pg{major}, got {v}");
    }

    #[test]
    fn parse_pg_version_env_handles_supported_values() {
        assert_eq!(parse_pg_version_env(None), PgVersion::Pg16);
        assert_eq!(parse_pg_version_env(Some("14")), PgVersion::Pg14);
        assert_eq!(parse_pg_version_env(Some("15")), PgVersion::Pg15);
        assert_eq!(parse_pg_version_env(Some("16")), PgVersion::Pg16);
        assert_eq!(parse_pg_version_env(Some("17")), PgVersion::Pg17);
    }

    #[test]
    #[should_panic(expected = "PGEVOLVE_TEST_PG_VERSION must be one of")]
    fn parse_pg_version_env_panics_on_unknown() {
        let _ = parse_pg_version_env(Some("99"));
    }
}
