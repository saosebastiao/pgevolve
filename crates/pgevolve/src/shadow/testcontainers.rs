//! `testcontainers`-backed shadow Postgres.
//!
//! Starts a `postgres:<major>-alpine` container, waits for it to be
//! ready, and exposes the DSN via [`ShadowGuard::url`].  The container
//! is removed when the guard drops.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio_postgres::NoTls;

use crate::config::ShadowConfig;

use super::{CheckoutFuture, PgMajor, ResetFuture, ShadowBackend, ShadowGuard};

/// Backend that spins up a one-shot Docker container per checkout.
pub struct TestcontainersBackend {
    config: ShadowConfig,
}

impl TestcontainersBackend {
    /// Construct from the `[shadow]` config section.
    pub fn new(config: &ShadowConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }
}

impl ShadowBackend for TestcontainersBackend {
    fn checkout(&self, major: PgMajor) -> CheckoutFuture<'_> {
        Box::pin(async move {
            let tag = major_to_tag(major)?;

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
            let dsn = format!(
                "postgresql://pgevolve:pgevolve@{host}:{port}/pgevolve?connect_timeout=5"
            );

            wait_until_ready(&dsn).await?;

            if !self.config.extensions.is_empty() {
                super::install_extensions(&dsn, &self.config.extensions).await?;
            }

            Ok(Box::new(TestcontainersGuard {
                _container: container,
                dsn,
            }) as Box<dyn ShadowGuard>)
        })
    }
}

/// Live handle to a running container.  Dropped when the caller is done.
struct TestcontainersGuard {
    _container: ContainerAsync<GenericImage>,
    dsn: String,
}

impl ShadowGuard for TestcontainersGuard {
    fn url(&self) -> &str {
        &self.dsn
    }

    fn reset(&mut self) -> ResetFuture<'_> {
        // testcontainers backend: the container is destroyed at Drop, so
        // reset is a no-op.  Pool-reuse with DROP SCHEMA CASCADE can be
        // added here later.
        Box::pin(async { Ok(()) })
    }
}

/// Convert a numeric PG major to the Docker image tag.
fn major_to_tag(major: PgMajor) -> Result<&'static str> {
    match major {
        14 => Ok("14-alpine"),
        15 => Ok("15-alpine"),
        16 => Ok("16-alpine"),
        17 => Ok("17-alpine"),
        other => Err(anyhow!(
            "unsupported Postgres major version: {other}; expected 14–17"
        )),
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
            Err(e) => last_err = Some(anyhow!("connect failed: {e}")),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow!("timed out waiting for shadow Postgres")))
}
