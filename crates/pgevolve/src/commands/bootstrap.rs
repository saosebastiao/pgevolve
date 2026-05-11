//! `pgevolve bootstrap` — install or upgrade the pgevolve metadata schema.

use anyhow::Result;

use crate::cli::BootstrapArgs;
use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};

/// Run `pgevolve bootstrap`.
pub async fn run(args: BootstrapArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let mut client = connect(&opts).await?;
    crate::executor::bootstrap_metadata(&mut client).await?;
    println!("pgevolve metadata schema is up to date");
    Ok(0)
}
