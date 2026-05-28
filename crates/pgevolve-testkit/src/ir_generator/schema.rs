//! Schema strategies — name pool plus owner/grant decoration.

#![allow(clippy::needless_pass_by_value)]

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;
use proptest::sample::SizeRange;

use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::schema::Schema;

use super::IRGeneratorConfig;
use super::grants::{SCHEMA_PRIVS, arb_object_grants, arb_owner};

pub(super) fn arbitrary_schemas(
    cfg: &IRGeneratorConfig,
) -> impl Strategy<Value = Vec<Schema>> + use<> {
    let count = SizeRange::from(cfg.schema_count_range.0..=cfg.schema_count_range.1);
    proptest_vec(
        (
            schema_name_strategy(),
            arb_owner(),
            arb_object_grants(SCHEMA_PRIVS),
        ),
        count,
    )
    .prop_map(|triples| {
        // Deduplicate while preserving order: HashSet would lose ordering.
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for (name, owner, grants) in triples {
            if seen.insert(name.clone()) {
                let mut schema = Schema::new(name);
                schema.owner = owner;
                schema.grants = grants;
                out.push(schema);
            }
        }
        out
    })
}

fn schema_name_strategy() -> impl Strategy<Value = Identifier> {
    prop_oneof![
        Just("app"),
        Just("billing"),
        Just("audit"),
        Just("inventory"),
        Just("auth"),
        Just("ops"),
    ]
    .prop_map(|s| Identifier::from_unquoted(s).unwrap())
}
