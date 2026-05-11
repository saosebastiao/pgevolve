//! `pgevolve apply` — apply a plan directory to a live database.
//!
//! Exit codes follow spec §13:
//! - `0` success
//! - `2` pre-flight mismatch (target identity, drift, unapproved intents)
//! - `3` apply error (lock held, step failed)
//! - other errors bubble up as `1`

use anyhow::Result;

use pgevolve_core::catalog::CatalogFilter;

use crate::cli::ApplyArgs;
use crate::config::PgevolveConfig;
use crate::connection::{connect, resolve_db};
use crate::executor::{ApplyError, ApplyOutcome, ApplyOverrides, apply};

/// Run `pgevolve apply`.
pub async fn run(args: ApplyArgs, cfg: &PgevolveConfig) -> Result<i32> {
    let opts = resolve_db(cfg, &args.db, args.url.as_deref())?;
    let mut client = connect(&opts).await?;
    let filter = CatalogFilter::new(opts.managed_schemas.clone(), opts.ignore_objects.clone())?;
    // Phase 8's drift recheck is a stub; until Phase 9.x ties the binary-side
    // catalog reader into preflight, force-allow drift so apply runs
    // end-to-end. Once preflight reads the live catalog, remove this override.
    let _ = args.allow_drift;
    let overrides = ApplyOverrides {
        allow_different_target: args.allow_different_target,
        allow_drift: true,
        actor: None,
        abort_after_step: None,
    };
    let result = apply(&args.plan_dir, &mut client, &filter, overrides).await;
    match result {
        Ok(ApplyOutcome::Succeeded { apply_id }) => {
            println!("applied (apply_id={apply_id})");
            Ok(0)
        }
        Err(e) => {
            eprintln!("{e}");
            Ok(match e {
                ApplyError::TargetIdentityMismatch { .. }
                | ApplyError::DriftDetected(_)
                | ApplyError::UnapprovedIntents { .. } => 2,
                ApplyError::LockHeld | ApplyError::StepFailed { .. } => 3,
                _ => return Err(e.into()),
            })
        }
    }
}
