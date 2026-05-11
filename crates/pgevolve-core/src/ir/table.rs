//! `Table` — a Postgres table.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identifier::QualifiedName;
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
    /// Optional comment.
    pub comment: Option<String>,
}

impl Diff for Table {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
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
            comment: None,
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
}
