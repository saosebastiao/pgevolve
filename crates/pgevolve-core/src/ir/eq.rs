//! `Equiv` trait — produces structured differences between two IR values.

use super::difference::Difference;

/// Compute the structured difference between two IR values.
///
/// Equivalence is the inverse of `differences(...).is_empty()`. Implementors
/// derive equivalence from `Equiv` rather than from `PartialEq` so that
/// equivalence rules can diverge from structural equality (e.g., field
/// reordering inside a `Vec<Constraint>` doesn't matter, but `PartialEq` would
/// say it does).
pub trait Equiv {
    /// List the differences between `self` and `other`. Empty list = equivalent.
    fn differences(&self, other: &Self) -> Vec<Difference>;

    /// Convenience: `true` iff `self.differences(other).is_empty()`.
    fn canonical_eq(&self, other: &Self) -> bool {
        self.differences(other).is_empty()
    }
}

/// Helper: produces a single-element `Vec<Difference>` if `from != to`, else empty.
pub fn field_difference<T: PartialEq + std::fmt::Display>(
    path: &str,
    from: &T,
    to: &T,
) -> Vec<Difference> {
    if from == to {
        Vec::new()
    } else {
        vec![Difference::new(path, from, to)]
    }
}

/// Helper: prefix every element's path.
#[must_use]
pub fn prefix_differences(prefix: &str, diffs: Vec<Difference>) -> Vec<Difference> {
    diffs.into_iter().map(|d| d.prefix_path(prefix)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_difference_matches() {
        let r = field_difference("name", &1, &1);
        assert!(r.is_empty());
    }

    #[test]
    fn field_difference_reports() {
        let r = field_difference("name", &1, &2);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].path, "name");
    }

    #[test]
    fn prefix_differences_simple() {
        let d = vec![Difference::new("len", "5", "10")];
        let p = prefix_differences("ty", d);
        assert_eq!(p[0].path, "ty.len");
    }

    #[test]
    fn prefix_differences_empty_path() {
        let d = vec![Difference::new("", "a", "b")];
        let p = prefix_differences("ty", d);
        assert_eq!(p[0].path, "ty");
    }
}
