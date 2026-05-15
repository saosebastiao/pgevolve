//! Shadow Postgres backends.
//!
//! `ShadowBackend` is the trait every backend implements. Two concrete
//! backends ship in v0.2:
//!
//! - `testcontainers`: boots a fresh container via the testcontainers
//!   crate. Hermetic; requires Docker. Default when Docker is present.
//! - `dsn`: connects to a user-supplied Postgres via a DSN. Useful for
//!   developers without Docker, for projects with extensions that need
//!   pre-installed binaries, and for managed-PG environments.

pub mod dsn;
pub mod testcontainers;

use async_trait::async_trait;

/// PG major version requested.
pub type PgMajor = u32;

/// One reservation of a shadow Postgres. Drop returns it (or destroys it)
/// per the backend's reset policy.
#[async_trait]
pub trait ShadowGuard: Send {
    /// DSN of the live shadow database.
    fn url(&self) -> &str;
    /// Reset the database to a clean state.
    async fn reset(&mut self) -> anyhow::Result<()>;
}

/// A pluggable shadow Postgres backend.
#[async_trait]
pub trait ShadowBackend: Send + Sync {
    /// Check out a shadow instance for the given PG major.
    async fn checkout(&self, major: PgMajor) -> anyhow::Result<Box<dyn ShadowGuard>>;
}

/// Resolve the configured backend from the `[shadow]` section.
///
/// Selection order when `backend = "auto"` (the default):
/// 1. If `url` or `url_env` is present → `dsn`
/// 2. If Docker is available → `testcontainers`
/// 3. Error
pub fn resolve(config: &crate::config::ShadowConfig) -> anyhow::Result<Box<dyn ShadowBackend>> {
    match config.backend.as_deref().unwrap_or("auto") {
        "testcontainers" => Ok(Box::new(
            testcontainers::TestcontainersBackend::new(config),
        )),
        "dsn" => Ok(Box::new(dsn::DsnBackend::new(config)?)),
        "auto" => {
            if config.url.is_some() || config.url_env.is_some() {
                Ok(Box::new(dsn::DsnBackend::new(config)?))
            } else if docker_available() {
                Ok(Box::new(testcontainers::TestcontainersBackend::new(config)))
            } else {
                anyhow::bail!(
                    "no shadow backend available: configure [shadow].url or install Docker"
                )
            }
        }
        other => anyhow::bail!("unknown shadow backend: {other}"),
    }
}

/// Quick check: is Docker available on this host?
///
/// Returns `false` if `PGEVOLVE_DISABLE_DOCKER_TESTS` is set, or if
/// `docker info` fails.
#[must_use]
pub fn docker_available() -> bool {
    if std::env::var_os("PGEVOLVE_DISABLE_DOCKER_TESTS").is_some() {
        return false;
    }
    std::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Install Postgres extensions into the database at `url`.
///
/// Every name is validated against `[a-zA-Z_][a-zA-Z0-9_]*` before being
/// formatted into SQL, eliminating the SQL-injection surface that would
/// otherwise exist when names come from user config.
pub(super) async fn install_extensions(
    url: &str,
    extensions: &[String],
) -> anyhow::Result<()> {
    for ext in extensions {
        validate_extension_name(ext)?;
    }
    let (client, conn) = tokio_postgres::connect(url, tokio_postgres::NoTls).await?;
    tokio::spawn(conn);
    for ext in extensions {
        // Validated above; safe to format.  Double-quoting is belt-and-suspenders.
        let stmt = format!("CREATE EXTENSION IF NOT EXISTS \"{}\"", ext.replace('"', "\"\""));
        client.batch_execute(&stmt).await?;
    }
    Ok(())
}

fn validate_extension_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("[shadow].extensions: empty extension name");
    }
    let valid = name.chars().enumerate().all(|(i, c)| {
        if i == 0 {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        }
    });
    if !valid {
        anyhow::bail!(
            "[shadow].extensions: {name:?} is not a valid extension identifier \
             (expected [a-zA-Z_][a-zA-Z0-9_]*)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_valid_names() {
        for valid in &["pg_trgm", "uuid_ossp", "vector", "_underscore_first", "p"] {
            assert!(validate_extension_name(valid).is_ok(), "{valid} should validate");
        }
    }

    #[test]
    fn validate_rejects_invalid_names() {
        for bad in &[
            "",
            "1leading_digit",
            "has space",
            "semicolon;here",
            "dash-separated",
            "pg_trgm; DROP TABLE x",
        ] {
            assert!(validate_extension_name(bad).is_err(), "{bad:?} should reject");
        }
    }
}
