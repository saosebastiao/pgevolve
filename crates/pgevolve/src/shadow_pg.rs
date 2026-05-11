//! Ephemeral Postgres provisioning for `validate --shadow`.
//!
//! Spec §10. Spins up a `postgres:<major>-alpine` container, waits for the
//! database to accept connections, and exposes a connection-string DSN.
//!
//! Mirrors `pgevolve-testkit::EphemeralPostgres` (which is dev-only) so the
//! production binary can run shadow validation without depending on the
//! testkit crate.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio_postgres::NoTls;

use pgevolve_core::catalog::PgVersion;

/// `postgres:<major>-alpine` running in a one-shot container.
///
/// The container is removed when this struct drops (via `testcontainers`).
pub struct ShadowPostgres {
    _container: ContainerAsync<GenericImage>,
    dsn: String,
    version: PgVersion,
}

impl std::fmt::Debug for ShadowPostgres {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShadowPostgres")
            .field("version", &self.version)
            .field("dsn", &self.dsn)
            .finish_non_exhaustive()
    }
}

impl ShadowPostgres {
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

        wait_until_ready(&dsn).await?;

        Ok(Self {
            _container: container,
            dsn,
            version,
        })
    }

    /// DSN consumable by `tokio_postgres::connect`.
    #[must_use]
    pub fn dsn(&self) -> &str {
        &self.dsn
    }

    /// Major version of the running container.
    #[must_use]
    pub const fn version(&self) -> PgVersion {
        self.version
    }
}

async fn wait_until_ready(dsn: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut last_err: Option<anyhow::Error> = None;
    while std::time::Instant::now() < deadline {
        match tokio_postgres::connect(dsn, NoTls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                if let Err(e) = client.simple_query("SELECT 1").await {
                    last_err = Some(anyhow!("smoke query failed: {e}"));
                } else {
                    return Ok(());
                }
            }
            Err(e) => last_err = Some(anyhow!("connect failed: {e}")),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow!("timed out waiting for shadow Postgres")))
}

/// Quick check: is Docker available on this host?
///
/// Returns `false` if `PGEVOLVE_DISABLE_DOCKER_TESTS` is set, or if
/// `docker info` fails. `pgevolve validate --shadow` calls this and prints
/// a clear error rather than failing inside testcontainers.
#[must_use]
pub fn docker_available() -> bool {
    if std::env::var_os("PGEVOLVE_DISABLE_DOCKER_TESTS").is_some() {
        return false;
    }
    std::process::Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
