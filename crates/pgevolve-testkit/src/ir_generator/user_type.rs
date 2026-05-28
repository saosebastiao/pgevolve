//! User-type generators (v0.3.8).
//!
//! Generates a small mix of `UserTypeKind` variants so the property tests
//! exercise the Range branch added in v0.3.8 alongside Enum / Domain /
//! Composite. The mix is intentionally biased: most generated types stay
//! Enum (a long-standing safe path), and only ~10 % become Range.
//!
//! All variants are produced with conservative shapes — subtype drawn from
//! a safe pool of built-in scalar types, no optional `canonical` /
//! `subtype_diff` functions, no opclass — so canon and the round-trip
//! property tests stay green without needing per-PG-version gating.

#![allow(clippy::needless_pass_by_value)]
// EnumValue sort_order is f32; we cast small `usize` indices (0..=3) — well
// within f32's exact-representation range. The pedantic lint can't see that
// bound.
#![allow(clippy::cast_precision_loss)]

use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::user_type::{CompositeAttribute, EnumValue, UserType, UserTypeKind};

/// Safe subtypes for `CREATE TYPE … AS RANGE (subtype = …)`.
///
/// All five are built-in scalar types available on every supported PG image.
/// Kept as `(schema, name)` pairs so the strategy can build a `QualifiedName`
/// without further allocation.
const SAFE_RANGE_SUBTYPES: &[(&str, &str)] = &[
    ("pg_catalog", "int4"),
    ("pg_catalog", "int8"),
    ("pg_catalog", "numeric"),
    ("pg_catalog", "text"),
    ("pg_catalog", "timestamptz"),
];

/// Small fixed pool of user-type name suffixes per schema. Each generated
/// schema emits 0–2 types drawn from this pool, ensuring intra-schema name
/// uniqueness.
const USER_TYPE_NAMES: &[&str] = &["ty_a", "ty_b"];

/// Generate an `Enum` kind with 1–3 labels.
fn arb_enum_kind() -> impl Strategy<Value = UserTypeKind> {
    (1usize..=3usize).prop_map(|n| UserTypeKind::Enum {
        values: (0..n)
            .map(|i| EnumValue {
                name: format!("v{i}"),
                sort_order: (i as f32) + 1.0,
            })
            .collect(),
    })
}

/// Generate a `Composite` kind with 1–2 attributes.
fn arb_composite_kind() -> impl Strategy<Value = UserTypeKind> {
    (1usize..=2usize).prop_map(|n| UserTypeKind::Composite {
        attributes: (0..n)
            .map(|i| CompositeAttribute {
                name: Identifier::from_unquoted(&format!("a{i}")).unwrap(),
                ty: ColumnType::Text,
                collation: None,
            })
            .collect(),
    })
}

/// Generate a `Range` kind. Subtype drawn from `SAFE_RANGE_SUBTYPES`; all
/// other fields default to `None` for v0.3.8.
fn arb_range_kind() -> impl Strategy<Value = UserTypeKind> {
    proptest::sample::select(SAFE_RANGE_SUBTYPES).prop_map(|(schema, name)| UserTypeKind::Range {
        subtype: QualifiedName::new(
            Identifier::from_unquoted(schema).unwrap(),
            Identifier::from_unquoted(name).unwrap(),
        ),
        subtype_opclass: None,
        collation: None,
        canonical: None,
        subtype_diff: None,
        multirange_type_name: None,
    })
}

/// Generate any `UserTypeKind` we currently exercise in proptests.
///
/// Domain is intentionally omitted from the rotation — it relies on a
/// `ColumnType` base that interacts with canon's `resolve_user_defined_types`
/// pass and would require additional plumbing to keep the round-trip stable.
/// Enum / Composite / Range cover the v0.3.8 surface change adequately.
///
/// Weights: ~50 % Enum, ~40 % Composite, ~10 % Range. This is the "~10 % of
/// generated `UserType`s should be `Range`" target from Stage 10.2.
fn arb_user_type_kind() -> impl Strategy<Value = UserTypeKind> {
    prop_oneof![
        5 => arb_enum_kind().boxed(),
        4 => arb_composite_kind().boxed(),
        1 => arb_range_kind().boxed(),
    ]
}

/// Generate a single [`UserType`] in `schema` with a name drawn from
/// `USER_TYPE_NAMES[name_idx]`.
fn arb_user_type(schema: Identifier, name_idx: usize) -> impl Strategy<Value = UserType> {
    let raw_name = USER_TYPE_NAMES[name_idx % USER_TYPE_NAMES.len()];
    let name = Identifier::from_unquoted(raw_name).unwrap();
    arb_user_type_kind().prop_map(move |kind| UserType {
        qname: QualifiedName::new(schema.clone(), name.clone()),
        kind,
        comment: None,
        owner: None,
        grants: vec![],
    })
}

/// Generate 0–2 user types per schema, flattened into a single `Vec` for the
/// whole catalog.
///
/// Distinct names within a schema are guaranteed because `USER_TYPE_NAMES`
/// has exactly two entries and we never request more than `len()` per
/// schema.
pub(super) fn arb_user_types_for_schemas(schemas: &[Identifier]) -> BoxedStrategy<Vec<UserType>> {
    if schemas.is_empty() {
        return Just(Vec::new()).boxed();
    }
    let per_schema: Vec<BoxedStrategy<Vec<UserType>>> = schemas
        .iter()
        .cloned()
        .map(|schema| {
            let max = USER_TYPE_NAMES.len(); // 0..=2
            (0usize..=max)
                .prop_flat_map(move |count| {
                    let schema = schema.clone();
                    let strategies: Vec<_> = (0..count)
                        .map(|idx| arb_user_type(schema.clone(), idx))
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
