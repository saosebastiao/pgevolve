//! `View` and `MaterializedView` — Postgres view IR records.
//!
//! These types are the flat IR representation of views introduced in v0.2.
//! They reference [`NormalizedBody`] for the canonicalized SELECT body and
//! [`DepEdge`] for body-extracted dependency provenance.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};
use crate::parse::normalize_body::NormalizedBody;
use crate::plan::edges::DepEdge;

/// A single named column in a view or materialized view.
///
/// Postgres allows column alias lists on `CREATE VIEW` to override the
/// column names derived from the SELECT list. This struct records the
/// (possibly overridden) column name and any attached comment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewColumn {
    /// Column name as it appears in the view definition (or is aliased).
    pub name: Identifier,
    /// Optional `COMMENT ON COLUMN` text.
    pub comment: Option<String>,
}

/// A Postgres `CREATE VIEW`.
///
/// The `body_canonical` is the parsed-and-deparsed SELECT statement in
/// canonical form. `body_dependencies` lists the IR objects the body
/// references, extracted from the AST (v0.2 task 4; initially empty until
/// the AST-walk pass lands).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct View {
    /// Schema-qualified view name.
    pub qname: QualifiedName,
    /// Explicit column alias list (empty when none was provided).
    pub columns: Vec<ViewColumn>,
    /// Canonical form of the SELECT body.
    pub body_canonical: NormalizedBody,
    /// Dependency edges extracted from the body AST.
    pub body_dependencies: Vec<DepEdge>,
    /// `WITH (security_barrier = ...)` option, if present.
    pub security_barrier: Option<bool>,
    /// `WITH (security_invoker = ...)` option, if present.
    pub security_invoker: Option<bool>,
    /// Optional `COMMENT ON VIEW` text.
    pub comment: Option<String>,
}

/// A Postgres `CREATE MATERIALIZED VIEW`.
///
/// Unlike regular views, materialized views are physically stored.
/// They lack the `security_barrier` / `security_invoker` options of regular
/// views but are otherwise structurally similar.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MaterializedView {
    /// Schema-qualified materialized view name.
    pub qname: QualifiedName,
    /// Explicit column alias list (empty when none was provided).
    pub columns: Vec<ViewColumn>,
    /// Canonical form of the SELECT body.
    pub body_canonical: NormalizedBody,
    /// Dependency edges extracted from the body AST.
    pub body_dependencies: Vec<DepEdge>,
    /// Optional `COMMENT ON MATERIALIZED VIEW` text.
    pub comment: Option<String>,
}

impl Diff for View {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field(
            "body_canonical",
            &self.body_canonical.canonical_text(),
            &other.body_canonical.canonical_text(),
        ));
        out.extend(diff_field(
            "security_barrier",
            &format!("{:?}", self.security_barrier),
            &format!("{:?}", other.security_barrier),
        ));
        out.extend(diff_field(
            "security_invoker",
            &format!("{:?}", self.security_invoker),
            &format!("{:?}", other.security_invoker),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));

        // Column diff: pair by name.
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
                    if l.comment != r.comment {
                        out.push(Difference::new(
                            format!("columns.{name}.comment"),
                            format!("{:?}", l.comment),
                            format!("{:?}", r.comment),
                        ));
                    }
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
        let lhs_order: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        let rhs_order: Vec<&str> = other.columns.iter().map(|c| c.name.as_str()).collect();
        if lhs_order != rhs_order {
            out.push(Difference::new(
                "columns.<order>",
                lhs_order.join(","),
                rhs_order.join(","),
            ));
        }

        // Dependency-edge diff: format vec for comparison.
        out.extend(diff_field(
            "body_dependencies",
            &format!("{:?}", self.body_dependencies),
            &format!("{:?}", other.body_dependencies),
        ));

        out
    }
}

impl Diff for MaterializedView {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(diff_field("qname", &self.qname, &other.qname));
        out.extend(diff_field(
            "body_canonical",
            &self.body_canonical.canonical_text(),
            &other.body_canonical.canonical_text(),
        ));
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));

        // Column diff: pair by name.
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
                    if l.comment != r.comment {
                        out.push(Difference::new(
                            format!("columns.{name}.comment"),
                            format!("{:?}", l.comment),
                            format!("{:?}", r.comment),
                        ));
                    }
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
        let lhs_order: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        let rhs_order: Vec<&str> = other.columns.iter().map(|c| c.name.as_str()).collect();
        if lhs_order != rhs_order {
            out.push(Difference::new(
                "columns.<order>",
                lhs_order.join(","),
                rhs_order.join(","),
            ));
        }

        // Dependency-edge diff: format vec for comparison.
        out.extend(diff_field(
            "body_dependencies",
            &format!("{:?}", self.body_dependencies),
            &format!("{:?}", other.body_dependencies),
        ));

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::IrError;
    use crate::ir::catalog::Catalog;
    use crate::plan::edges::{DepSource, NodeId};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn body(sql: &str) -> NormalizedBody {
        NormalizedBody::from_sql(sql).unwrap()
    }

    fn simple_view(schema: &str, name: &str) -> View {
        View {
            qname: qn(schema, name),
            columns: vec![ViewColumn {
                name: id("id"),
                comment: None,
            }],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
        }
    }

    fn simple_mv(schema: &str, name: &str) -> MaterializedView {
        MaterializedView {
            qname: qn(schema, name),
            columns: vec![],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            comment: None,
        }
    }

    #[test]
    fn views_with_equal_fields_compare_equal() {
        let v1 = simple_view("app", "active_users");
        let v2 = View {
            qname: qn("app", "active_users"),
            columns: vec![ViewColumn {
                name: id("id"),
                comment: None,
            }],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            comment: None,
        };
        assert_eq!(v1, v2);
    }

    #[test]
    fn materialized_view_round_trips_through_serde() {
        let mv = MaterializedView {
            qname: qn("app", "summary"),
            columns: vec![ViewColumn {
                name: id("total"),
                comment: Some("total count".to_string()),
            }],
            body_canonical: body("SELECT count(*) FROM users"),
            body_dependencies: vec![DepEdge {
                from: NodeId::Table(qn("app", "summary")),
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstExtracted,
            }],
            comment: Some("materialized summary".to_string()),
        };
        let json = serde_json::to_string(&mv).expect("serialization must succeed");
        let roundtripped: MaterializedView =
            serde_json::from_str(&json).expect("deserialization must succeed");
        assert_eq!(mv, roundtripped);
    }

    #[test]
    fn catalog_with_views_canonicalizes() {
        let mut c = Catalog::empty();
        c.views.push(simple_view("app", "zzz_view"));
        c.views.push(simple_view("app", "aaa_view"));
        c.materialized_views.push(simple_mv("app", "zzz_mv"));
        c.materialized_views.push(simple_mv("app", "aaa_mv"));

        let result = c.canonicalize();
        assert!(result.is_ok(), "canonicalize should succeed: {result:?}");
        let canonical = result.unwrap();

        assert_eq!(canonical.views[0].qname, qn("app", "aaa_view"));
        assert_eq!(canonical.views[1].qname, qn("app", "zzz_view"));
        assert_eq!(canonical.materialized_views[0].qname, qn("app", "aaa_mv"));
        assert_eq!(canonical.materialized_views[1].qname, qn("app", "zzz_mv"));
    }

    #[test]
    fn catalog_rejects_duplicate_view_qname() {
        let mut c = Catalog::empty();
        c.views.push(simple_view("app", "my_view"));
        c.views.push(simple_view("app", "my_view"));

        let result = c.canonicalize();
        assert!(
            matches!(result, Err(IrError::InvalidIdentifier(_))),
            "expected duplicate-view error, got: {result:?}",
        );
    }

    #[test]
    fn catalog_rejects_duplicate_materialized_view_qname() {
        let mut c = Catalog::empty();
        c.materialized_views.push(simple_mv("app", "my_mv"));
        c.materialized_views.push(simple_mv("app", "my_mv"));

        let result = c.canonicalize();
        assert!(
            matches!(result, Err(IrError::InvalidIdentifier(_))),
            "expected duplicate-mv error, got: {result:?}",
        );
    }
}
