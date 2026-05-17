//! `TestPgBackend` — pluggable real-Postgres backend for tests.
//!
//! Three backends, selected via the `PGEVOLVE_TEST_PG_MODE` env var:
//!
//! - `testcontainers` (default): boots fresh containers via testcontainers.
//!   Hermetic; requires Docker.
//! - `compose`: connects to fixed-port containers from
//!   `dev/docker-compose.pg.yml`. Fast iteration; shared state across runs.
//!   URLs from `PGEVOLVE_TEST_PG_<MAJOR>_URL` env vars.
//! - `dsn`: connects to user-supplied DSNs. Managed PG / restricted dev.
//!   URLs from `PGEVOLVE_TEST_PG_<MAJOR>_URL` env vars.
//!
//! # Quickstart (compose mode)
//!
//! ```text
//! docker compose -f dev/docker-compose.pg.yml up -d
//!
//! PGEVOLVE_TEST_PG_MODE=compose \
//!   PGEVOLVE_TEST_PG_14_URL=postgres://postgres:postgres@localhost:54214/postgres \
//!   PGEVOLVE_TEST_PG_15_URL=postgres://postgres:postgres@localhost:54215/postgres \
//!   PGEVOLVE_TEST_PG_16_URL=postgres://postgres:postgres@localhost:54216/postgres \
//!   PGEVOLVE_TEST_PG_17_URL=postgres://postgres:postgres@localhost:54217/postgres \
//!   cargo test
//! ```
//!
//! # Quickstart (dsn mode)
//!
//! ```text
//! PGEVOLVE_TEST_PG_MODE=dsn \
//!   PGEVOLVE_TEST_PG_16_URL=postgres://user:pass@managed-host:5432/testdb \
//!   cargo test
//! ```

use std::collections::BTreeMap;
use std::env;

use anyhow::Result;
use async_trait::async_trait;
use pgevolve_core::catalog::PgVersion;

/// Which Postgres provisioning strategy to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendMode {
    /// Start hermetic containers via `testcontainers`. Requires Docker.
    Testcontainers,
    /// Connect to fixed-port containers managed by `dev/docker-compose.pg.yml`.
    /// Requires `PGEVOLVE_TEST_PG_<MAJOR>_URL` env vars.
    Compose,
    /// Connect to externally managed Postgres instances.
    /// Requires `PGEVOLVE_TEST_PG_<MAJOR>_URL` env vars.
    Dsn,
}

impl BackendMode {
    /// Read the mode from `PGEVOLVE_TEST_PG_MODE`.
    ///
    /// Defaults to [`BackendMode::Testcontainers`] when the var is absent or
    /// set to an unrecognised value.
    pub fn from_env() -> Self {
        match env::var("PGEVOLVE_TEST_PG_MODE").as_deref() {
            Ok("compose") => Self::Compose,
            Ok("dsn") => Self::Dsn,
            _ => Self::Testcontainers,
        }
    }
}

/// A live connection / container for a single test run.
///
/// The guard is borrowed for the duration of a test; calling [`reset`] drops
/// all user-created schemas so the next caller gets a clean slate.
#[async_trait]
pub trait TestPgGuard: Send {
    /// DSN string suitable for `tokio_postgres::connect`.
    fn dsn(&self) -> &str;
    /// Reset the database to a clean state (drop all user schemas, recreate
    /// `public`). For the testcontainers backend this is a no-op because the
    /// container is torn down on drop.
    async fn reset(&mut self) -> Result<()>;
}

/// Source of [`TestPgGuard`] instances.
///
/// Each call to [`checkout`] returns a ready-to-use guard for the requested
/// major Postgres version.
#[async_trait]
pub trait TestPgBackend: Send + Sync {
    /// Check out a connection to a Postgres instance of the given major
    /// version.  The caller is responsible for calling [`TestPgGuard::reset`]
    /// between independent test cases when reusing the same guard.
    async fn checkout(&self, version: PgVersion) -> Result<Box<dyn TestPgGuard>>;
}

/// Build the backend indicated by [`BackendMode::from_env`].
pub fn resolve() -> Result<Box<dyn TestPgBackend>> {
    match BackendMode::from_env() {
        BackendMode::Testcontainers => Ok(Box::new(TestcontainersBackend)),
        BackendMode::Compose => Ok(Box::new(ComposeBackend::from_env()?)),
        BackendMode::Dsn => Ok(Box::new(DsnBackend::from_env()?)),
    }
}

// ---------------------------------------------------------------------------
// testcontainers backend
// ---------------------------------------------------------------------------

/// Starts a fresh `postgres:<major>-alpine` container per checkout.
pub struct TestcontainersBackend;

#[async_trait]
impl TestPgBackend for TestcontainersBackend {
    async fn checkout(&self, version: PgVersion) -> Result<Box<dyn TestPgGuard>> {
        let pg = crate::ephemeral_pg::EphemeralPostgres::start(version).await?;
        Ok(Box::new(TestcontainersGuard { pg }))
    }
}

struct TestcontainersGuard {
    pg: crate::ephemeral_pg::EphemeralPostgres,
}

#[async_trait]
impl TestPgGuard for TestcontainersGuard {
    fn dsn(&self) -> &str {
        self.pg.dsn()
    }

    async fn reset(&mut self) -> Result<()> {
        // Testcontainers backend: the container is destroyed on Drop and a new
        // container is booted for the next checkout, so reset is a no-op.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// compose backend
// ---------------------------------------------------------------------------

/// Connects to containers started by `dev/docker-compose.pg.yml`.
pub struct ComposeBackend {
    urls: BTreeMap<u32, String>,
}

impl ComposeBackend {
    /// Build from `PGEVOLVE_TEST_PG_<MAJOR>_URL` env vars.
    ///
    /// # Errors
    /// Returns an error when no URLs are configured.
    pub fn from_env() -> Result<Self> {
        let urls = collect_dsn_env_vars();
        if urls.is_empty() {
            anyhow::bail!(
                "compose mode requires PGEVOLVE_TEST_PG_<MAJOR>_URL env vars \
                 (e.g. PGEVOLVE_TEST_PG_17_URL=postgres://postgres:postgres@localhost:54217/postgres)"
            );
        }
        Ok(Self { urls })
    }
}

#[async_trait]
impl TestPgBackend for ComposeBackend {
    async fn checkout(&self, version: PgVersion) -> Result<Box<dyn TestPgGuard>> {
        let major = version.major();
        let url = self.urls.get(&major).ok_or_else(|| {
            anyhow::anyhow!(
                "no PGEVOLVE_TEST_PG_{major}_URL configured for compose mode"
            )
        })?;
        let mut guard = DsnGuard { url: url.clone() };
        guard.reset().await?;
        Ok(Box::new(guard))
    }
}

// ---------------------------------------------------------------------------
// dsn backend
// ---------------------------------------------------------------------------

/// Connects to externally managed Postgres instances via user-supplied DSNs.
pub struct DsnBackend {
    urls: BTreeMap<u32, String>,
}

impl DsnBackend {
    /// Build from `PGEVOLVE_TEST_PG_<MAJOR>_URL` env vars.
    ///
    /// # Errors
    /// Returns an error when no URLs are configured.
    pub fn from_env() -> Result<Self> {
        let urls = collect_dsn_env_vars();
        if urls.is_empty() {
            anyhow::bail!(
                "dsn mode requires PGEVOLVE_TEST_PG_<MAJOR>_URL env vars \
                 (e.g. PGEVOLVE_TEST_PG_16_URL=postgres://user:pass@host:5432/db)"
            );
        }
        Ok(Self { urls })
    }
}

#[async_trait]
impl TestPgBackend for DsnBackend {
    async fn checkout(&self, version: PgVersion) -> Result<Box<dyn TestPgGuard>> {
        let major = version.major();
        let url = self.urls.get(&major).ok_or_else(|| {
            anyhow::anyhow!(
                "no PGEVOLVE_TEST_PG_{major}_URL configured for dsn mode"
            )
        })?;
        let mut guard = DsnGuard { url: url.clone() };
        guard.reset().await?;
        Ok(Box::new(guard))
    }
}

// ---------------------------------------------------------------------------
// shared DSN guard (compose + dsn)
// ---------------------------------------------------------------------------

struct DsnGuard {
    url: String,
}

#[async_trait]
impl TestPgGuard for DsnGuard {
    fn dsn(&self) -> &str {
        &self.url
    }

    async fn reset(&mut self) -> Result<()> {
        let (client, conn) =
            tokio_postgres::connect(&self.url, tokio_postgres::NoTls).await?;
        let conn_handle = tokio::spawn(conn);
        let result = client
            .batch_execute(
                "DO $$
                 DECLARE r record;
                 BEGIN
                   FOR r IN SELECT nspname FROM pg_namespace
                            WHERE nspname NOT IN (
                              'pg_catalog', 'information_schema',
                              'pg_toast', 'public'
                            )
                              AND nspname NOT LIKE 'pg_temp_%'
                              AND nspname NOT LIKE 'pg_toast_temp_%'
                   LOOP EXECUTE format('DROP SCHEMA %I CASCADE', r.nspname); END LOOP;
                   EXECUTE 'DROP SCHEMA public CASCADE';
                   EXECUTE 'CREATE SCHEMA public';
                 END $$;",
            )
            .await;
        drop(client);
        let _ = conn_handle.await;
        result?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn collect_dsn_env_vars() -> BTreeMap<u32, String> {
    let mut urls = BTreeMap::new();
    for major in [14_u32, 15, 16, 17] {
        let key = format!("PGEVOLVE_TEST_PG_{major}_URL");
        if let Ok(url) = env::var(&key) {
            urls.insert(major, url);
        }
    }
    urls
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_mode_from_env_defaults_to_testcontainers() {
        // SAFETY: env access is exclusive to this variable in these tests; no
        // other test in this module touches PGEVOLVE_TEST_PG_MODE. Edition
        // 2024 marks set_var/remove_var as `unsafe` because they are racy
        // across threads in the general case.
        #[allow(unsafe_code)]
        unsafe {
            env::remove_var("PGEVOLVE_TEST_PG_MODE");
        }
        assert_eq!(BackendMode::from_env(), BackendMode::Testcontainers);
    }

    #[test]
    fn backend_mode_recognizes_compose_and_dsn() {
        // SAFETY: see `backend_mode_from_env_defaults_to_testcontainers`.
        #[allow(unsafe_code)]
        unsafe {
            env::set_var("PGEVOLVE_TEST_PG_MODE", "compose");
        }
        assert_eq!(BackendMode::from_env(), BackendMode::Compose);

        #[allow(unsafe_code)]
        unsafe {
            env::set_var("PGEVOLVE_TEST_PG_MODE", "dsn");
        }
        assert_eq!(BackendMode::from_env(), BackendMode::Dsn);

        #[allow(unsafe_code)]
        unsafe {
            env::remove_var("PGEVOLVE_TEST_PG_MODE");
        }
    }

    #[test]
    fn backend_mode_unknown_value_falls_back_to_testcontainers() {
        // SAFETY: see `backend_mode_from_env_defaults_to_testcontainers`.
        #[allow(unsafe_code)]
        unsafe {
            env::set_var("PGEVOLVE_TEST_PG_MODE", "unknown_value");
        }
        assert_eq!(BackendMode::from_env(), BackendMode::Testcontainers);

        #[allow(unsafe_code)]
        unsafe {
            env::remove_var("PGEVOLVE_TEST_PG_MODE");
        }
    }

    #[test]
    fn collect_dsn_env_vars_picks_up_configured_versions() {
        // SAFETY: see `backend_mode_from_env_defaults_to_testcontainers`.
        #[allow(unsafe_code)]
        unsafe {
            env::set_var("PGEVOLVE_TEST_PG_16_URL", "postgres://x:x@localhost/db");
        }
        let urls = collect_dsn_env_vars();
        assert!(urls.contains_key(&16));

        #[allow(unsafe_code)]
        unsafe {
            env::remove_var("PGEVOLVE_TEST_PG_16_URL");
        }
    }
}
