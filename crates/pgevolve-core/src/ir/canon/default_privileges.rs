//! Canon rules for `default_privileges`.

use crate::ir::default_privileges::DefaultPrivilegeRule;

/// Sort default-privilege rules by `(target_role, schema, object_type)` and
/// canonicalize each rule's grants list.
///
/// Per rule we first strip the PG-implicit `PUBLIC` grants for the rule's
/// object type (`USAGE` on `TYPES`, `EXECUTE` on `FUNCTIONS`), mirroring the
/// live catalog reader so that source IR — including IR-generator output that
/// never passes through the SQL parser — normalises identically. Then the
/// grant list is canonicalized.
///
/// Rules with an empty grants list are stripped: Postgres has no DDL to
/// create a zero-grant default-privilege rule, so such an entry is
/// semantically equivalent to "no rule". Keeping empty rules in the IR would
/// cause spurious `catalogs-differ` failures when comparing against a live
/// database that never received a GRANT step. A rule whose only grant was the
/// implicit `PUBLIC` entry empties out above and is removed here.
pub fn run(rules: &mut Vec<DefaultPrivilegeRule>) {
    for rule in rules.iter_mut() {
        super::grants::strip_public_implicit_grants(&mut rule.grants, rule.object_type);
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

    /// Regression for the soak `default_privileges.*.TYPES: present vs removed`
    /// failure. The IR generator (and any source path that does not go through
    /// the SQL parser) can produce a default-privilege rule whose only grant is
    /// the PG-implicit `(PUBLIC, USAGE)` on `TYPES` — or `(PUBLIC, EXECUTE)` on
    /// `FUNCTIONS`. The live catalog reader strips these implicit grants, so the
    /// live rule becomes empty and is dropped, while the source rule survived,
    /// making `diff(live, source)` non-empty. Canon must strip the same implicit
    /// grants so source IR and live IR normalise identically; the resulting
    /// empty rule is then removed by the empty-grants filter.
    #[test]
    fn strips_implicit_public_grants_then_drops_empty_rule() {
        let mut rules = vec![
            // TYPES rule whose only grant is implicit PUBLIC/USAGE → empties out.
            DefaultPrivilegeRule {
                target_role: id("app_owner"),
                schema: Some(id("ops")),
                object_type: DefaultPrivObjectType::Types,
                grants: vec![Grant {
                    grantee: GrantTarget::Public,
                    privilege: Privilege::Usage,
                    with_grant_option: false,
                    columns: None,
                }],
            },
            // FUNCTIONS rule whose only grant is implicit PUBLIC/EXECUTE → empties out.
            DefaultPrivilegeRule {
                target_role: id("app_owner"),
                schema: Some(id("ops")),
                object_type: DefaultPrivObjectType::Functions,
                grants: vec![Grant {
                    grantee: GrantTarget::Public,
                    privilege: Privilege::Execute,
                    with_grant_option: false,
                    columns: None,
                }],
            },
        ];
        run(&mut rules);
        assert!(
            rules.is_empty(),
            "implicit-only rules must be stripped to empty and removed; remaining: {rules:?}"
        );
    }

    /// A `TYPES` rule carrying a *real* user grant alongside the implicit one
    /// keeps the user grant but drops the implicit `(PUBLIC, USAGE)`.
    #[test]
    fn strips_implicit_but_keeps_real_grants() {
        let mut rules = vec![DefaultPrivilegeRule {
            target_role: id("app_owner"),
            schema: Some(id("ops")),
            object_type: DefaultPrivObjectType::Types,
            grants: vec![
                Grant {
                    grantee: GrantTarget::Public,
                    privilege: Privilege::Usage,
                    with_grant_option: false,
                    columns: None,
                },
                Grant {
                    grantee: GrantTarget::Role(id("readers")),
                    privilege: Privilege::Usage,
                    with_grant_option: false,
                    columns: None,
                },
            ],
        }];
        run(&mut rules);
        assert_eq!(rules.len(), 1, "rule with a real grant survives");
        assert_eq!(
            rules[0].grants.len(),
            1,
            "only the implicit grant is stripped"
        );
        assert!(
            matches!(&rules[0].grants[0].grantee, GrantTarget::Role(n) if n.as_str() == "readers"),
        );
    }

    /// A WGO `(PUBLIC, USAGE WITH GRANT OPTION)` on `TYPES` is user-declared
    /// (PG's implicit grant is never WGO) and must be kept.
    #[test]
    fn keeps_wgo_public_grant_on_types() {
        let mut rules = vec![DefaultPrivilegeRule {
            target_role: id("app_owner"),
            schema: Some(id("ops")),
            object_type: DefaultPrivObjectType::Types,
            grants: vec![Grant {
                grantee: GrantTarget::Public,
                privilege: Privilege::Usage,
                with_grant_option: true,
                columns: None,
            }],
        }];
        run(&mut rules);
        assert_eq!(rules.len(), 1, "WGO public grant is user-declared, kept");
        assert!(rules[0].grants[0].with_grant_option);
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
