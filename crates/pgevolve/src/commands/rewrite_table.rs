//! `pgevolve rewrite-table` — destructive table-rewrite operation.
//!
//! v0.2 skeleton. Accepts the qname and `--confirm-rewrite` flag, then errors
//! with a v0.2-not-yet-implemented pointer. The CLI surface lands now so users
//! discover the command; the actual implementation arrives with the upcoming
//! v0.2 partitioning / column-type-change sub-spec.
//!
//! See arch spec Decision 17.

use anyhow::Result;

/// Run `pgevolve rewrite-table`.
pub fn run(qname: &str, _env: &str, confirm: bool) -> Result<i32> {
    if !confirm {
        anyhow::bail!(
            "pgevolve rewrite-table is destructive (downtime, full table rewrite). \
             Pass --confirm-rewrite to proceed.",
        );
    }
    anyhow::bail!(
        "rewrite-table {qname} is recognized but not yet implemented in v0.2 readiness. \
         See arch spec Decision 17 and the upcoming v0.2 partitioning / \
         column-type-change sub-spec for the implementation venue.",
    )
}
