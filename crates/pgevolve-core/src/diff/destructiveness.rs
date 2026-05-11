//! `Destructiveness` — risk classification attached to every diff change.
//!
//! Each `ChangeEntry`, `TableOpEntry`, and `SequenceOpEntry` carries one of these
//! tags so the planner and CLI can decide whether a plan needs explicit approval
//! or carries data-loss risk.

use serde::{Deserialize, Serialize};

/// Risk classification for a single diff change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum Destructiveness {
    /// No approval required; the change cannot lose data on its own.
    Safe,
    /// Requires explicit approval but is not a data-loss risk on success.
    RequiresApproval {
        /// Human-readable reason shown to the user / written to the plan manifest.
        reason: String,
    },
    /// Requires explicit approval *and* warns of data loss.
    RequiresApprovalAndDataLossWarning {
        /// Human-readable reason shown to the user / written to the plan manifest.
        reason: String,
    },
}

impl Destructiveness {
    /// True when the change is anything other than [`Self::Safe`].
    pub const fn requires_approval(&self) -> bool {
        !matches!(self, Self::Safe)
    }

    /// True when the change is flagged as a data-loss risk.
    pub const fn data_loss_risk(&self) -> bool {
        matches!(self, Self::RequiresApprovalAndDataLossWarning { .. })
    }

    /// Returns the attached human-readable reason, if any.
    pub const fn reason(&self) -> Option<&str> {
        match self {
            Self::Safe => None,
            Self::RequiresApproval { reason }
            | Self::RequiresApprovalAndDataLossWarning { reason } => Some(reason.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_does_not_require_approval() {
        let d = Destructiveness::Safe;
        assert!(!d.requires_approval());
        assert!(!d.data_loss_risk());
        assert_eq!(d.reason(), None);
    }

    #[test]
    fn requires_approval_flags_approval_only() {
        let d = Destructiveness::RequiresApproval {
            reason: "drops index".into(),
        };
        assert!(d.requires_approval());
        assert!(!d.data_loss_risk());
        assert_eq!(d.reason(), Some("drops index"));
    }

    #[test]
    fn data_loss_warning_flags_both() {
        let d = Destructiveness::RequiresApprovalAndDataLossWarning {
            reason: "drops column email".into(),
        };
        assert!(d.requires_approval());
        assert!(d.data_loss_risk());
        assert_eq!(d.reason(), Some("drops column email"));
    }

    #[test]
    fn serde_round_trip_safe() {
        let d = Destructiveness::Safe;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, r#"{"level":"safe"}"#);
        let back: Destructiveness = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn serde_round_trip_requires_approval() {
        let d = Destructiveness::RequiresApproval { reason: "x".into() };
        let json = serde_json::to_string(&d).unwrap();
        let back: Destructiveness = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn serde_round_trip_data_loss() {
        let d = Destructiveness::RequiresApprovalAndDataLossWarning { reason: "x".into() };
        let json = serde_json::to_string(&d).unwrap();
        let back: Destructiveness = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
