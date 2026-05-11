//! `Index` and related types.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};

/// A Postgres index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// Schema-qualified index name.
    pub qname: QualifiedName,
    /// Table the index is on.
    pub table: QualifiedName,
    /// Index access method.
    pub method: IndexMethod,
    /// Indexed columns / expressions; order is significant.
    pub columns: Vec<IndexColumn>,
    /// `INCLUDE (cols)` covering columns.
    pub include: Vec<Identifier>,
    /// True for `UNIQUE` indexes.
    pub unique: bool,
    /// PG 15+ `NULLS NOT DISTINCT`.
    pub nulls_not_distinct: bool,
    /// Optional partial-index predicate.
    pub predicate: Option<NormalizedExpr>,
    /// Optional tablespace.
    pub tablespace: Option<Identifier>,
    /// Optional comment.
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

impl Diff for Index {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field("table", &self.table, &other.table));
        out.extend(diff_field(
            "method",
            &format!("{:?}", self.method),
            &format!("{:?}", other.method),
        ));
        out.extend(diff_field(
            "columns",
            &format!("{:?}", self.columns),
            &format!("{:?}", other.columns),
        ));
        out.extend(diff_field(
            "include",
            &render_idents(&self.include),
            &render_idents(&other.include),
        ));
        out.extend(diff_field("unique", &self.unique, &other.unique));
        out.extend(diff_field(
            "nulls_not_distinct",
            &self.nulls_not_distinct,
            &other.nulls_not_distinct,
        ));
        out.extend(diff_field(
            "predicate",
            &format!("{:?}", self.predicate),
            &format!("{:?}", other.predicate),
        ));
        out.extend(diff_field(
            "tablespace",
            &format!("{:?}", self.tablespace),
            &format!("{:?}", other.tablespace),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
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
            table: qn("app", "users"),
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
}
