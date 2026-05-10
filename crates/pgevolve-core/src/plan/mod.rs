//! Dependency analyzer and three-phase planner core.
//!
//! Turns an unordered [`ChangeSet`](crate::diff::ChangeSet) into an
//! [`OrderedChangeSet`] suitable for the rewrite pass and step grouping.
//!
//! See spec §6.4 for the design.

pub mod edges;
pub mod error;
pub mod graph;
pub mod grouping;
pub mod ordered;
pub mod ordering;
pub mod plan;
pub mod policy;
pub mod raw_step;
pub mod rewrite;

pub use edges::{build_create_graph, build_drop_graph, NodeId};
pub use error::PlanError;
pub use graph::{Cycle, Graph};
pub use ordered::{DeferredFkAdd, OrderedChangeSet};
pub use ordering::order;
pub use policy::{OnlineRewrites, PlannerPolicy, Strategy};
pub use raw_step::{RawStep, StepKind, TransactionConstraint};
pub use grouping::{group_steps, TransactionGroup};
pub use plan::{DestructiveIntent, InvalidPlanHash, Plan, PlanId, PlanMetadata};
pub use rewrite::rewrite;
