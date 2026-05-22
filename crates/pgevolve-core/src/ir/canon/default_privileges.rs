//! Canon rules for `default_privileges`.

use crate::ir::default_privileges::DefaultPrivilegeRule;

/// Sort default-privilege rules by `(target_role, schema, object_type)` and
/// canonicalize each rule's grants list.
pub fn run(rules: &mut [DefaultPrivilegeRule]) {
    for rule in rules.iter_mut() {
        super::grants::run_on_list(&mut rule.grants);
    }
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

    fn rule(
        target: &str,
        schema: Option<&str>,
        kind: DefaultPrivObjectType,
    ) -> DefaultPrivilegeRule {
        DefaultPrivilegeRule {
            target_role: id(target),
            schema: schema.map(id),
            object_type: kind,
            grants: vec![],
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
