//! `status` command queries over the `pgevolve.apply_log` and
//! `pgevolve.plan_steps` tables.
//!
//! Two query primitives plus human and JSON formatters. Callers compose
//! them into a CLI subcommand in Phase 9.

#![allow(clippy::format_push_string)]

use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio_postgres::Client;
use uuid::Uuid;

use super::error::ApplyError;

/// One row from `pgevolve.apply_log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyRecord {
    /// Apply id.
    pub apply_id: Uuid,
    /// Short plan id (16-char hex).
    pub plan_id: String,
    /// pgevolve version that ran the apply.
    pub pgevolve_version: String,
    /// Optional source-tree revision.
    pub source_rev: Option<String>,
    /// Target-database identity hash.
    pub target_identity: String,
    /// Best-effort actor string.
    pub actor: Option<String>,
    /// When the apply started (RFC3339 UTC).
    pub started_at: String,
    /// When the apply finished, if any.
    pub finished_at: Option<String>,
    /// Final status: `running` / `succeeded` / `failed` / `aborted`.
    pub status: String,
    /// Captured error message on failure.
    pub error_message: Option<String>,
}

/// One row from `pgevolve.plan_steps`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepRecord {
    /// Apply this step belongs to.
    pub apply_id: Uuid,
    /// 1-indexed step number across the plan.
    pub step_no: i32,
    /// 1-indexed group number.
    pub group_no: i32,
    /// `StepKind` name (e.g., `create_table`).
    pub kind: String,
    /// Destructive flag.
    pub destructive: bool,
    /// Rendered targets.
    pub targets: Vec<String>,
    /// SQL body actually executed.
    pub sql_text: String,
    /// When the step started, if any.
    pub started_at: Option<String>,
    /// When the step finished, if any.
    pub finished_at: Option<String>,
    /// Final step status.
    pub status: String,
    /// Captured error message on failure.
    pub error_message: Option<String>,
}

/// Most recent `apply_log` rows, ordered by `started_at` descending.
pub async fn fetch_recent_applies(
    client: &Client,
    limit: i64,
) -> Result<Vec<ApplyRecord>, ApplyError> {
    let rows = client
        .query(
            "SELECT apply_id, plan_id, pgevolve_version, source_rev, target_identity,
                    actor, started_at, finished_at, status, error_message
             FROM pgevolve.apply_log
             ORDER BY started_at DESC
             LIMIT $1",
            &[&limit],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(ApplyRecord {
            apply_id: r.get(0),
            plan_id: r.get(1),
            pgevolve_version: r.get(2),
            source_rev: r.get(3),
            target_identity: r.get(4),
            actor: r.get(5),
            started_at: format_ts(r.get(6)),
            finished_at: r
                .get::<_, Option<OffsetDateTime>>(7)
                .map(format_ts),
            status: r.get(8),
            error_message: r.get(9),
        });
    }
    Ok(out)
}

/// Every step row for one apply, in step-number order.
pub async fn fetch_apply_steps(
    client: &Client,
    apply_id: Uuid,
) -> Result<Vec<StepRecord>, ApplyError> {
    let rows = client
        .query(
            "SELECT apply_id, step_no, group_no, kind, destructive, targets,
                    sql_text, started_at, finished_at, status, error_message
             FROM pgevolve.plan_steps
             WHERE apply_id=$1
             ORDER BY step_no",
            &[&apply_id],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(StepRecord {
            apply_id: r.get(0),
            step_no: r.get(1),
            group_no: r.get(2),
            kind: r.get(3),
            destructive: r.get(4),
            targets: r.get(5),
            sql_text: r.get(6),
            started_at: r
                .get::<_, Option<OffsetDateTime>>(7)
                .map(format_ts),
            finished_at: r
                .get::<_, Option<OffsetDateTime>>(8)
                .map(format_ts),
            status: r.get(9),
            error_message: r.get(10),
        });
    }
    Ok(out)
}

/// Human-friendly multi-line summary of one apply.
pub fn format_status_human(record: &ApplyRecord, steps: &[StepRecord]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "apply {}  plan={}  status={}\n",
        record.apply_id, record.plan_id, record.status,
    ));
    s.push_str(&format!(
        "  started_at={}  finished_at={}\n",
        record.started_at,
        record.finished_at.as_deref().unwrap_or("(running)"),
    ));
    s.push_str(&format!(
        "  pgevolve={}  source_rev={}  target={}\n",
        record.pgevolve_version,
        record.source_rev.as_deref().unwrap_or("-"),
        record.target_identity,
    ));
    if let Some(err) = &record.error_message {
        s.push_str(&format!("  error: {err}\n"));
    }
    s.push_str(&format!("  steps ({}):\n", steps.len()));
    for st in steps {
        s.push_str(&format!(
            "    [{:>3}] g{} {} {}  status={}\n",
            st.step_no,
            st.group_no,
            st.kind,
            if st.destructive { "(destructive)" } else { "" },
            st.status,
        ));
    }
    s
}

/// JSON-encoded summary; never errors because [`ApplyRecord`] and
/// [`StepRecord`] are infallibly serializable.
pub fn format_status_json(record: &ApplyRecord, steps: &[StepRecord]) -> String {
    #[derive(Serialize)]
    struct Wrapper<'a> {
        apply: &'a ApplyRecord,
        steps: &'a [StepRecord],
    }
    serde_json::to_string_pretty(&Wrapper {
        apply: record,
        steps,
    })
    .expect("serializable")
}

fn format_ts(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}
