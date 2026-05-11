//! Dependency analyzer and three-phase planner core.
//!
//! Turns an unordered [`ChangeSet`](crate::diff::ChangeSet) into an
//! [`OrderedChangeSet`] suitable for the rewrite pass and step grouping.
//!
//! See spec §6.4 for the design.

pub mod deserialize;
pub mod edges;
pub mod error;
pub mod graph;
pub mod grouping;
pub mod io_error;
pub mod ordered;
pub mod ordering;
pub mod plan;
pub mod policy;
pub mod raw_step;
pub mod rewrite;
pub mod serialize;

pub use deserialize::{
    ParsedIntent, ParsedManifest, PartialPlan, read_intent_toml, read_manifest_toml, read_plan_dir,
    read_plan_sql,
};
pub use edges::{NodeId, build_create_graph, build_drop_graph};
pub use error::PlanError;
pub use graph::{Cycle, Graph};
pub use grouping::{TransactionGroup, group_steps};
pub use io_error::PlanIoError;
pub use ordered::{DeferredFkAdd, OrderedChangeSet};
pub use ordering::order;
pub use plan::{
    DestructiveIntent, InvalidPlanHash, Plan, PlanId, PlanMetadata, kind_name, parse_kind_name,
};
pub use policy::{OnlineRewrites, PlannerPolicy, Strategy};
pub use raw_step::{RawStep, StepKind, TransactionConstraint};
pub use rewrite::rewrite;
pub use serialize::{write_intent_toml, write_manifest_toml, write_plan_dir, write_plan_sql};
