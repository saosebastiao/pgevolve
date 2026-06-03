//! Decode `pg_subscription` rows into [`Subscription`] IR.
//!
//! `pg_subscription` is superuser-readable only. The catalog reader catches
//! permission errors at the query layer (returning empty rows) and surfaces
//! the gap via [`crate::catalog::DriftReport::unreadable_subscriptions`].
//!
//! Column availability by PG version (handled by per-version SQL in
//! `queries/shared.rs` and `queries/pg14.rs`):
//!   - `subdisableonerr`                          — PG 15+
//!   - `subpasswordrequired`, `subrunasowner`, `suborigin` — PG 16+
//!   - `subfailover`                              — PG 17+
//!
//! The PG 14 override substitutes `NULL::bool` / `NULL::text` for the missing
//! columns, so this decoder uses a single code path for all supported versions.

// `CatalogError` can embed large variants. Cold-path catalog reads; boxing
// adds noise without benefit.
#![allow(clippy::result_large_err)]

use crate::catalog::CatalogQuery;
use crate::catalog::error::CatalogError;
use crate::catalog::rows::Row;
use crate::identifier::Identifier;
use crate::ir::subscription::{OriginMode, StreamingMode, Subscription, SubscriptionOptions};

const Q: CatalogQuery = CatalogQuery::Subscriptions;

/// Decode a single `pg_subscription` row into a [`Subscription`].
pub fn decode_subscription_row(row: &Row) -> Result<Subscription, CatalogError> {
    let name_str = row.get_text(Q, "name")?;
    let owner_str = row.get_text(Q, "owner")?;
    let comment_str = row.get_text(Q, "comment")?;
    let connection = row.get_text(Q, "connection")?;

    let name = Identifier::from_unquoted(&name_str).map_err(|e| CatalogError::BadColumnType {
        query: Q,
        column: "name".to_string(),
        message: format!("invalid subscription name {name_str:?}: {e}"),
    })?;

    let owner = if owner_str.is_empty() {
        None
    } else {
        Some(
            Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::BadColumnType {
                query: Q,
                column: "owner".to_string(),
                message: format!("invalid owner name {owner_str:?}: {e}"),
            })?,
        )
    };

    let comment = if comment_str.is_empty() {
        None
    } else {
        Some(comment_str)
    };

    // Decode publications list.
    let pub_names = row.get_text_array(Q, "publications")?;
    let publications = pub_names
        .into_iter()
        .map(|s| {
            Identifier::from_unquoted(&s).map_err(|e| CatalogError::BadColumnType {
                query: Q,
                column: "publications".to_string(),
                message: format!("invalid publication name {s:?}: {e}"),
            })
        })
        .collect::<Result<Vec<Identifier>, CatalogError>>()?;

    let options = decode_options(row)?;

    Ok(Subscription {
        name,
        connection,
        publications,
        options,
        owner,
        comment,
    })
}

/// Decode `SubscriptionOptions` from a `pg_subscription` row.
///
/// Separated from [`decode_subscription_row`] to keep each function under the
/// line-count lint threshold.
fn decode_options(row: &Row) -> Result<SubscriptionOptions, CatalogError> {
    let enabled = Some(row.get_bool(Q, "enabled")?);

    // Slot name — empty string means "use the subscription name" (PG default).
    let slot_name_str = row.get_text(Q, "slot_name")?;
    let slot_name = if slot_name_str.is_empty() {
        None
    } else {
        Some(Identifier::from_unquoted(&slot_name_str).map_err(|e| {
            CatalogError::BadColumnType {
                query: Q,
                column: "slot_name".to_string(),
                message: format!("invalid slot name {slot_name_str:?}: {e}"),
            }
        })?)
    };

    // Synchronous commit — empty means "off"; guard against empty.
    let synchronous_commit_str = row.get_text(Q, "synchronous_commit")?;
    let synchronous_commit = if synchronous_commit_str.is_empty() {
        None
    } else {
        Some(synchronous_commit_str)
    };

    let binary = Some(row.get_bool(Q, "binary")?);

    // Streaming mode.
    let streaming_str = row.get_text(Q, "streaming")?;
    let streaming = Some(decode_streaming(&streaming_str).map_err(|e| match e {
        CatalogError::BadColumnType { message, .. } => CatalogError::BadColumnType {
            query: Q,
            column: "streaming".to_string(),
            message,
        },
        other => other,
    })?);

    // Two-phase state — 'd'/'e'/'p' (PG 15+). NULL on PG 14 → `None`.
    let two_phase = if row.is_null("two_phase_state") {
        None
    } else {
        let two_phase_str = row.get_text(Q, "two_phase_state")?;
        Some(decode_two_phase(&two_phase_str).map_err(|e| match e {
            CatalogError::BadColumnType { message, .. } => CatalogError::BadColumnType {
                query: Q,
                column: "two_phase_state".to_string(),
                message,
            },
            other => other,
        })?)
    };

    // Version-gated bool fields — NULL on PG versions that don't have them.
    let disable_on_error = opt_bool(row, "disable_on_error")?;
    let password_required = opt_bool(row, "password_required")?;
    let run_as_owner = opt_bool(row, "run_as_owner")?;
    let failover = opt_bool(row, "failover")?;

    let origin = if row.is_null("origin") {
        None
    } else {
        let origin_str = row.get_text(Q, "origin")?;
        Some(decode_origin(&origin_str).map_err(|e| match e {
            CatalogError::BadColumnType { message, .. } => CatalogError::BadColumnType {
                query: Q,
                column: "origin".to_string(),
                message,
            },
            other => other,
        })?)
    };

    // `connect`, `create_slot`, and `copy_data` are CREATE-time-only;
    // pg_subscription does not store them. Always None when reading from
    // catalog.
    Ok(SubscriptionOptions {
        enabled,
        slot_name,
        connect: None,    // CREATE-only; not stored in pg_subscription.
        create_slot: None,
        copy_data: None,
        synchronous_commit,
        binary,
        streaming,
        two_phase,
        disable_on_error,
        password_required,
        run_as_owner,
        origin,
        failover,
    })
}

/// Read an optional bool column: `None` when the column is NULL (not supported
/// on this PG version), `Some(v)` otherwise.
fn opt_bool(row: &Row, col: &str) -> Result<Option<bool>, CatalogError> {
    if row.is_null(col) {
        Ok(None)
    } else {
        Ok(Some(row.get_bool(Q, col)?))
    }
}

// ---- helpers ------------------------------------------------------------------

/// Decode `substream` text to [`StreamingMode`].
///
/// PG 14 stores `substream` as `bool` (`f`/`t`), cast to `text` by the SQL.
/// PG 16+ uses `text` directly with values `'f'`/`'t'`/`'p'`.
pub fn decode_streaming(s: &str) -> Result<StreamingMode, CatalogError> {
    match s {
        "f" => Ok(StreamingMode::Off),
        "t" => Ok(StreamingMode::On),
        "p" => Ok(StreamingMode::Parallel),
        other => Err(CatalogError::BadColumnType {
            query: Q,
            column: "streaming".to_string(),
            message: format!("unknown substream value: {other:?}"),
        }),
    }
}

/// Decode `subtwophasestate` single-char code to `bool`.
///
/// - `'d'` = disabled → `false`
/// - `'e'` = enabled  → `true`
/// - `'p'` = pending (transient) → `true` (matches eventual steady state)
pub fn decode_two_phase(s: &str) -> Result<bool, CatalogError> {
    match s {
        "d" => Ok(false),
        // 'p' (pending) is transient; treat as enabled for diff (matches the eventual state).
        "e" | "p" => Ok(true),
        other => Err(CatalogError::BadColumnType {
            query: Q,
            column: "two_phase_state".to_string(),
            message: format!("unknown subtwophasestate value: {other:?}"),
        }),
    }
}

/// Decode `suborigin` text to [`OriginMode`].
pub fn decode_origin(s: &str) -> Result<OriginMode, CatalogError> {
    match s.to_ascii_lowercase().as_str() {
        "any" => Ok(OriginMode::Any),
        "none" => Ok(OriginMode::None),
        other => Err(CatalogError::BadColumnType {
            query: Q,
            column: "origin".to_string(),
            message: format!("unknown suborigin value: {other:?}"),
        }),
    }
}

// ---- tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // decode_streaming -----------------------------------------------------------

    #[test]
    fn streaming_off_decodes() {
        assert_eq!(decode_streaming("f").unwrap(), StreamingMode::Off);
    }

    #[test]
    fn streaming_on_decodes() {
        assert_eq!(decode_streaming("t").unwrap(), StreamingMode::On);
    }

    #[test]
    fn streaming_parallel_decodes() {
        assert_eq!(decode_streaming("p").unwrap(), StreamingMode::Parallel);
    }

    #[test]
    fn streaming_unknown_errors() {
        let err = decode_streaming("x").unwrap_err();
        assert!(
            format!("{err}").contains("unknown substream value"),
            "error should mention unknown value: {err}"
        );
    }

    // decode_two_phase -----------------------------------------------------------

    #[test]
    fn two_phase_disabled_decodes() {
        assert!(!decode_two_phase("d").unwrap());
    }

    #[test]
    fn two_phase_enabled_decodes() {
        assert!(decode_two_phase("e").unwrap());
    }

    #[test]
    fn two_phase_pending_decodes_as_true() {
        // 'p' (pending) is transient — we treat it as the eventual enabled state.
        assert!(decode_two_phase("p").unwrap());
    }

    #[test]
    fn two_phase_unknown_errors() {
        let err = decode_two_phase("z").unwrap_err();
        assert!(
            format!("{err}").contains("unknown subtwophasestate value"),
            "error should mention unknown value: {err}"
        );
    }

    // decode_origin --------------------------------------------------------------

    #[test]
    fn origin_any_lowercase_decodes() {
        assert_eq!(decode_origin("any").unwrap(), OriginMode::Any);
    }

    #[test]
    fn origin_any_uppercase_decodes() {
        assert_eq!(decode_origin("ANY").unwrap(), OriginMode::Any);
    }

    #[test]
    fn origin_none_decodes() {
        assert_eq!(decode_origin("none").unwrap(), OriginMode::None);
    }

    #[test]
    fn origin_unknown_errors() {
        let err = decode_origin("both").unwrap_err();
        assert!(
            format!("{err}").contains("unknown suborigin value"),
            "error should mention unknown value: {err}"
        );
    }
}
