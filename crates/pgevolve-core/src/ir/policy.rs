//! Row-level security policies — `Policy`, `PolicyCommand`.
//!
//! Policies embed on [`crate::ir::table::Table`] — there's no orphan
//! shape possible in PG. USING / WITH CHECK expressions reuse the
//! [`NormalizedExpr`] canonicalization shared with check constraints.

use serde::{Deserialize, Serialize};

use crate::identifier::Identifier;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::grant::GrantTarget;

/// A row-level security policy attached to a table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Policy {
    /// Policy name. Unique per table.
    pub name: Identifier,
    /// `AS PERMISSIVE` (true; PG default) vs `AS RESTRICTIVE` (false).
    pub permissive: bool,
    /// Which command(s) this policy applies to. `All` covers all DML.
    pub command: PolicyCommand,
    /// `TO roles` list. Source omission canonicalizes to
    /// `vec![GrantTarget::Public]` at parse time so source and catalog
    /// round-trip equally.
    pub roles: Vec<GrantTarget>,
    /// `USING (expr)` — row-visibility filter. PG default: absent.
    pub using: Option<NormalizedExpr>,
    /// `WITH CHECK (expr)` — write-time filter. Valid only on commands
    /// that write rows (Insert/Update/All); parser rejects on Select/Delete.
    pub with_check: Option<NormalizedExpr>,
}

/// The command kind a policy applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyCommand {
    /// `FOR ALL` — covers SELECT, INSERT, UPDATE, DELETE.
    All,
    /// `FOR SELECT`.
    Select,
    /// `FOR INSERT`.
    Insert,
    /// `FOR UPDATE`.
    Update,
    /// `FOR DELETE`.
    Delete,
}

impl PolicyCommand {
    /// SQL keyword used in CREATE POLICY rendering.
    #[must_use]
    pub const fn sql_keyword(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
        }
    }

    /// `pg_policies.cmd` text value. PG emits one of these strings.
    #[must_use]
    pub fn from_pg_text(s: &str) -> Option<Self> {
        match s {
            "ALL" => Some(Self::All),
            "SELECT" => Some(Self::Select),
            "INSERT" => Some(Self::Insert),
            "UPDATE" => Some(Self::Update),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }

    /// Whether `WITH CHECK` is valid for this command. PG rejects WITH CHECK
    /// on FOR SELECT and FOR DELETE policies.
    #[must_use]
    pub const fn allows_with_check(self) -> bool {
        matches!(self, Self::All | Self::Insert | Self::Update)
    }

    /// Whether `USING` is valid for this command. PG rejects USING on FOR
    /// INSERT policies (`only WITH CHECK expression allowed for INSERT`).
    #[must_use]
    pub const fn allows_using(self) -> bool {
        matches!(self, Self::All | Self::Select | Self::Update | Self::Delete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn pg_text_roundtrips() {
        for cmd in [
            PolicyCommand::All,
            PolicyCommand::Select,
            PolicyCommand::Insert,
            PolicyCommand::Update,
            PolicyCommand::Delete,
        ] {
            assert_eq!(PolicyCommand::from_pg_text(cmd.sql_keyword()), Some(cmd));
        }
    }

    #[test]
    fn from_pg_text_rejects_unknown() {
        assert_eq!(PolicyCommand::from_pg_text("BOGUS"), None);
    }

    #[test]
    fn select_and_delete_reject_with_check() {
        assert!(!PolicyCommand::Select.allows_with_check());
        assert!(!PolicyCommand::Delete.allows_with_check());
        assert!(PolicyCommand::All.allows_with_check());
        assert!(PolicyCommand::Insert.allows_with_check());
        assert!(PolicyCommand::Update.allows_with_check());
    }

    #[test]
    fn insert_rejects_using() {
        assert!(!PolicyCommand::Insert.allows_using());
        assert!(PolicyCommand::Select.allows_using());
        assert!(PolicyCommand::Delete.allows_using());
        assert!(PolicyCommand::Update.allows_using());
        assert!(PolicyCommand::All.allows_using());
    }

    #[test]
    fn policy_sort_by_name() {
        let a = Policy {
            name: id("alpha"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        };
        let b = Policy {
            name: id("beta"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        };
        let mut policies = vec![b.clone(), a.clone()];
        policies.sort();
        assert_eq!(policies, vec![a, b]);
    }
}
