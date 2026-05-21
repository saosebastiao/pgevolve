//! DSN-backed shadow Postgres.
//!
//! Connects to a user-supplied Postgres instance whose DSN is provided
//! via `[shadow].url` (literal DSN) or `[shadow].url_env` (name of an
//! environment variable holding the DSN).
//!
//! Before handing out the guard the backend runs a reset operation
//! (default: `drop_schema_cascade`) so each checkout starts clean.

use anyhow::Result;
use tokio_postgres::NoTls;

use crate::config::ShadowConfig;

use super::{CheckoutFuture, PgMajor, ResetFuture, ShadowBackend, ShadowGuard};

/// Backend that connects to a caller-supplied Postgres DSN.
pub struct DsnBackend {
    base_url: String,
    reset: ResetPolicy,
    extensions: Vec<String>,
}

#[derive(Debug, Clone)]
enum ResetPolicy {
    DropSchemaCascade,
    None,
}

impl DsnBackend {
    /// Construct from the `[shadow]` config section.
    pub fn new(config: &ShadowConfig) -> Result<Self> {
        let base_url = config
            .url
            .clone()
            .or_else(|| config.url_env.as_ref().and_then(|k| std::env::var(k).ok()))
            .ok_or_else(|| {
                anyhow::anyhow!("[shadow].url or [shadow].url_env required for dsn backend")
            })?;

        let reset = match config.reset.as_deref().unwrap_or("drop_schema_cascade") {
            "drop_schema_cascade" => ResetPolicy::DropSchemaCascade,
            "none" => ResetPolicy::None,
            other => anyhow::bail!("unknown [shadow].reset: {other}"),
        };

        Ok(Self {
            base_url,
            reset,
            extensions: config.extensions.clone(),
        })
    }
}

impl ShadowBackend for DsnBackend {
    fn checkout(&self, _major: PgMajor) -> CheckoutFuture<'_> {
        Box::pin(async move {
            let guard = DsnGuard {
                url: self.base_url.clone(),
                reset: self.reset.clone(),
            };
            guard.reset_now().await?;
            if !self.extensions.is_empty() {
                super::install_extensions(&guard.url, &self.extensions).await?;
            }
            Ok(Box::new(guard) as Box<dyn ShadowGuard>)
        })
    }
}

struct DsnGuard {
    url: String,
    reset: ResetPolicy,
}

impl DsnGuard {
    async fn reset_now(&self) -> Result<()> {
        let (client, conn) = tokio_postgres::connect(&self.url, NoTls).await?;
        tokio::spawn(conn);
        match &self.reset {
            ResetPolicy::None => Ok(()),
            ResetPolicy::DropSchemaCascade => {
                // Drop every non-system schema, then recreate `public`.
                client
                    .batch_execute(
                        "DO $$
                         DECLARE r record;
                         BEGIN
                           FOR r IN SELECT nspname FROM pg_namespace
                                    WHERE nspname NOT IN ('pg_catalog','information_schema','pg_toast','public')
                                      AND nspname NOT LIKE 'pg_temp_%'
                                      AND nspname NOT LIKE 'pg_toast_temp_%'
                           LOOP EXECUTE format('DROP SCHEMA %I CASCADE', r.nspname); END LOOP;
                           EXECUTE 'DROP SCHEMA public CASCADE';
                           EXECUTE 'CREATE SCHEMA public';
                         END $$;",
                    )
                    .await?;
                Ok(())
            }
        }
    }
}

impl ShadowGuard for DsnGuard {
    fn url(&self) -> &str {
        &self.url
    }

    fn reset(&mut self) -> ResetFuture<'_> {
        Box::pin(self.reset_now())
    }
}
