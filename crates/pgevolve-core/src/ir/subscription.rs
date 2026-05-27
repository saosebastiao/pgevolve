//! Subscription IR — declarative logical-replication subscriber-side metadata.
//!
//! A `Subscription` is a Postgres `CREATE SUBSCRIPTION` object. It lives at
//! the Catalog top level (not schema-qualified) because Postgres treats
//! subscriptions as a per-database global namespace.
//!
//! The `connection` field stores the libpq connection string verbatim,
//! including unresolved `${VAR}` env-var references. Resolution happens at
//! apply-time preflight (`crates/pgevolve/src/executor/env_interp.rs`),
//! never at parse or canon time. The source IR — and therefore plan.sql —
//! never contains resolved secrets.
//!
//! Spec: `docs/superpowers/specs/2026-05-26-subscriptions-design.md`.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;

/// Declarative model of a Postgres `SUBSCRIPTION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    /// Subscription name (not schema-qualified — subscriptions are global).
    pub name: Identifier,
    /// libpq connection string. May contain `${VAR}` env-var references that
    /// are resolved at apply-time preflight. Stored verbatim through parse,
    /// canon, diff, and plan serialization.
    pub connection: String,
    /// Publications this subscription reads from. Sorted + deduped by canon.
    /// Non-empty (enforced by canon).
    pub publications: Vec<Identifier>,
    /// Per-field lenient WITH options.
    pub options: SubscriptionOptions,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER SUBSCRIPTION ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

/// Per-field lenient WITH options for a `Subscription`.
///
/// Every field is `Option<T>`. `None` = unmanaged (pgevolve neither sets
/// nor resets); `Some(value)` = managed (diff emits an ALTER to match).
/// Matches the v0.3.3 reloptions per-field-lenient pattern.
///
/// **CREATE-only fields**: `create_slot` and `copy_data` are PG-CREATE-only
/// (no `ALTER SUBSCRIPTION s SET (create_slot = …)` exists). They flow into
/// the IR from source CREATE statements so users can declare them, but the
/// differ NEVER includes them in `AlterSubscriptionSetOptions` deltas, and
/// the catalog reader ALWAYS returns `None` for them (`pg_subscription`
/// doesn't store the CREATE-time decision). See `diff::subscriptions::options_delta`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SubscriptionOptions {
    /// Whether the subscription is enabled. PG default: true.
    pub enabled: Option<bool>,
    /// Replication slot name on the publisher. `None` = use subscription name.
    pub slot_name: Option<Identifier>,
    /// Whether `CREATE SUBSCRIPTION` should create the publisher-side slot.
    /// PG default: true.
    pub create_slot: Option<bool>,
    /// Whether to copy existing rows during initial sync. PG default: true.
    pub copy_data: Option<bool>,
    /// `synchronous_commit` GUC value for the subscription's apply worker.
    /// Free-form string (e.g., `"on"`, `"off"`, `"remote_write"`, `"local"`).
    pub synchronous_commit: Option<String>,
    /// Use binary copy / binary replication protocol. PG default: false.
    pub binary: Option<bool>,
    /// Streaming mode for large in-progress transactions.
    pub streaming: Option<StreamingMode>,
    /// Two-phase commit handling. PG 14+; default: false.
    pub two_phase: Option<bool>,
    /// Disable the subscription on apply error. PG 15+; default: false.
    pub disable_on_error: Option<bool>,
    /// Whether the subscription owner must supply a password. PG 16+; default: true.
    pub password_required: Option<bool>,
    /// Run the apply worker as the subscription owner (instead of the table owner).
    /// PG 16+; default: false.
    pub run_as_owner: Option<bool>,
    /// Replication origin handling. PG 16+; default: Any.
    pub origin: Option<OriginMode>,
    /// Whether the subscription survives failover. PG 17+; default: false.
    pub failover: Option<bool>,
}

/// `streaming` mode for in-progress transactions on a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamingMode {
    /// Stream nothing; spool to disk at the subscriber.
    Off,
    /// Stream in-progress transactions to disk on the subscriber.
    On,
    /// Stream in-progress transactions to parallel apply workers. PG 16+.
    Parallel,
}

/// `origin` mode for replication-origin handling on a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginMode {
    /// Replicate all changes regardless of origin (default).
    Any,
    /// Replicate only changes from non-replicated sources (avoid loops).
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_all_none() {
        let o = SubscriptionOptions::default();
        assert!(o.enabled.is_none());
        assert!(o.slot_name.is_none());
        assert!(o.create_slot.is_none());
        assert!(o.copy_data.is_none());
        assert!(o.synchronous_commit.is_none());
        assert!(o.binary.is_none());
        assert!(o.streaming.is_none());
        assert!(o.two_phase.is_none());
        assert!(o.disable_on_error.is_none());
        assert!(o.password_required.is_none());
        assert!(o.run_as_owner.is_none());
        assert!(o.origin.is_none());
        assert!(o.failover.is_none());
    }

    #[test]
    fn streaming_off_does_not_equal_on() {
        assert_ne!(StreamingMode::Off, StreamingMode::On);
        assert_ne!(StreamingMode::On, StreamingMode::Parallel);
    }

    #[test]
    fn origin_any_does_not_equal_none() {
        assert_ne!(OriginMode::Any, OriginMode::None);
    }
}
