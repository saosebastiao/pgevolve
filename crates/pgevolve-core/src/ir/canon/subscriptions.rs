//! Canon pass for subscriptions. Validates and sorts.
//!
//! Invariants enforced:
//! - `Subscription.publications` is non-empty (PG requires at least one).
//! - `Subscription.connection` is non-empty / non-whitespace.
//!
//! Sorts:
//! - `Subscription.publications` by identifier text; duplicates silently
//!   deduplicated (source-side order is not semantically meaningful).
//! - The subscriptions collection itself is sorted by `sort_and_dedupe`,
//!   not here.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;
use crate::ir::subscription::Subscription;

/// Validate and sort all subscriptions in `cat`.
///
/// Returns the first [`IrError`] encountered (empty connection string or
/// empty publication list).
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    for s in &mut cat.subscriptions {
        validate_and_sort(s)?;
    }
    Ok(())
}

fn validate_and_sort(s: &mut Subscription) -> Result<(), IrError> {
    if s.connection.trim().is_empty() {
        return Err(IrError::EmptyConnection(s.name.clone()));
    }
    if s.publications.is_empty() {
        return Err(IrError::EmptySubscriptionPublications(s.name.clone()));
    }
    s.publications.sort();
    s.publications.dedup();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::subscription::SubscriptionOptions;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn sub_with(connection: &str, publications: Vec<Identifier>) -> Subscription {
        Subscription {
            name: id("s"),
            connection: connection.into(),
            publications,
            options: SubscriptionOptions::default(),
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn rejects_empty_connection() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with("", vec![id("p")]));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptyConnection(_)
        ));
    }

    #[test]
    fn rejects_whitespace_only_connection() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with("   ", vec![id("p")]));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptyConnection(_)
        ));
    }

    #[test]
    fn rejects_empty_publications() {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(sub_with("host=x", vec![]));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::EmptySubscriptionPublications(_)
        ));
    }

    #[test]
    fn sorts_and_dedupes_publications() {
        let mut cat = Catalog::empty();
        cat.subscriptions
            .push(sub_with("host=x", vec![id("c"), id("a"), id("b"), id("a")]));
        run(&mut cat).unwrap();
        let pubs = &cat.subscriptions[0].publications;
        assert_eq!(pubs.len(), 3);
        assert_eq!(pubs[0].as_str(), "a");
        assert_eq!(pubs[1].as_str(), "b");
        assert_eq!(pubs[2].as_str(), "c");
    }

    #[test]
    fn passes_through_valid_subscription() {
        let mut cat = Catalog::empty();
        cat.subscriptions
            .push(sub_with("host=x dbname=app", vec![id("p")]));
        assert!(run(&mut cat).is_ok());
    }
}
