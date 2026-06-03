//! RLS policy generators (v0.3.2).
//!
//! Respects PG's `WITH CHECK` vs `FOR SELECT`/`FOR DELETE` incompatibility —
//! generated policies never produce invalid PG syntax.

#![allow(clippy::needless_pass_by_value)]

use proptest::collection::vec as proptest_vec;
use proptest::prelude::*;

use pgevolve_core::identifier::Identifier;
use pgevolve_core::ir::default_expr::NormalizedExpr;
use pgevolve_core::ir::grant::GrantTarget;
use pgevolve_core::ir::policy::{Policy, PolicyCommand};

use super::grants::GRANTEE_ROLE_NAMES;

/// Small fixed pool of policy names (SQL-safe, short, distinct).
const POLICY_NAMES: &[&str] = &[
    "allow_select",
    "allow_insert",
    "tenant_isolation",
    "owner_policy",
    "admin_bypass",
    "audit_row",
];

/// Generate a random [`PolicyCommand`].
fn arb_policy_command() -> impl Strategy<Value = PolicyCommand> {
    prop_oneof![
        Just(PolicyCommand::All),
        Just(PolicyCommand::Select),
        Just(PolicyCommand::Insert),
        Just(PolicyCommand::Update),
        Just(PolicyCommand::Delete),
    ]
}

/// Generate a random [`Policy`].
///
/// Respects the PG rule that `WITH CHECK` is invalid on `FOR SELECT` and
/// `FOR DELETE` policies — generated policies never produce invalid SQL.
/// Names are drawn from a small fixed pool so deduplication in
/// `arbitrary_table` is effective.
pub(super) fn arb_policy() -> BoxedStrategy<Policy> {
    let name_strategy = prop_oneof![
        Just(POLICY_NAMES[0]),
        Just(POLICY_NAMES[1]),
        Just(POLICY_NAMES[2]),
        Just(POLICY_NAMES[3]),
        Just(POLICY_NAMES[4]),
        Just(POLICY_NAMES[5]),
    ]
    .prop_map(|s| Identifier::from_unquoted(s).unwrap());

    let roles_strategy = proptest_vec(
        prop_oneof![
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
        ],
        0..3,
    );

    (
        name_strategy,
        any::<bool>(),
        arb_policy_command(),
        roles_strategy,
    )
        .prop_flat_map(|(name, permissive, command, mut roles)| {
            // Normalize roles: ensure non-empty (PG omission → PUBLIC) and deduplicate.
            if roles.is_empty() {
                roles.push(GrantTarget::Public);
            }
            roles.sort();
            roles.dedup();

            // WITH CHECK is only valid for ALL / INSERT / UPDATE.
            let with_check_strategy: BoxedStrategy<Option<NormalizedExpr>> =
                if command.allows_with_check() {
                    prop_oneof![
                        Just(None),
                        Just(Some(NormalizedExpr::from_canonical_text("true"))),
                    ]
                    .boxed()
                } else {
                    Just(None).boxed()
                };

            // USING is only valid for ALL / SELECT / UPDATE / DELETE.
            // PG rejects USING on FOR INSERT policies.
            let using_strategy: BoxedStrategy<Option<NormalizedExpr>> = if command.allows_using() {
                prop_oneof![
                    Just(None),
                    Just(Some(NormalizedExpr::from_canonical_text("true"))),
                ]
                .boxed()
            } else {
                Just(None).boxed()
            };

            (
                Just(name),
                Just(permissive),
                Just(command),
                Just(roles),
                using_strategy,
                with_check_strategy,
            )
        })
        .prop_map(
            |(name, permissive, command, roles, using, with_check)| Policy {
                name,
                permissive,
                command,
                roles,
                using,
                with_check,
            },
        )
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::test_runner::TestRunner;

    /// Verify that the generator never produces a policy violating PG's
    /// predicate-validity matrix:
    ///  - INSERT  → USING must be absent
    ///  - SELECT  → WITH CHECK must be absent
    ///  - DELETE  → WITH CHECK must be absent
    #[test]
    fn predicate_gating_matches_pg_rules() {
        let mut runner = TestRunner::new(proptest::test_runner::Config {
            cases: 256,
            ..Default::default()
        });
        runner
            .run(&arb_policy(), |policy| {
                if policy.command == PolicyCommand::Insert {
                    prop_assert!(
                        policy.using.is_none(),
                        "INSERT policy must not carry USING; got {:?}",
                        policy
                    );
                }
                if policy.command == PolicyCommand::Select {
                    prop_assert!(
                        policy.with_check.is_none(),
                        "SELECT policy must not carry WITH CHECK; got {:?}",
                        policy
                    );
                }
                if policy.command == PolicyCommand::Delete {
                    prop_assert!(
                        policy.with_check.is_none(),
                        "DELETE policy must not carry WITH CHECK; got {:?}",
                        policy
                    );
                }
                Ok(())
            })
            .expect("predicate gating test failed");
    }
}
