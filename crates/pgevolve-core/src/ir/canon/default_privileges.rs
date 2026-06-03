//! Canon rules for `default_privileges`.

use crate::ir::default_privileges::DefaultPrivilegeRule;

/// Sort default-privilege rules by `(target_role, schema, object_type)` and
/// canonicalize each rule's grants list.
///
/// Rules with an empty grants list are stripped: Postgres has no DDL to
/// create a zero-grant default-privilege rule, so such an entry is
/// semantically equivalent to "no rule". Keeping empty rules in the IR would
/// cause spurious `catalogs-differ` failures when comparing against a live
/// database that never received a GRANT step.
pub fn run(rules: &mut Vec<DefaultPrivilegeRule>) {
    for rule in rules.iter_mut() {
        super::grants::run_on_list(&mut rule.grants);
    }
    // Remove rules whose grant list is empty after canonicalization.
    rules.retain(|r| !r.grants.is_empty());
    rules.sort_by(|a, b| {
        a.target_role
            .as_str()
            .cmp(b.target_role.as_str())
            .then_with(|| match (&a.schema, &b.schema) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => x.as_str().cmp(y.as_str()),
            })
            .then_with(|| a.object_type.pg_char().cmp(&b.object_type.pg_char()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
    use crate::ir::grant::{Grant, GrantTarget, Privilege};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    /// Build a rule with a single grant so it survives the empty-grants filter.
    fn rule(
        target: &str,
        schema: Option<&str>,
        kind: DefaultPrivObjectType,
    ) -> DefaultPrivilegeRule {
        DefaultPrivilegeRule {
            target_role: id(target),
            schema: schema.map(id),
            object_type: kind,
            // Provide a non-empty grants list so the rule is not stripped.
            grants: vec![Grant {
                grantee: GrantTarget::Public,
                privilege: Privilege::Select,
                with_grant_option: false,
                columns: None,
            }],
        }
    }

    #[test]
    fn sorts_by_target_then_schema_then_type() {
        let mut rules = vec![
            rule("zebra", Some("z"), DefaultPrivObjectType::Tables),
            rule("alice", Some("b"), DefaultPrivObjectType::Tables),
            rule("alice", None, DefaultPrivObjectType::Sequences),
            rule("alice", Some("a"), DefaultPrivObjectType::Tables),
        ];
        run(&mut rules);
        assert_eq!(rules[0].target_role.as_str(), "alice");
        assert!(
            rules[0].schema.is_none(),
            "None schema sorts first for alice"
        );
        assert_eq!(rules[1].target_role.as_str(), "alice");
        assert_eq!(rules[1].schema.as_ref().unwrap().as_str(), "a");
        assert_eq!(rules[2].target_role.as_str(), "alice");
        assert_eq!(rules[2].schema.as_ref().unwrap().as_str(), "b");
        assert_eq!(rules[3].target_role.as_str(), "zebra");
    }

    /// Rules with an empty grants list are semantically equivalent to "no rule"
    /// in Postgres (you cannot create a zero-grant default-privilege rule via
    /// DDL). Canon must strip them so that source catalogs generated with empty
    /// rules do not diverge from a live catalog that has no such rule.
    ///
    /// Regression for issue #25: `drift_recovery_property` surfaced
    /// `default_privileges.app_owner.*.SCHEMAS: 'present' vs 'removed'`
    /// when the source catalog had an empty-grants rule and the apply emitted
    /// nothing (correctly — no grants to add), leaving live without the rule
    /// but the canonical comparison reporting a spurious mismatch.
    #[test]
    fn empty_grants_rule_is_stripped() {
        let mut rules = vec![
            // This rule has grants — must be kept.
            DefaultPrivilegeRule {
                target_role: id("alice"),
                schema: None,
                object_type: DefaultPrivObjectType::Tables,
                grants: vec![Grant {
                    grantee: GrantTarget::Role(id("reader")),
                    privilege: Privilege::Select,
                    with_grant_option: false,
                    columns: None,
                }],
            },
            // This rule has no grants — must be removed.
            DefaultPrivilegeRule {
                target_role: id("app_owner"),
                schema: None,
                object_type: DefaultPrivObjectType::Schemas,
                grants: vec![],
            },
        ];
        run(&mut rules);
        assert_eq!(
            rules.len(),
            1,
            "empty-grants rule should be stripped; remaining: {rules:?}"
        );
        assert_eq!(rules[0].target_role.as_str(), "alice");
    }

    #[test]
    fn delegates_grant_list_to_grants_canon() {
        let mut rules = vec![DefaultPrivilegeRule {
            target_role: id("alice"),
            schema: None,
            object_type: DefaultPrivObjectType::Tables,
            grants: vec![
                Grant {
                    grantee: GrantTarget::Role(id("zelda")),
                    privilege: Privilege::Select,
                    with_grant_option: false,
                    columns: None,
                },
                Grant {
                    grantee: GrantTarget::Public,
                    privilege: Privilege::Select,
                    with_grant_option: false,
                    columns: None,
                },
            ],
        }];
        run(&mut rules);
        // Public sorts before Role per grants canon.
        assert!(matches!(rules[0].grants[0].grantee, GrantTarget::Public));
    }
}
