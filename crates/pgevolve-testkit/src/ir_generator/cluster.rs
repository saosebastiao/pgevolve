//! Cluster catalog generators (v0.3).
//!
//! Roles are generated in topological order (each role's `member_of`
//! references are drawn from earlier roles only) so the resulting
//! membership graph is acyclic.

#![allow(clippy::needless_pass_by_value)]

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;

use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::cluster::catalog::ClusterCatalog;
use pgevolve_core::ir::cluster::role::{Role, RoleAttributes};

/// Candidate role name pool — short, distinct, SQL-safe identifiers.
const ROLE_NAMES: &[&str] = &[
    "app", "ops", "reader", "writer", "admin", "auditor", "analyst", "deploy",
];

/// Generate a [`RoleAttributes`] with uniformly-random boolean flags and an
/// optional `connection_limit` / `valid_until`.
pub fn arbitrary_role_attributes() -> impl Strategy<Value = RoleAttributes> {
    (
        any::<bool>(),                                               // superuser
        any::<bool>(),                                               // createdb
        any::<bool>(),                                               // createrole
        any::<bool>(),                                               // inherit
        any::<bool>(),                                               // login
        any::<bool>(),                                               // replication
        any::<bool>(),                                               // bypass_rls
        prop_oneof![Just(None), (1i64..=10_000i64).prop_map(Some),], // connection_limit
        prop_oneof![
            Just(None),
            Just(Some("2030-01-01T00:00:00Z".to_string())),
            Just(Some("2035-06-15T12:00:00Z".to_string())),
        ], // valid_until
    )
        .prop_map(
            |(
                superuser,
                createdb,
                createrole,
                inherit,
                login,
                replication,
                bypass_rls,
                connection_limit,
                valid_until,
            )| {
                RoleAttributes {
                    superuser,
                    createdb,
                    createrole,
                    inherit,
                    login,
                    replication,
                    bypass_rls,
                    connection_limit,
                    valid_until,
                }
            },
        )
}

/// Generate a single [`Role`] with a name drawn from `role_name_idx` into
/// `ROLE_NAMES`, random attributes, and `member_of` edges drawn exclusively
/// from `peer_name_indices` (indices into `ROLE_NAMES`).
///
/// The `peer_name_indices` slice contains the indices of roles that were
/// generated *before* this one in topological order — passing only earlier
/// roles' indices guarantees the resulting membership graph is acyclic.
fn arbitrary_role_inner(
    name_idx: usize,
    peer_name_indices: Vec<usize>,
) -> impl Strategy<Value = Role> {
    let name = Identifier::from_unquoted(ROLE_NAMES[name_idx]).unwrap();

    (
        arbitrary_role_attributes(),
        // A bitmask that selects a subset of `peer_name_indices` to add as
        // `member_of` edges.  Using a u16 caps at 16 peers which is well
        // above the pool size (8 roles max).
        any::<u16>(),
        prop_oneof![
            Just(None),
            ".*".prop_map(|s| if s.is_empty() { None } else { Some(s) }),
        ],
    )
        .prop_map(move |(attributes, peer_mask, comment)| {
            let member_of: Vec<Identifier> = peer_name_indices
                .iter()
                .enumerate()
                .filter_map(|(bit, &peer_idx)| {
                    if bit < 16 && (peer_mask >> bit) & 1 == 1 {
                        Some(Identifier::from_unquoted(ROLE_NAMES[peer_idx]).unwrap())
                    } else {
                        None
                    }
                })
                .collect();
            Role {
                name: name.clone(),
                attributes,
                member_of,
                comment,
            }
        })
}

/// Generate a [`ClusterCatalog`] with 0–`ROLE_NAMES.len()` roles.
///
/// Roles are generated in topological order (each role's `member_of`
/// references are drawn from roles earlier in the list only) to guarantee
/// the membership graph is acyclic.  The catalog is canonicalized before
/// being returned.
pub fn arbitrary_cluster_catalog() -> impl Strategy<Value = ClusterCatalog> {
    // First pick how many roles (0..=len) and which distinct names to use.
    let max = ROLE_NAMES.len();
    (0usize..=max)
        .prop_flat_map(move |count| {
            // Sample `count` distinct indices from 0..max.
            proptest_vec(0usize..max, count..=count).prop_map(move |mut indices| {
                // De-duplicate while preserving order.
                let mut seen = std::collections::BTreeSet::new();
                indices.retain(|i| seen.insert(*i));
                // Trim to `count` after dedup (may be shorter).
                indices
            })
        })
        .prop_flat_map(|indices| {
            // For each role at position `i`, its peers are `indices[0..i]`.
            let strategies: Vec<_> = indices
                .iter()
                .enumerate()
                .map(|(i, &name_idx)| {
                    let peers: Vec<usize> = indices[..i].to_vec();
                    arbitrary_role_inner(name_idx, peers)
                })
                .collect();
            strategies.prop_map(|roles| {
                let mut cat = ClusterCatalog { roles };
                cat.canonicalize();
                cat
            })
        })
}
