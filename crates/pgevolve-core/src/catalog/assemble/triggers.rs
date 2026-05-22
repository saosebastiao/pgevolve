//! Trigger assembly from `pg_trigger` catalog rows.
//!
//! Called from [`super::assemble`] to build [`crate::ir::trigger::Trigger`]
//! IR entries by re-parsing `pg_get_triggerdef` output.

use std::path::PathBuf;

use pg_query::NodeEnum;

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::ir::trigger::Trigger;
use crate::parse::error::SourceLocation;

/// Re-parse `pg_get_triggerdef` output and build [`Trigger`] IR.
pub(super) fn build_triggers(rows: &[Row]) -> Result<Vec<Trigger>, CatalogError> {
    use crate::parse::builder::create_trigger_stmt::build_trigger;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Triggers;
        let triggerdef = r.get_text(q, "triggerdef")?;
        let parsed = pg_query::parse(&triggerdef).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "pg_get_triggerdef returned invalid SQL: {e}: {triggerdef}"
            )))
        })?;
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .ok_or_else(|| {
                CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                    "pg_get_triggerdef returned no statement".into(),
                ))
            })?;
        let NodeEnum::CreateTrigStmt(trig_stmt) = stmt else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "pg_get_triggerdef did not yield CreateTrigStmt".into(),
            )));
        };
        let location = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
        let mut trigger = build_trigger(&trig_stmt, &location).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "rebuild trigger from pg_get_triggerdef: {e}"
            )))
        })?;
        trigger.comment = r.get_opt_text(q, "comment")?;
        out.push(trigger);
    }
    Ok(out)
}
