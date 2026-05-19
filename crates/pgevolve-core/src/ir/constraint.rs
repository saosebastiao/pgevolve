//! `Constraint` and related types.
//!
//! The IR represents fully-validated constraints. The `NOT VALID` intermediate
//! state is a planner concern; it does not appear here.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, DiffMacro, diff_field, prefix_diffs};

/// A table constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraint {
    /// Schema-qualified constraint name (constraints carry their own names).
    pub qname: QualifiedName,
    /// What the constraint enforces.
    pub kind: ConstraintKind,
    /// Deferrability.
    pub deferrable: Deferrable,
    /// Optional comment.
    pub comment: Option<String>,
}

/// What a constraint enforces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConstraintKind {
    /// `PRIMARY KEY (cols) [INCLUDE (cols)]`. Column order is significant.
    PrimaryKey {
        /// Key columns; order matters.
        columns: Vec<Identifier>,
        /// `INCLUDE` (covering) columns.
        include: Vec<Identifier>,
    },
    /// `UNIQUE (cols) [INCLUDE (cols)] [NULLS [NOT] DISTINCT]`.
    Unique {
        /// Unique columns; order matters.
        columns: Vec<Identifier>,
        /// `INCLUDE` columns.
        include: Vec<Identifier>,
        /// Default Postgres semantics: nulls are distinct (i.e., multiple NULLs allowed).
        /// PG 15+ supports `NULLS NOT DISTINCT` to disallow duplicate NULLs.
        nulls_distinct: bool,
    },
    /// `FOREIGN KEY ...`.
    ForeignKey(ForeignKey),
    /// `CHECK (expr)`.
    Check {
        /// Boolean predicate expression.
        expression: NormalizedExpr,
        /// `NO INHERIT` flag.
        no_inherit: bool,
    },
}

/// `FOREIGN KEY ... REFERENCES ...` definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct ForeignKey {
    /// Local columns; order matches `referenced_columns`.
    #[diff(via_debug)]
    pub columns: Vec<Identifier>,
    /// Referenced table.
    pub referenced_table: QualifiedName,
    /// Referenced columns; order matches `columns`.
    #[diff(via_debug)]
    pub referenced_columns: Vec<Identifier>,
    /// Action on update.
    #[diff(via_debug)]
    pub on_update: ReferentialAction,
    /// Action on delete.
    #[diff(via_debug)]
    pub on_delete: ReferentialAction,
    /// Match type.
    #[diff(via_debug)]
    pub match_type: FkMatchType,
}

/// Referential action for `ON UPDATE` / `ON DELETE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferentialAction {
    /// `NO ACTION` (default).
    NoAction,
    /// `RESTRICT`.
    Restrict,
    /// `CASCADE`.
    Cascade,
    /// `SET NULL [ (cols) ]`.
    SetNull(Vec<Identifier>),
    /// `SET DEFAULT [ (cols) ]`.
    SetDefault(Vec<Identifier>),
}

/// Foreign-key match type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FkMatchType {
    /// `MATCH SIMPLE` (default).
    Simple,
    /// `MATCH FULL`.
    Full,
}

/// Deferrability of a constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Deferrable {
    /// `NOT DEFERRABLE` (default).
    NotDeferrable,
    /// `DEFERRABLE [INITIALLY DEFERRED|IMMEDIATE]`.
    Deferrable {
        /// `INITIALLY DEFERRED` if true; `INITIALLY IMMEDIATE` otherwise.
        initially_deferred: bool,
    },
}

fn render_idents(v: &[Identifier]) -> String {
    let mut s = String::from("[");
    for (i, id) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(id.as_str());
    }
    s.push(']');
    s
}

impl Diff for Constraint {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field(
            "deferrable",
            &format!("{:?}", self.deferrable),
            &format!("{:?}", other.deferrable),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out.extend(self.kind.diff(&other.kind));
        out
    }
}

impl Diff for ConstraintKind {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        match (self, other) {
            (
                Self::PrimaryKey {
                    columns: c1,
                    include: i1,
                },
                Self::PrimaryKey {
                    columns: c2,
                    include: i2,
                },
            ) => {
                out.extend(diff_field(
                    "kind.columns",
                    &render_idents(c1),
                    &render_idents(c2),
                ));
                out.extend(diff_field(
                    "kind.include",
                    &render_idents(i1),
                    &render_idents(i2),
                ));
            }
            (
                Self::Unique {
                    columns: c1,
                    include: i1,
                    nulls_distinct: n1,
                },
                Self::Unique {
                    columns: c2,
                    include: i2,
                    nulls_distinct: n2,
                },
            ) => {
                out.extend(diff_field(
                    "kind.columns",
                    &render_idents(c1),
                    &render_idents(c2),
                ));
                out.extend(diff_field(
                    "kind.include",
                    &render_idents(i1),
                    &render_idents(i2),
                ));
                out.extend(diff_field("kind.nulls_distinct", n1, n2));
            }
            (Self::ForeignKey(a), Self::ForeignKey(b)) => {
                out.extend(prefix_diffs("kind.fk", a.diff(b)));
            }
            (
                Self::Check {
                    expression: e1,
                    no_inherit: n1,
                },
                Self::Check {
                    expression: e2,
                    no_inherit: n2,
                },
            ) => {
                out.extend(diff_field(
                    "kind.expression",
                    &e1.canonical_text,
                    &e2.canonical_text,
                ));
                out.extend(diff_field("kind.no_inherit", n1, n2));
            }
            _ => {
                out.push(Difference::new(
                    "kind",
                    format!("{self:?}"),
                    format!("{other:?}"),
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn pk_constraint(cols: &[&str]) -> Constraint {
        Constraint {
            qname: qn("app", "users_pkey"),
            kind: ConstraintKind::PrimaryKey {
                columns: cols.iter().map(|c| id(c)).collect(),
                include: vec![],
            },
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        }
    }

    #[test]
    fn equal_pks_have_no_diff() {
        assert!(pk_constraint(&["id"]).canonical_eq(&pk_constraint(&["id"])));
    }

    #[test]
    fn pk_column_list_change_diffs() {
        let a = pk_constraint(&["id"]);
        let b = pk_constraint(&["id", "tenant_id"]);
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "kind.columns"));
    }

    #[test]
    fn pk_column_order_matters() {
        let a = pk_constraint(&["a", "b"]);
        let b = pk_constraint(&["b", "a"]);
        assert!(!a.canonical_eq(&b));
    }

    #[test]
    fn fk_on_delete_change_diffs() {
        let mk = |on_delete| Constraint {
            qname: qn("app", "users_org_fkey"),
            kind: ConstraintKind::ForeignKey(ForeignKey {
                columns: vec![id("org_id")],
                referenced_table: qn("app", "orgs"),
                referenced_columns: vec![id("id")],
                on_update: ReferentialAction::NoAction,
                on_delete,
                match_type: FkMatchType::Simple,
            }),
            deferrable: Deferrable::NotDeferrable,
            comment: None,
        };
        let a = mk(ReferentialAction::NoAction);
        let b = mk(ReferentialAction::Cascade);
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "kind.fk.on_delete"));
    }

    #[test]
    fn pk_vs_unique_kind_change() {
        let a = pk_constraint(&["id"]);
        let b = Constraint {
            kind: ConstraintKind::Unique {
                columns: vec![id("id")],
                include: vec![],
                nulls_distinct: true,
            },
            ..pk_constraint(&["id"])
        };
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "kind"));
    }

    #[test]
    fn deferrable_change_diffs() {
        let mut b = pk_constraint(&["id"]);
        b.deferrable = Deferrable::Deferrable {
            initially_deferred: true,
        };
        let d = pk_constraint(&["id"]).diff(&b);
        assert!(d.iter().any(|x| x.path == "deferrable"));
    }
}
