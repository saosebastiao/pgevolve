//! `pgevolve status` — recent applies + per-step detail.

use anyhow::Result;
use uuid::Uuid;

use crate::cli::{OutputFormat, StatusArgs};
use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};
use crate::executor::status::{
    fetch_apply_steps, fetch_recent_applies, format_status_human, format_status_json,
};

/// Run `pgevolve status`.
pub async fn run(args: StatusArgs, cfg: &PgevolveConfig, format: OutputFormat) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let client = connect(&opts).await?;

    let recent = fetch_recent_applies(&client, i64::from(args.limit.max(1))).await?;

    let target_id = args
        .apply_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|e| anyhow::anyhow!("invalid --apply-id: {e}"))?;

    if let Some(id) = target_id {
        let Some(rec) = recent.iter().find(|r| r.apply_id == id) else {
            return Err(anyhow::anyhow!(
                "no apply with id {id} in the most recent {}",
                args.limit
            ));
        };
        let steps = fetch_apply_steps(&client, id).await?;
        match format {
            OutputFormat::Human | OutputFormat::Sql => {
                println!("{}", format_status_human(rec, &steps));
            }
            OutputFormat::Json => println!("{}", format_status_json(rec, &steps)?),
        }
        return Ok(0);
    }

    match format {
        OutputFormat::Human | OutputFormat::Sql => {
            if recent.is_empty() {
                println!("no applies recorded");
            } else {
                println!("{} recent apply/applies:", recent.len());
                for r in &recent {
                    println!(
                        "  {}  plan={}  status={}  started={}  finished={}",
                        r.apply_id,
                        r.plan_id,
                        r.status,
                        r.started_at,
                        r.finished_at.as_deref().unwrap_or("(running)"),
                    );
                }
            }
        }
        OutputFormat::Json => {
            let s = serde_json::to_string_pretty(&recent)?;
            println!("{s}");
        }
    }
    Ok(0)
}
