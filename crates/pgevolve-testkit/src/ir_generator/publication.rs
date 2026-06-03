//! Publication generators (v0.3.4).
//!
//! Schema and table targets are drawn from the catalog's actual contents so
//! generated publications always reference real objects. Row filters and
//! column lists are left `None` — deeper variation is a v0.3.4.1 follow-up.

#![allow(clippy::needless_pass_by_value)]

use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::publication::{Publication, PublicationScope, PublishKinds, PublishedTable};

/// Small fixed pool of publication names (SQL-safe, short, distinct).
const PUB_NAMES: &[&str] = &["pub_a", "pub_b", "pub_c"];

/// Generate a random [`PublishKinds`] with at least one DML kind enabled.
fn arb_publish_kinds() -> impl Strategy<Value = PublishKinds> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>())
        .prop_filter("at least one DML kind", |(i, u, d, t)| *i || *u || *d || *t)
        .prop_map(|(insert, update, delete, truncate)| PublishKinds {
            insert,
            update,
            delete,
            truncate,
        })
}

/// Generate a random [`PublicationScope`] drawn from the provided pools.
///
/// The `schema_pool` and `table_pool` are the catalog's actual schemas and
/// tables so that generated publications always reference real objects.
///
/// # PG version compatibility
///
/// `FOR TABLES IN SCHEMA` (i.e. non-empty `schemas` in
/// `PublicationScope::Selective`) was added in PG 15. The testkit must run
/// against all supported versions including PG 14, so the `schemas` field is
/// always kept empty here. Coverage of `FOR TABLES IN SCHEMA` lives in the
/// conformance suite, where each fixture pins its own `min_pg_version`. See
/// <https://github.com/saosebastiao/pgevolve/issues/18>.
fn arb_publication_scope(
    _schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> BoxedStrategy<PublicationScope> {
    if table_pool.is_empty() {
        // Degenerate: no tables to reference — fall back to AllTables.
        return Just(PublicationScope::AllTables).boxed();
    }
    let tp = table_pool.clone();
    prop_oneof![
        Just(PublicationScope::AllTables),
        proptest::sample::subsequence(tp, 0..=(table_pool.len()))
            .prop_filter("non-empty Selective", |t| !t.is_empty())
            .prop_map(|tables| {
                let tables = tables
                    .into_iter()
                    .map(|qname| PublishedTable {
                        qname,
                        row_filter: None,
                        columns: None,
                    })
                    .collect();
                // schemas is always empty: FOR TABLES IN SCHEMA is PG 15+ only.
                // PG 14 compat — see issue #18.
                PublicationScope::Selective {
                    schemas: std::collections::BTreeSet::new(),
                    tables,
                }
            })
    ]
    .boxed()
}

/// Generate a single [`Publication`] with a name drawn from `pub_name_idx`
/// into `PUB_NAMES`, a random scope drawn from the provided pools, and
/// random publish/via-root flags.
fn arb_publication_inner(
    pub_name_idx: usize,
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> impl Strategy<Value = Publication> {
    let name = Identifier::from_unquoted(PUB_NAMES[pub_name_idx]).unwrap();
    (
        arb_publication_scope(schema_pool, table_pool),
        arb_publish_kinds(),
        any::<bool>(), // publish_via_partition_root
    )
        .prop_map(
            move |(scope, publish, publish_via_partition_root)| Publication {
                name: name.clone(),
                scope,
                publish,
                publish_via_partition_root,
                owner: None,
                comment: None,
            },
        )
}

/// Generate 0–2 [`Publication`]s with distinct names drawn from `PUB_NAMES`.
///
/// Publication names are globally unique (not schema-qualified) in PG.
/// The strategy generates at most `PUB_NAMES.len()` publications and ensures
/// name uniqueness by picking distinct indices from the pool.
pub(super) fn arb_publications(
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> BoxedStrategy<Vec<Publication>> {
    let max = PUB_NAMES.len().min(2); // 0..=2 publications
    (0usize..=max)
        .prop_flat_map(move |count| {
            let sp = schema_pool.clone();
            let tp = table_pool.clone();
            proptest::sample::subsequence((0..PUB_NAMES.len()).collect::<Vec<_>>(), count..=count)
                .prop_flat_map(move |indices| {
                    let sp = sp.clone();
                    let tp = tp.clone();
                    let strategies: Vec<_> = indices
                        .into_iter()
                        .map(|idx| arb_publication_inner(idx, sp.clone(), tp.clone()))
                        .collect();
                    strategies
                })
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    use pgevolve_core::identifier::{Identifier, QualifiedName};
    use pgevolve_core::ir::publication::PublicationScope;

    use super::arb_publication_scope;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    /// `FOR TABLES IN SCHEMA` (non-empty `schemas` in `PublicationScope::Selective`)
    /// was added in PG 15. The testkit covers PG 14–18, so generated scopes must
    /// never contain schema-scoped entries. See <https://github.com/saosebastiao/pgevolve/issues/18>.
    #[test]
    fn publication_scope_never_contains_schema_scope() {
        let schema_pool = vec![id("app"), id("public")];
        let table_pool = vec![qn("app", "orders"), qn("app", "users")];
        let strategy = arb_publication_scope(schema_pool, table_pool);
        let mut runner = TestRunner::default();
        for _ in 0..256 {
            let tree = strategy
                .new_tree(&mut runner)
                .expect("strategy construction failed");
            let scope = tree.current();
            if let PublicationScope::Selective { schemas, .. } = &scope {
                assert!(
                    schemas.is_empty(),
                    "FOR TABLES IN SCHEMA is PG 15+ only; generated scope must not use schema scope, got schemas = {schemas:?}",
                );
            }
        }
    }
}
