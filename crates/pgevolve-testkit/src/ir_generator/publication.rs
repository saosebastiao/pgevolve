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
fn arb_publication_scope(
    schema_pool: Vec<Identifier>,
    table_pool: Vec<QualifiedName>,
) -> BoxedStrategy<PublicationScope> {
    if schema_pool.is_empty() && table_pool.is_empty() {
        // Degenerate: no objects to reference — fall back to AllTables.
        return Just(PublicationScope::AllTables).boxed();
    }
    let sp = schema_pool.clone();
    let tp = table_pool.clone();
    prop_oneof![
        Just(PublicationScope::AllTables),
        (
            proptest::sample::subsequence(sp, 0..=(schema_pool.len())),
            proptest::sample::subsequence(tp, 0..=(table_pool.len())),
        )
            .prop_filter("non-empty Selective", |(s, t)| !s.is_empty()
                || !t.is_empty())
            .prop_map(|(schemas, tables)| {
                let schemas = schemas.into_iter().collect();
                let tables = tables
                    .into_iter()
                    .map(|qname| PublishedTable {
                        qname,
                        row_filter: None,
                        columns: None,
                    })
                    .collect();
                PublicationScope::Selective { schemas, tables }
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
