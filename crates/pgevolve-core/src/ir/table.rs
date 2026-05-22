//! `Table` — a Postgres table.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column::Column;
use crate::ir::constraint::Constraint;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field, prefix_diffs};

/// A Postgres table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    /// Schema-qualified table name.
    pub qname: QualifiedName,
    /// Columns in their logical order.
    pub columns: Vec<Column>,
    /// Constraints, paired by `qname` for diffing.
    pub constraints: Vec<Constraint>,
    /// `Some` → this table is a partitioned parent (`PARTITION BY …`).
    pub partition_by: Option<crate::ir::partition::PartitionBy>,
    /// `Some` → this table is itself a partition (`PARTITION OF … FOR VALUES …`).
    pub partition_of: Option<crate::ir::partition::PartitionOf>,
    /// Optional comment.
    pub comment: Option<String>,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER TABLE ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    pub grants: Vec<crate::ir::grant::Grant>,
    /// `ROW LEVEL SECURITY` enabled flag. PG default: false.
    pub rls_enabled: bool,
    /// `FORCE ROW LEVEL SECURITY` flag (applies even to owner). PG default: false.
    pub rls_forced: bool,
    /// Policies attached to this table. Canonicalized in `ir::canon::policies`.
    pub policies: Vec<crate::ir::policy::Policy>,
}

impl Diff for Table {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field(
            "partition_by",
            &format!("{:?}", self.partition_by),
            &format!("{:?}", other.partition_by),
        ));
        out.extend(diff_field(
            "partition_of",
            &format!("{:?}", self.partition_of),
            &format!("{:?}", other.partition_of),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out.extend(diff_field(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(diff_field(
            "grants",
            &format!("{:?}", self.grants),
            &format!("{:?}", other.grants),
        ));
        out.extend(diff_field(
            "rls_enabled",
            &format!("{:?}", self.rls_enabled),
            &format!("{:?}", other.rls_enabled),
        ));
        out.extend(diff_field(
            "rls_forced",
            &format!("{:?}", self.rls_forced),
            &format!("{:?}", other.rls_forced),
        ));
        out.extend(diff_field(
            "policies",
            &format!("{:?}", self.policies),
            &format!("{:?}", other.policies),
        ));

        // Column diff: pair by name, then check positions.
        let lhs: BTreeMap<_, _> = self.columns.iter().map(|c| (c.name.as_str(), c)).collect();
        let rhs: BTreeMap<_, _> = other.columns.iter().map(|c| (c.name.as_str(), c)).collect();
        for (name, l) in &lhs {
            match rhs.get(name) {
                None => out.push(Difference::new(
                    format!("columns.{name}"),
                    "present",
                    "removed",
                )),
                Some(r) => {
                    out.extend(prefix_diffs(&format!("columns.{name}"), l.diff(r)));
                }
            }
        }
        for name in rhs.keys() {
            if !lhs.contains_key(name) {
                out.push(Difference::new(
                    format!("columns.{name}"),
                    "missing",
                    "added",
                ));
            }
        }

        // Position drift: same set of names, different ordering.
        let lhs_order: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        let rhs_order: Vec<&str> = other.columns.iter().map(|c| c.name.as_str()).collect();
        if lhs_order != rhs_order {
            out.push(Difference::new(
                "columns.<order>",
                lhs_order.join(","),
                rhs_order.join(","),
            ));
        }

        // Constraint diff: pair by qname.
        let lhs_cs: BTreeMap<_, _> = self.constraints.iter().map(|c| (&c.qname, c)).collect();
        let rhs_cs: BTreeMap<_, _> = other.constraints.iter().map(|c| (&c.qname, c)).collect();
        for (qn, l) in &lhs_cs {
            match rhs_cs.get(qn) {
                None => out.push(Difference::new(
                    format!("constraints.{qn}"),
                    "present",
                    "removed",
                )),
                Some(r) => {
                    out.extend(prefix_diffs(&format!("constraints.{qn}"), l.diff(r)));
                }
            }
        }
        for qn in rhs_cs.keys() {
            if !lhs_cs.contains_key(qn) {
                out.push(Difference::new(
                    format!("constraints.{qn}"),
                    "missing",
                    "added",
                ));
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::column_type::ColumnType;
    use crate::ir::constraint::{ConstraintKind, Deferrable};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn col(name: &str, ty: ColumnType, nullable: bool) -> Column {
        Column {
            name: id(name),
            ty,
            nullable,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn pk(name: &str, cols: &[&str]) -> Constraint {
        Constraint {
            qname: qn(name),
            kind: ConstraintKind::PrimaryKey {
                columns: cols.iter().map(|c| id(c)).collect(),
                include: vec![],
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    fn base() -> Table {
        Table {
            qname: qn("users"),
            columns: vec![
                col("id", ColumnType::BigInt, false),
                col("email", ColumnType::Text, false),
            ],
            constraints: vec![pk("users_pkey", &["id"])],
            partition_by: None,
            partition_of: None,
            comment: None,
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
        }
    }

    #[test]
    fn equal_tables_have_no_diff() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn add_column_diffs() {
        let mut b = base();
        b.columns.push(col("name", ColumnType::Text, true));
        let d = base().diff(&b);
        assert!(d.iter().any(|x| x.path == "columns.name"));
    }

    #[test]
    fn remove_column_diffs() {
        let mut b = base();
        b.columns.pop();
        let d = base().diff(&b);
        assert!(d.iter().any(|x| x.path == "columns.email"));
    }

    #[test]
    fn reorder_columns_diffs_as_order() {
        let mut b = base();
        b.columns.reverse();
        let d = base().diff(&b);
        assert!(d.iter().any(|x| x.path == "columns.<order>"));
    }

    #[test]
    fn add_constraint_diffs() {
        let mut b = base();
        b.constraints.push(pk("users_alt_pkey", &["email"]));
        let d = base().diff(&b);
        assert!(d.iter().any(|x| x.path == "constraints.app.users_alt_pkey"));
    }

    #[test]
    fn changed_column_definition_diffs_under_path() {
        let mut b = base();
        b.columns[1].nullable = true;
        let d = base().diff(&b);
        assert!(d.iter().any(|x| x.path == "columns.email.nullable"));
    }

    #[test]
    fn owner_change_diffs() {
        let mut b = base();
        b.owner = Some(id("new_owner"));
        assert!(base().diff(&b).iter().any(|x| x.path == "owner"));
    }

    #[test]
    fn grants_change_diffs() {
        let mut b = base();
        b.grants.push(crate::ir::grant::Grant {
            grantee: crate::ir::grant::GrantTarget::Public,
            privilege: crate::ir::grant::Privilege::Select,
            with_grant_option: false,
            columns: None,
        });
        assert!(base().diff(&b).iter().any(|x| x.path == "grants"));
    }

    #[test]
    fn rls_enabled_change_diffs() {
        let mut b = base();
        b.rls_enabled = true;
        assert!(base().diff(&b).iter().any(|x| x.path == "rls_enabled"));
    }

    #[test]
    fn rls_forced_change_diffs() {
        let mut b = base();
        b.rls_forced = true;
        assert!(base().diff(&b).iter().any(|x| x.path == "rls_forced"));
    }

    #[test]
    fn policies_change_diffs() {
        use crate::ir::grant::GrantTarget;
        use crate::ir::policy::{Policy, PolicyCommand};
        let mut b = base();
        b.policies.push(Policy {
            name: id("p1"),
            permissive: true,
            command: PolicyCommand::All,
            roles: vec![GrantTarget::Public],
            using: None,
            with_check: None,
        });
        assert!(base().diff(&b).iter().any(|x| x.path == "policies"));
    }
}
