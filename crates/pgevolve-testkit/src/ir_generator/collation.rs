//! Collation generators (v0.3.8).
//!
//! Locale strings are drawn from a small hand-curated safe list so generated
//! catalogs always apply cleanly on PG 14–18. To avoid PG-version gating in
//! the proptest soak, this generator emits only libc collations (deterministic
//! only). ICU + nondeterministic + builtin variants are exercised by the
//! conformance fixtures instead.
//!
//! The libc-only restriction also dodges the canon-pass rejection of the
//! invalid libc + nondeterministic combination (see
//! `crates/pgevolve-core/src/ir/canon/collations.rs`).

#![allow(clippy::needless_pass_by_value)]

use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::collation::{Collation, CollationProvider};

/// Safe libc locales available on every supported PG image.
const SAFE_LIBC_LOCALES: &[&str] = &["C", "POSIX"];

/// Small fixed pool of collation name suffixes per schema.
const COLLATION_NAMES: &[&str] = &["coll_a", "coll_b"];

/// Generate a single libc [`Collation`] in `schema_name` with a name drawn
/// from `COLLATION_NAMES[name_idx]`.
fn arb_collation(schema_name: Identifier, name_idx: usize) -> impl Strategy<Value = Collation> {
    let raw_name = COLLATION_NAMES[name_idx % COLLATION_NAMES.len()];
    let name = Identifier::from_unquoted(raw_name).unwrap();
    proptest::sample::select(SAFE_LIBC_LOCALES).prop_map(move |loc| Collation {
        qname: QualifiedName::new(schema_name.clone(), name.clone()),
        provider: CollationProvider::Libc,
        lc_collate: (*loc).to_string(),
        lc_ctype: (*loc).to_string(),
        // libc + nondeterministic is rejected by canon — keep it true.
        deterministic: true,
        version: None,
        owner: None,
        comment: None,
    })
}

/// Generate 0–2 [`Collation`]s per schema (flat `Vec` for the whole catalog).
///
/// Names are drawn from `COLLATION_NAMES` so within a single schema we never
/// emit a duplicate qname (canon would reject).
pub(super) fn arb_collations_for_schemas(schemas: &[Identifier]) -> BoxedStrategy<Vec<Collation>> {
    if schemas.is_empty() {
        return Just(Vec::new()).boxed();
    }
    let per_schema: Vec<BoxedStrategy<Vec<Collation>>> = schemas
        .iter()
        .cloned()
        .map(|schema| {
            let max = COLLATION_NAMES.len(); // 0..=2
            (0usize..=max)
                .prop_flat_map(move |count| {
                    let schema = schema.clone();
                    let strategies: Vec<_> = (0..count)
                        .map(|idx| arb_collation(schema.clone(), idx))
                        .collect();
                    strategies
                })
                .boxed()
        })
        .collect();

    per_schema
        .prop_map(|per| per.into_iter().flatten().collect())
        .boxed()
}
