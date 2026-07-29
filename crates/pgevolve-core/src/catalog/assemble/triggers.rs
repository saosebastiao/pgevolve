//! Trigger assembly from `pg_trigger` catalog rows.
//!
//! Called from [`super::assemble`] to build [`crate::ir::trigger::Trigger`]
//! IR entries by re-parsing `pg_get_triggerdef` output.

use std::path::PathBuf;

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::ir::trigger::Trigger;
use crate::parse::error::SourceLocation;
use crate::parse::from_catalog;

/// Re-parse `pg_get_triggerdef` output and build [`Trigger`] IR.
pub(super) fn build_triggers(rows: &[Row]) -> Result<Vec<Trigger>, CatalogError> {
    let location = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Triggers;
        let triggerdef = r.get_text(q, "triggerdef")?;
        let mut trigger =
            from_catalog::trigger(&triggerdef, &location).map_err(CatalogError::from)?;
        trigger.comment = r.get_opt_text(q, "comment")?;
        out.push(trigger);
    }
    Ok(out)
}
