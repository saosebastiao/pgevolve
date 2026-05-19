//! `Index` and related types.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::eq::DiffMacro;

/// The parent object of an [`Index`]: either a table or a materialized view.
///
/// Introduced in v0.2 to support MV indexes alongside table indexes.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IndexParent {
    /// The index targets a regular table.
    Table(QualifiedName),
    /// The index targets a materialized view.
    Mv(QualifiedName),
}

impl IndexParent {
    /// The qualified name of the parent (table or MV).
    pub const fn qname(&self) -> &QualifiedName {
        match self {
            Self::Table(q) | Self::Mv(q) => q,
        }
    }

    /// True if the parent is a materialized view.
    pub const fn is_mv(&self) -> bool {
        matches!(self, Self::Mv(_))
    }
}

/// A Postgres index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DiffMacro)]
pub struct Index {
    /// Schema-qualified index name.
    pub qname: QualifiedName,
    /// The parent table or materialized view this index is defined on.
    #[diff(via_debug)]
    pub on: IndexParent,
    /// Index access method.
    #[diff(via_debug)]
    pub method: IndexMethod,
    /// Indexed columns / expressions; order is significant.
    #[diff(via_debug)]
    pub columns: Vec<IndexColumn>,
    /// `INCLUDE (cols)` covering columns.
    #[diff(via_debug)]
    pub include: Vec<Identifier>,
    /// True for `UNIQUE` indexes.
    pub unique: bool,
    /// PG 15+ `NULLS NOT DISTINCT`.
    pub nulls_not_distinct: bool,
    /// Optional partial-index predicate.
    #[diff(via_debug)]
    pub predicate: Option<NormalizedExpr>,
    /// Optional tablespace.
    #[diff(via_debug)]
    pub tablespace: Option<Identifier>,
    /// Optional comment.
    #[diff(via_debug)]
    pub comment: Option<String>,
}

/// One indexed column or expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexColumn {
    /// Column name or expression.
    pub expr: IndexColumnExpr,
    /// Optional collation.
    pub collation: Option<QualifiedName>,
    /// Optional operator class.
    pub opclass: Option<QualifiedName>,
    /// `ASC` or `DESC`.
    pub sort_order: SortOrder,
    /// `NULLS FIRST` or `NULLS LAST`.
    pub nulls_order: NullsOrder,
}

/// Either a bare column name or an expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexColumnExpr {
    /// Bare column reference.
    Column(Identifier),
    /// Computed expression.
    Expression(NormalizedExpr),
}

/// Index access method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexMethod {
    /// `btree`.
    BTree,
    /// `hash`.
    Hash,
    /// `gin`.
    Gin,
    /// `gist`.
    Gist,
    /// `brin`.
    Brin,
    /// `spgist`.
    Spgist,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// `ASC` (default for B-tree).
    Asc,
    /// `DESC`.
    Desc,
}

/// Null ordering within an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NullsOrder {
    /// `NULLS FIRST`.
    NullsFirst,
    /// `NULLS LAST`.
    NullsLast,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eq::Diff;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn col(name: &str) -> IndexColumn {
        IndexColumn {
            expr: IndexColumnExpr::Column(id(name)),
            collation: None,
            opclass: None,
            sort_order: SortOrder::Asc,
            nulls_order: NullsOrder::NullsLast,
        }
    }

    fn base() -> Index {
        Index {
            qname: qn("app", "users_email_idx"),
            on: IndexParent::Table(qn("app", "users")),
            method: IndexMethod::BTree,
            columns: vec![col("email")],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        }
    }

    #[test]
    fn equal_indexes_have_no_diff() {
        assert!(base().canonical_eq(&base()));
    }

    #[test]
    fn unique_change_diffs() {
        let mut b = base();
        b.unique = false;
        assert!(base().diff(&b).iter().any(|x| x.path == "unique"));
    }

    #[test]
    fn include_columns_diff() {
        let mut b = base();
        b.include = vec![id("name")];
        assert!(base().diff(&b).iter().any(|x| x.path == "include"));
    }

    #[test]
    fn predicate_change_diffs() {
        let mut b = base();
        b.predicate = Some(NormalizedExpr::from_text("deleted_at is null"));
        assert!(base().diff(&b).iter().any(|x| x.path == "predicate"));
    }

    #[test]
    fn opclass_change_diffs() {
        let mut b = base();
        b.columns[0].opclass = Some(qn("pg_catalog", "text_pattern_ops"));
        assert!(base().diff(&b).iter().any(|x| x.path == "columns"));
    }

    #[test]
    fn column_order_matters() {
        let a = Index {
            columns: vec![col("a"), col("b")],
            ..base()
        };
        let b = Index {
            columns: vec![col("b"), col("a")],
            ..base()
        };
        assert!(!a.canonical_eq(&b));
    }

    #[test]
    fn index_can_target_a_materialized_view() {
        let mv_idx = Index {
            qname: qn("app", "mv_email_idx"),
            on: IndexParent::Mv(qn("app", "users_mv")),
            method: IndexMethod::BTree,
            columns: vec![col("email")],
            include: vec![],
            unique: true,
            nulls_not_distinct: false,
            predicate: None,
            tablespace: None,
            comment: None,
        };
        assert!(mv_idx.on.is_mv());
        assert_eq!(mv_idx.on.qname().to_string(), "app.users_mv");
        assert!(mv_idx.canonical_eq(&mv_idx));

        // A table-parent index does not report is_mv.
        let tbl_idx = base();
        assert!(!tbl_idx.on.is_mv());
        assert_eq!(tbl_idx.on.qname().to_string(), "app.users");

        // An MV-parent index differs from a Table-parent index.
        let tbl_idx_same_name = Index {
            qname: qn("app", "mv_email_idx"),
            on: IndexParent::Table(qn("app", "users_mv")),
            ..mv_idx.clone()
        };
        assert!(!mv_idx.canonical_eq(&tbl_idx_same_name));
        let diffs = mv_idx.diff(&tbl_idx_same_name);
        assert!(diffs.iter().any(|d| d.path == "on"));
    }
}
