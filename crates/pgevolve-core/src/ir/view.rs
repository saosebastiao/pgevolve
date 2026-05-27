//! `View` and `MaterializedView` — Postgres view IR records.
//!
//! These types are the flat IR representation of views introduced in v0.2.
//! They reference [`NormalizedBody`] for the canonicalized SELECT body and
//! [`DepEdge`] for body-extracted dependency provenance.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};

/// `WITH [LOCAL | CASCADED] CHECK OPTION` setting on a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckOption {
    /// `WITH LOCAL CHECK OPTION` — applies only to this view's predicate.
    Local,
    /// `WITH CASCADED CHECK OPTION` — applies through chained updatable views.
    Cascaded,
}
use crate::ir::column_type::ColumnType;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, diff_field};
use crate::parse::normalize_body::NormalizedBody;
use crate::plan::edges::DepEdge;

/// A single named column in a view or materialized view.
///
/// Postgres allows column alias lists on `CREATE VIEW` to override the
/// column names derived from the SELECT list. This struct records the
/// (possibly overridden) column name, its resolved type, and any attached
/// comment.
///
/// ## `column_type` sentinel
///
/// When `ViewColumn` is constructed from an explicit alias list in T3
/// (parsing), the type is not yet known — it requires resolving the SELECT
/// body against the catalog. In that case `column_type` is set to
/// `ColumnType::Other { raw: "unresolved".to_string() }`, which serves as
/// a parser-internal sentinel. T4's AST canonicalization pass replaces it
/// with the resolved type. The sentinel **must never appear** in a serialized
/// catalog or plan — T4 always runs before serialization.
///
/// When `ViewColumn` is built from the live catalog (T5), `column_type` is
/// parsed directly from `format_type(a.atttypid, a.atttypmod)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewColumn {
    /// Column name as it appears in the view definition (or is aliased).
    pub name: Identifier,
    /// Resolved data type of the column.
    ///
    /// Set to `ColumnType::Other { raw: "unresolved".to_string() }` as a
    /// parser-internal sentinel when type resolution has not yet occurred
    /// (T3 → T4 transition). Must be fully resolved before serialization.
    pub column_type: ColumnType,
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
    /// `WITH [LOCAL | CASCADED] CHECK OPTION`, when set in source.
    /// `None` = unmanaged (lenient — operator may have set it out-of-band;
    /// pgevolve neither sets nor resets unless source declares).
    pub check_option: Option<CheckOption>,
    /// Optional `COMMENT ON VIEW` text.
    pub comment: Option<String>,
    /// Raw SELECT body text from source SQL. Populated by the parser (T3);
    /// consumed by the AST canonicalization pass (T4) to fill
    /// `body_canonical` and `body_dependencies`. Not serialized to plan
    /// output or JSON (T4 produces the canonical form which IS serialized).
    #[serde(skip, default)]
    pub raw_body: String,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER VIEW ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    pub grants: Vec<crate::ir::grant::Grant>,
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
    /// Raw SELECT body text from source SQL. Populated by the parser (T3);
    /// consumed by the AST canonicalization pass (T4) to fill
    /// `body_canonical` and `body_dependencies`. Not serialized to plan
    /// output or JSON (T4 produces the canonical form which IS serialized).
    #[serde(skip, default)]
    pub raw_body: String,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER MATERIALIZED VIEW ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    pub grants: Vec<crate::ir::grant::Grant>,
    /// Storage parameters. Same key set as Table.
    pub storage: crate::ir::reloptions::MaterializedViewStorageOptions,
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
                    if l.column_type != r.column_type {
                        out.push(Difference::new(
                            format!("columns.{name}.column_type"),
                            l.column_type.render_sql(),
                            r.column_type.render_sql(),
                        ));
                    }
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
            "storage",
            &format!("{:?}", self.storage),
            &format!("{:?}", other.storage),
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
                    if l.column_type != r.column_type {
                        out.push(Difference::new(
                            format!("columns.{name}.column_type"),
                            l.column_type.render_sql(),
                            r.column_type.render_sql(),
                        ));
                    }
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
    use crate::ir::column_type::ColumnType;
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
                column_type: ColumnType::BigInt,
                comment: None,
            }],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        }
    }

    fn simple_mv(schema: &str, name: &str) -> MaterializedView {
        MaterializedView {
            qname: qn(schema, name),
            columns: vec![],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
        }
    }

    #[test]
    fn views_with_equal_fields_compare_equal() {
        let v1 = simple_view("app", "active_users");
        let v2 = View {
            qname: qn("app", "active_users"),
            columns: vec![ViewColumn {
                name: id("id"),
                column_type: ColumnType::BigInt,
                comment: None,
            }],
            body_canonical: body("SELECT 1"),
            body_dependencies: vec![],
            security_barrier: None,
            security_invoker: None,
            check_option: None,
            comment: None,
            raw_body: String::new(),
            owner: None,
            grants: vec![],
        };
        assert_eq!(v1, v2);
    }

    #[test]
    fn materialized_view_round_trips_through_serde() {
        let mv = MaterializedView {
            qname: qn("app", "summary"),
            columns: vec![ViewColumn {
                name: id("total"),
                column_type: ColumnType::BigInt,
                comment: Some("total count".to_string()),
            }],
            body_canonical: body("SELECT count(*) FROM users"),
            body_dependencies: vec![DepEdge {
                from: NodeId::Table(qn("app", "summary")),
                to: NodeId::Table(qn("app", "users")),
                source: DepSource::AstExtracted,
            }],
            comment: Some("materialized summary".to_string()),
            raw_body: String::new(),
            owner: None,
            grants: vec![],
            storage: crate::ir::reloptions::MaterializedViewStorageOptions::default(),
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

    #[test]
    fn view_owner_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = simple_view("app", "active_users");
        b.owner = Some(id("new_owner"));
        assert!(
            simple_view("app", "active_users")
                .diff(&b)
                .iter()
                .any(|x| x.path == "owner")
        );
    }

    #[test]
    fn view_grants_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = simple_view("app", "active_users");
        b.grants.push(crate::ir::grant::Grant {
            grantee: crate::ir::grant::GrantTarget::Public,
            privilege: crate::ir::grant::Privilege::Select,
            with_grant_option: false,
            columns: None,
        });
        assert!(
            simple_view("app", "active_users")
                .diff(&b)
                .iter()
                .any(|x| x.path == "grants")
        );
    }

    #[test]
    fn materialized_view_owner_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = simple_mv("app", "my_mv");
        b.owner = Some(id("new_owner"));
        assert!(
            simple_mv("app", "my_mv")
                .diff(&b)
                .iter()
                .any(|x| x.path == "owner")
        );
    }

    #[test]
    fn materialized_view_grants_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = simple_mv("app", "my_mv");
        b.grants.push(crate::ir::grant::Grant {
            grantee: crate::ir::grant::GrantTarget::Public,
            privilege: crate::ir::grant::Privilege::Select,
            with_grant_option: false,
            columns: None,
        });
        assert!(
            simple_mv("app", "my_mv")
                .diff(&b)
                .iter()
                .any(|x| x.path == "grants")
        );
    }

    #[test]
    fn materialized_view_storage_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = simple_mv("app", "my_mv");
        b.storage = crate::ir::reloptions::MaterializedViewStorageOptions {
            fillfactor: Some(80),
            ..Default::default()
        };
        assert!(
            simple_mv("app", "my_mv")
                .diff(&b)
                .iter()
                .any(|x| x.path == "storage")
        );
    }

    #[test]
    fn check_option_local_does_not_equal_cascaded() {
        assert_ne!(CheckOption::Local, CheckOption::Cascaded);
    }

    #[test]
    fn check_option_implements_copy() {
        let a = CheckOption::Local;
        let _b = a; // copies
        let _c = a; // still usable
        assert_eq!(a, CheckOption::Local);
    }
}
