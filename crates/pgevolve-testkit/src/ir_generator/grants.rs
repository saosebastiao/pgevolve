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
            // PG rejects `GRANT ... TO PUBLIC WITH GRANT OPTION` (error 0LP01).
            // Force the flag off for PUBLIC grantees.
            let with_grant_option = with_grant_option && grantee != GrantTarget::Public;
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
///
/// `schema_pool` is the set of schemas declared in the target catalog. The
/// `IN SCHEMA y` field of generated rules is restricted to `None` (meaning
/// "all schemas owned by the target role") or a schema actually present in
/// `schema_pool`, so the DDL never references a schema that does not exist.
/// Pass an empty slice to always generate `None` (schema-agnostic rules only).
pub fn arbitrary_default_privileges(
    schema_pool: &[Identifier],
) -> BoxedStrategy<Vec<DefaultPrivilegeRule>> {
    // Build a per-rule schema strategy: None or one declared schema.
    let schema_strategy: BoxedStrategy<Option<Identifier>> = if schema_pool.is_empty() {
        Just(None).boxed()
    } else {
        let pool = schema_pool.to_vec();
        prop_oneof![Just(None), proptest::sample::select(pool).prop_map(Some),].boxed()
    };

    // Build the rule strategy by pairing an independent schema pick with the
    // other fields so that each generated rule carries its own schema choice.
    //
    // Grants within each rule must use a privilege valid for the rule's
    // object_type: PG rejects mismatches with error 0LP01. We pick the
    // object_type first, then derive the privilege set from it via
    // `prop_flat_map`.
    let grantee_strategy = || {
        prop_oneof![
            Just(GrantTarget::Public),
            Just(GrantTarget::Role(
                Identifier::from_unquoted("readers").unwrap()
            )),
            Just(GrantTarget::Role(
                Identifier::from_unquoted("writers").unwrap()
            )),
        ]
    };

    let rule_strategy = (
        // target_role
        prop_oneof![Just("app_owner"), Just("app"), Just("ops"),]
            .prop_map(|s| Identifier::from_unquoted(s).unwrap()),
        // schema: None or a schema from the catalog's declared set.
        schema_strategy,
        // object_type — picked first so the grant strategy can depend on it.
        prop_oneof![
            Just(DefaultPrivObjectType::Tables),
            Just(DefaultPrivObjectType::Sequences),
            Just(DefaultPrivObjectType::Functions),
            Just(DefaultPrivObjectType::Types),
            Just(DefaultPrivObjectType::Schemas),
        ],
    )
        .prop_flat_map(move |(target_role, schema, object_type)| {
            // Per-object-type valid privilege sets (PG 14-17).
            // MAINTAIN (PG 17+) omitted to avoid version gating.
            let privilege_strategy: BoxedStrategy<Privilege> = match object_type {
                DefaultPrivObjectType::Tables => prop_oneof![
                    Just(Privilege::Select),
                    Just(Privilege::Insert),
                    Just(Privilege::Update),
                    Just(Privilege::Delete),
                    Just(Privilege::Truncate),
                    Just(Privilege::References),
                    Just(Privilege::Trigger),
                ]
                .boxed(),
                DefaultPrivObjectType::Sequences => prop_oneof![
                    Just(Privilege::Usage),
                    Just(Privilege::Select),
                    Just(Privilege::Update),
                ]
                .boxed(),
                DefaultPrivObjectType::Functions => Just(Privilege::Execute).boxed(),
                DefaultPrivObjectType::Types => Just(Privilege::Usage).boxed(),
                DefaultPrivObjectType::Schemas => {
                    prop_oneof![Just(Privilege::Usage), Just(Privilege::Create),].boxed()
                }
            };

            // grants within the rule (0–1): empty or one grant with a
            // privilege valid for this object_type.
            let grants_strategy: BoxedStrategy<Vec<Grant>> = prop_oneof![
                Just(vec![]),
                (grantee_strategy(), privilege_strategy).prop_map(|(grantee, privilege)| {
                    vec![Grant {
                        grantee,
                        privilege,
                        with_grant_option: false,
                        columns: None,
                    }]
                }),
            ]
            .boxed();

            grants_strategy.prop_map(move |grants| DefaultPrivilegeRule {
                target_role: target_role.clone(),
                schema: schema.clone(),
                object_type,
                grants,
            })
        });

    prop_oneof![
        Just(vec![]),
        rule_strategy.clone().prop_map(|r| vec![r]),
        (rule_strategy.clone(), rule_strategy.clone()).prop_map(|(a, b)| vec![a, b]),
        (rule_strategy.clone(), rule_strategy.clone(), rule_strategy)
            .prop_map(|(a, b, c)| vec![a, b, c]),
    ]
    .boxed()
}

/// Returns `true` iff `privilege` is a valid privilege for the given
/// `DefaultPrivObjectType` per PG 14-17.
///
/// Used in property tests to assert the generator never emits an invalid
/// `(object_type, privilege)` pair.
#[cfg(test)]
const fn is_valid_default_priv(
    object_type: pgevolve_core::ir::default_privileges::DefaultPrivObjectType,
    privilege: pgevolve_core::ir::grant::Privilege,
) -> bool {
    use pgevolve_core::ir::default_privileges::DefaultPrivObjectType as T;
    use pgevolve_core::ir::grant::Privilege as P;
    match object_type {
        T::Tables => matches!(
            privilege,
            P::Select
                | P::Insert
                | P::Update
                | P::Delete
                | P::Truncate
                | P::References
                | P::Trigger
        ),
        T::Sequences => matches!(privilege, P::Usage | P::Select | P::Update),
        T::Functions => matches!(privilege, P::Execute),
        T::Types => matches!(privilege, P::Usage),
        T::Schemas => matches!(privilege, P::Usage | P::Create),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest::test_runner::Config;

    use pgevolve_core::ir::grant::GrantTarget;

    use super::{
        TABLE_PRIVS, arb_object_grants, arbitrary_default_privileges, is_valid_default_priv,
    };

    proptest! {
        #![proptest_config(Config { cases: 256, ..Config::default() })]

        /// PG rejects `GRANT ... TO PUBLIC WITH GRANT OPTION` (error 0LP01).
        /// Assert that the generator never produces that combination.
        #[test]
        fn public_grant_never_has_with_grant_option(grants in arb_object_grants(TABLE_PRIVS)) {
            for grant in &grants {
                if grant.grantee == GrantTarget::Public {
                    prop_assert!(
                        !grant.with_grant_option,
                        "with_grant_option must be false for PUBLIC grantee (PG error 0LP01)"
                    );
                }
            }
        }

        /// PG rejects privilege/object-kind mismatches in ALTER DEFAULT PRIVILEGES
        /// (e.g. SELECT on FUNCTIONS is invalid — only EXECUTE is accepted).
        /// Assert that every generated rule uses a privilege valid for its object type.
        #[test]
        fn default_priv_rules_use_valid_privilege_for_object_type(
            rules in arbitrary_default_privileges(&[])
        ) {
            for rule in &rules {
                for grant in &rule.grants {
                    prop_assert!(
                        is_valid_default_priv(rule.object_type, grant.privilege),
                        "invalid privilege {:?} for object type {:?} (PG error 0LP01)",
                        grant.privilege,
                        rule.object_type,
                    );
                }
            }
        }
    }
}
