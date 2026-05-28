//! Owner + grants helpers (v0.3.1).
//!
//! Strategies for generating optional owners and grant lists across all the
//! grantable IR object types, plus top-level default-privilege rules.

// Proptest closures and `prop_map`/`prop_flat_map` chains in this module
// inherently clone moved captures; the pedantic lints fight straight-line
// strategy code.
#![allow(clippy::needless_pass_by_value)]
// Single-char binding names in slice-pattern arms are intentional for
// conciseness in `arb_grant_from`'s privilege-slice dispatch.
#![allow(clippy::many_single_char_names)]

use proptest::prelude::*;

use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
use pgevolve_core::ir::grant::{Grant, GrantTarget, Privilege};

/// Small fixed pool of role names used for generating owners and grantees.
/// Overlaps with `ROLE_NAMES` so cluster-link lint tests can cross-reference.
pub(super) const GRANTEE_ROLE_NAMES: &[&str] =
    &["app_owner", "readers", "writers", "app", "ops", "auditor"];

/// Generate an optional owner — `None` (unmanaged) or `Some(role)` from a
/// small pool.
pub(super) fn arb_owner() -> impl Strategy<Value = Option<Identifier>> {
    prop_oneof![
        Just(None),
        prop_oneof![
            Just("app_owner"),
            Just("readers"),
            Just("writers"),
            Just("app"),
            Just("ops"),
        ]
        .prop_map(|s| Some(Identifier::from_unquoted(s).unwrap())),
    ]
}

/// Generate a single [`Grant`] for an object-level privilege.
///
/// `privileges` must be the slice of privileges valid for the target object
/// kind. `with_columns` controls whether column restrictions may appear.
fn arb_grant_from(privileges: &'static [Privilege], with_columns: bool) -> BoxedStrategy<Grant> {
    let grantee_strategy = prop_oneof![
        Just(GrantTarget::Public),
        prop_oneof![
            Just(GRANTEE_ROLE_NAMES[0]),
            Just(GRANTEE_ROLE_NAMES[1]),
            Just(GRANTEE_ROLE_NAMES[2]),
            Just(GRANTEE_ROLE_NAMES[3]),
            Just(GRANTEE_ROLE_NAMES[4]),
            Just(GRANTEE_ROLE_NAMES[5]),
        ]
        .prop_map(|s| GrantTarget::Role(Identifier::from_unquoted(s).unwrap())),
    ];

    let priv_strategy: BoxedStrategy<Privilege> = {
        // Build a prop_oneof from the slice by cycling through it.
        // `privileges` is &'static so we can capture it safely.
        match privileges {
            [a] => Just(*a).boxed(),
            [a, b] => prop_oneof![Just(*a), Just(*b)].boxed(),
            [a, b, c] => prop_oneof![Just(*a), Just(*b), Just(*c)].boxed(),
            [a, b, c, d] => prop_oneof![Just(*a), Just(*b), Just(*c), Just(*d)].boxed(),
            [a, b, c, d, e] => {
                prop_oneof![Just(*a), Just(*b), Just(*c), Just(*d), Just(*e)].boxed()
            }
            [a, b, c, d, e, f] => {
                prop_oneof![Just(*a), Just(*b), Just(*c), Just(*d), Just(*e), Just(*f)].boxed()
            }
            [a, b, c, d, e, f, g] => prop_oneof![
                Just(*a),
                Just(*b),
                Just(*c),
                Just(*d),
                Just(*e),
                Just(*f),
                Just(*g),
            ]
            .boxed(),
            _ => Just(privileges[0]).boxed(),
        }
    };

    // Optional column restriction (only meaningful for table/view/mv).
    let col_strategy: BoxedStrategy<Option<Vec<Identifier>>> = if with_columns {
        prop_oneof![
            Just(None),
            prop_oneof![
                Just(vec!["id"]),
                Just(vec!["name"]),
                Just(vec!["email"]),
                Just(vec!["id", "name"]),
            ]
            .prop_map(|cols| {
                Some(
                    cols.into_iter()
                        .map(|c| Identifier::from_unquoted(c).unwrap())
                        .collect(),
                )
            }),
        ]
        .boxed()
    } else {
        Just(None).boxed()
    };

    (grantee_strategy, priv_strategy, any::<bool>(), col_strategy)
        .prop_map(|(grantee, privilege, with_grant_option, columns)| {
            // PG rejects column-level grants for privileges that aren't
            // column-eligible: only SELECT/INSERT/UPDATE/REFERENCES may
            // appear with a column subset. Drop the columns subset when
            // the rolled privilege isn't one of those, instead of
            // re-rolling the strategy.
            let columns = match (columns, privilege) {
                (
                    Some(cols),
                    Privilege::Select
                    | Privilege::Insert
                    | Privilege::Update
                    | Privilege::References,
                ) => Some(cols),
                _ => None,
            };
            Grant {
                grantee,
                privilege,
                with_grant_option,
                columns,
            }
        })
        .boxed()
}

/// Generate a `Vec<Grant>` of 0–3 grants for a non-column-level object.
pub(super) fn arb_object_grants(
    privileges: &'static [Privilege],
) -> impl Strategy<Value = Vec<Grant>> {
    prop_oneof![
        Just(vec![]),
        arb_grant_from(privileges, false).prop_map(|g| vec![g]),
        (
            arb_grant_from(privileges, false),
            arb_grant_from(privileges, false)
        )
            .prop_map(|(a, b)| vec![a, b]),
        (
            arb_grant_from(privileges, false),
            arb_grant_from(privileges, false),
            arb_grant_from(privileges, false)
        )
            .prop_map(|(a, b, c)| vec![a, b, c]),
    ]
}

/// Generate a `Vec<Grant>` of 0–3 grants for table/view/mv (may include
/// column restrictions).
pub(super) fn arb_table_grants(
    privileges: &'static [Privilege],
) -> impl Strategy<Value = Vec<Grant>> {
    prop_oneof![
        Just(vec![]),
        arb_grant_from(privileges, true).prop_map(|g| vec![g]),
        (
            arb_grant_from(privileges, true),
            arb_grant_from(privileges, true)
        )
            .prop_map(|(a, b)| vec![a, b]),
        (
            arb_grant_from(privileges, true),
            arb_grant_from(privileges, true),
            arb_grant_from(privileges, true)
        )
            .prop_map(|(a, b, c)| vec![a, b, c]),
    ]
}

/// Table privileges (SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER).
pub(super) const TABLE_PRIVS: &[Privilege] = &[
    Privilege::Select,
    Privilege::Insert,
    Privilege::Update,
    Privilege::Delete,
    Privilege::Truncate,
    Privilege::References,
    Privilege::Trigger,
];

/// Schema privileges (USAGE, CREATE).
pub(super) const SCHEMA_PRIVS: &[Privilege] = &[Privilege::Usage, Privilege::Create];

/// Sequence privileges (USAGE, SELECT, UPDATE).
pub(super) const SEQUENCE_PRIVS: &[Privilege] =
    &[Privilege::Usage, Privilege::Select, Privilege::Update];

/// Function/procedure privileges (EXECUTE only).
/// Prepared for future function/procedure generators; unused until those ship.
#[allow(dead_code)]
pub(super) const FUNCTION_PRIVS: &[Privilege] = &[Privilege::Execute];

/// Type privileges (USAGE only).
/// Prepared for future user-type generators; unused until those ship.
#[allow(dead_code)]
pub(super) const TYPE_PRIVS: &[Privilege] = &[Privilege::Usage];

/// Generate 0–3 [`DefaultPrivilegeRule`]s for arbitrary catalog-level rules.
pub fn arbitrary_default_privileges() -> impl Strategy<Value = Vec<DefaultPrivilegeRule>> {
    let rule_strategy = (
        // target_role
        prop_oneof![Just("app_owner"), Just("app"), Just("ops"),]
            .prop_map(|s| Identifier::from_unquoted(s).unwrap()),
        // schema (None = all schemas)
        prop_oneof![Just(None), Just(Some("app")), Just(Some("billing")),]
            .prop_map(|s| s.map(|n| Identifier::from_unquoted(n).unwrap())),
        // object_type
        prop_oneof![
            Just(DefaultPrivObjectType::Tables),
            Just(DefaultPrivObjectType::Sequences),
            Just(DefaultPrivObjectType::Functions),
            Just(DefaultPrivObjectType::Types),
            Just(DefaultPrivObjectType::Schemas),
        ],
        // grants within the rule (0–2)
        prop_oneof![
            Just(vec![]),
            prop_oneof![
                Just(GrantTarget::Public),
                Just(GrantTarget::Role(
                    Identifier::from_unquoted("readers").unwrap()
                )),
                Just(GrantTarget::Role(
                    Identifier::from_unquoted("writers").unwrap()
                )),
            ]
            .prop_map(|grantee| vec![Grant {
                grantee,
                privilege: Privilege::Select,
                with_grant_option: false,
                columns: None,
            }]),
        ],
    )
        .prop_map(
            |(target_role, schema, object_type, grants)| DefaultPrivilegeRule {
                target_role,
                schema,
                object_type,
                grants,
            },
        );

    prop_oneof![
        Just(vec![]),
        rule_strategy.clone().prop_map(|r| vec![r]),
        (rule_strategy.clone(), rule_strategy.clone()).prop_map(|(a, b)| vec![a, b]),
        (rule_strategy.clone(), rule_strategy.clone(), rule_strategy)
            .prop_map(|(a, b, c)| vec![a, b, c]),
    ]
}
