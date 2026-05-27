//! Assemble `pg_subscription` rows into `Vec<Subscription>`.
//!
//! `pg_subscription` is superuser-only. This module is intentionally thin —
//! the permission-error detection lives one level up in `read_catalog`
//! (which catches the `QueryFailed` error from the querier), so by the time
//! `assemble_subscriptions` is called the rows are already in hand (possibly
//! empty after a privilege-denied fallback).

// `CatalogError` can embed large variants. Cold-path catalog reads; boxing
// adds noise without benefit.
#![allow(clippy::result_large_err)]

use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::catalog::subscriptions::decode_subscription_row;
use crate::ir::subscription::Subscription;

/// Assemble all subscription rows into `Vec<Subscription>`.
///
/// Accepts pre-fetched rows from the `Subscriptions` catalog query.
/// Called by the top-level `assemble()` orchestrator with a (possibly empty)
/// slice — empty when the connection lacked `pg_subscription` read access.
pub(super) fn assemble_subscriptions(rows: &[Row]) -> Result<Vec<Subscription>, CatalogError> {
    let mut subscriptions = Vec::with_capacity(rows.len());
    for row in rows {
        subscriptions.push(decode_subscription_row(row)?);
    }
    Ok(subscriptions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::rows::Value;

    /// Build a minimal well-formed subscription row.
    fn sub_row(name: &str) -> Row {
        Row::new()
            .with("oid", Value::Integer(1))
            .with("name", Value::Text(name.to_string()))
            .with("owner", Value::Text("pg_user".to_string()))
            .with("enabled", Value::Bool(true))
            .with(
                "connection",
                Value::Text("host=127.0.0.1 port=5432 dbname=pub".to_string()),
            )
            .with("slot_name", Value::Text(String::new()))
            .with("synchronous_commit", Value::Text("off".to_string()))
            .with("publications", Value::TextArray(vec!["pub_a".to_string()]))
            .with("binary", Value::Bool(false))
            .with("streaming", Value::Text("f".to_string()))
            .with("two_phase_state", Value::Text("d".to_string()))
            .with("disable_on_error", Value::Null)
            .with("password_required", Value::Null)
            .with("run_as_owner", Value::Null)
            .with("origin", Value::Null)
            .with("failover", Value::Null)
            .with("comment", Value::Text(String::new()))
    }

    #[test]
    fn empty_rows_returns_empty_vec() {
        let subs = assemble_subscriptions(&[]).unwrap();
        assert!(subs.is_empty());
    }

    #[test]
    fn single_row_assembles_correctly() {
        let rows = vec![sub_row("my_sub")];
        let subs = assemble_subscriptions(&rows).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name.as_str(), "my_sub");
        assert_eq!(subs[0].publications.len(), 1);
        assert_eq!(subs[0].publications[0].as_str(), "pub_a");
        assert_eq!(subs[0].options.enabled, Some(true));
        assert_eq!(subs[0].options.binary, Some(false));
        assert_eq!(subs[0].options.create_slot, None);
        assert_eq!(subs[0].options.copy_data, None);
    }

    #[test]
    fn multiple_rows_all_assembled() {
        let rows = vec![sub_row("alpha"), sub_row("beta")];
        let subs = assemble_subscriptions(&rows).unwrap();
        assert_eq!(subs.len(), 2);
    }
}
