//! Smoke tests for `#[derive(Diff)]`.
//!
//! These tests construct dummy structs that mimic the shapes the derive
//! supports, derive `Diff`, and assert the emitted `Difference` records
//! exactly match a hand-written equivalent. They do not depend on
//! `pgevolve-core` to keep the macro crate's tests self-contained — a
//! minimal local copy of the `Diff` trait + `Difference` + `diff_field` /
//! `prefix_diffs` helpers lives in this file.

// This file is a self-contained test harness with intentionally public
// mirror types that don't need documentation, dead_code for the `ignored`
// field (used only to verify skip behaviour), and a builder-style method
// that doesn't warrant must_use.
#![allow(
    missing_docs,
    dead_code,
    clippy::return_self_not_must_use,
    clippy::missing_panics_doc
)]

use pgevolve_core_macros::Diff;

// --- Minimal local mirror of `pgevolve_core::ir::eq` ---
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub from: String,
    pub to: String,
}

impl Difference {
    pub fn new(path: impl Into<String>, from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            from: from.into(),
            to: to.into(),
        }
    }

    pub fn prefix_path(mut self, prefix: &str) -> Self {
        if self.path.is_empty() {
            self.path = prefix.to_string();
        } else {
            self.path = format!("{prefix}.{}", self.path);
        }
        self
    }
}

pub trait Diff {
    fn diff(&self, other: &Self) -> Vec<Difference>;
}

pub fn diff_field<T: PartialEq + std::fmt::Display>(
    path: &str,
    from: &T,
    to: &T,
) -> Vec<Difference> {
    if from == to {
        Vec::new()
    } else {
        vec![Difference::new(path, from.to_string(), to.to_string())]
    }
}

pub fn prefix_diffs(prefix: &str, diffs: Vec<Difference>) -> Vec<Difference> {
    diffs.into_iter().map(|d| d.prefix_path(prefix)).collect()
}

// The derive expands to paths like `crate::ir::eq::Diff`. Re-expose them
// at those paths so the generated code compiles in this test crate.
mod ir {
    pub mod eq {
        pub use super::super::{Diff, diff_field, prefix_diffs};
    }
    pub mod difference {
        pub use super::super::Difference;
    }
}

// --- Smoke structs ---

#[derive(Diff)]
struct OneField {
    name: String,
}

#[derive(Diff)]
struct ThreeStrategies {
    plain: String,
    #[diff(skip)]
    ignored: u32,
    #[diff(via_debug)]
    debug_field: Option<u32>,
    #[diff(nested)]
    nested_field: Inner,
}

#[derive(Debug, Clone)]
struct Inner {
    val: String,
}

impl Diff for Inner {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        diff_field("val", &self.val, &other.val)
    }
}

// --- Tests ---

#[test]
fn one_field_default_strategy_emits_diff_field() {
    let a = OneField { name: "a".into() };
    let b = OneField { name: "b".into() };
    let d = a.diff(&b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "name");
    assert_eq!(d[0].from, "a");
    assert_eq!(d[0].to, "b");
}

#[test]
fn skip_omits_the_field() {
    let a = ThreeStrategies {
        plain: "x".into(),
        ignored: 1,
        debug_field: None,
        nested_field: Inner { val: "v".into() },
    };
    let b = ThreeStrategies {
        plain: "x".into(),
        ignored: 999, // different but skipped
        debug_field: None,
        nested_field: Inner { val: "v".into() },
    };
    assert!(a.diff(&b).is_empty(), "skipped field must not appear");
}

#[test]
fn via_debug_uses_debug_format() {
    let a = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: Some(1),
        nested_field: Inner { val: "v".into() },
    };
    let b = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: Some(2),
        nested_field: Inner { val: "v".into() },
    };
    let d = a.diff(&b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "debug_field");
    assert_eq!(d[0].from, "Some(1)");
    assert_eq!(d[0].to, "Some(2)");
}

#[test]
fn nested_prefixes_field_paths() {
    let a = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: None,
        nested_field: Inner { val: "v1".into() },
    };
    let b = ThreeStrategies {
        plain: "x".into(),
        ignored: 0,
        debug_field: None,
        nested_field: Inner { val: "v2".into() },
    };
    let d = a.diff(&b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "nested_field.val");
    assert_eq!(d[0].from, "v1");
    assert_eq!(d[0].to, "v2");
}

#[test]
fn equal_values_produce_empty_diff() {
    let a = OneField {
        name: "same".into(),
    };
    let b = OneField {
        name: "same".into(),
    };
    assert!(a.diff(&b).is_empty());
}
