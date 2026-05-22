//! `Catalog` — a complete schema snapshot.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::IrError;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, prefix_diffs};
use crate::ir::extension::Extension;
use crate::ir::function::Function;
use crate::ir::index::Index;
use crate::ir::procedure::Procedure;
use crate::ir::schema::Schema;
use crate::ir::sequence::Sequence;
use crate::ir::table::Table;
use crate::ir::trigger::Trigger;
use crate::ir::user_type::UserType;
use crate::ir::view::{MaterializedView, View};

/// A whole-database schema snapshot.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Catalog {
    /// Schemas (namespaces).
    pub schemas: Vec<Schema>,
    /// Postgres extensions (`CREATE EXTENSION`).
    pub extensions: Vec<Extension>,
    /// Tables.
    pub tables: Vec<Table>,
    /// Indexes.
    pub indexes: Vec<Index>,
    /// Sequences.
    pub sequences: Vec<Sequence>,
    /// Views.
    pub views: Vec<View>,
    /// Materialized views.
    pub materialized_views: Vec<MaterializedView>,
    /// User-defined types (enums, domains, composites).
    pub types: Vec<UserType>,
    /// User-defined functions.
    pub functions: Vec<Function>,
    /// User-defined procedures.
    pub procedures: Vec<Procedure>,
    /// Triggers (`CREATE TRIGGER` / `CREATE CONSTRAINT TRIGGER`).
    pub triggers: Vec<Trigger>,
}

impl Catalog {
    /// Construct an empty catalog.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            schemas: Vec::new(),
            extensions: Vec::new(),
            tables: Vec::new(),
            indexes: Vec::new(),
            sequences: Vec::new(),
            views: Vec::new(),
            materialized_views: Vec::new(),
            types: Vec::new(),
            functions: Vec::new(),
            procedures: Vec::new(),
            triggers: Vec::new(),
        }
    }

    /// True iff a table with the given qualified name exists in the catalog.
    ///
    /// Used by the planner to decide whether a `CreateIndex` / `AddConstraint`
    /// targets an existing table (eligible for online rewrites) or one being
    /// created in the same plan (must stay inline / transactional).
    pub fn table_exists(&self, qname: &crate::identifier::QualifiedName) -> bool {
        self.tables.iter().any(|t| &t.qname == qname)
    }

    /// Sort each collection by its canonical key and reject duplicates.
    pub fn canonicalize(mut self) -> Result<Self, IrError> {
        crate::ir::canon::canonicalize(&mut self)?;

        Ok(self)
    }
}

impl Diff for Catalog {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(prefix_diffs(
            "schemas",
            diff_keyed(&self.schemas, &other.schemas, |s| s.name.to_string()),
        ));
        out.extend(prefix_diffs(
            "extensions",
            diff_keyed(&self.extensions, &other.extensions, |e| e.name.to_string()),
        ));
        out.extend(prefix_diffs(
            "tables",
            diff_keyed(&self.tables, &other.tables, |t| t.qname.to_string()),
        ));
        out.extend(prefix_diffs(
            "indexes",
            diff_keyed(&self.indexes, &other.indexes, |i| i.qname.to_string()),
        ));
        out.extend(prefix_diffs(
            "sequences",
            diff_keyed(&self.sequences, &other.sequences, |s| s.qname.to_string()),
        ));
        out.extend(prefix_diffs(
            "views",
            diff_keyed(&self.views, &other.views, |v| v.qname.to_string()),
        ));
        out.extend(prefix_diffs(
            "materialized_views",
            diff_keyed(&self.materialized_views, &other.materialized_views, |m| {
                m.qname.to_string()
            }),
        ));
        out.extend(prefix_diffs(
            "types",
            diff_keyed(&self.types, &other.types, |t| t.qname.to_string()),
        ));
        out.extend(prefix_diffs(
            "functions",
            diff_keyed(&self.functions, &other.functions, |f| {
                format!(
                    "{}({})",
                    f.qname,
                    f.arg_types_normalized
                        .types
                        .iter()
                        .map(crate::ir::column_type::ColumnType::render_sql)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }),
        ));
        out.extend(prefix_diffs(
            "procedures",
            diff_keyed(&self.procedures, &other.procedures, |p| p.qname.to_string()),
        ));
        out.extend(prefix_diffs(
            "triggers",
            diff_keyed(&self.triggers, &other.triggers, |t| t.qname.to_string()),
        ));
        out
    }
}

fn diff_keyed<T: Diff, K: Fn(&T) -> String>(lhs: &[T], rhs: &[T], key: K) -> Vec<Difference> {
    let mut out = Vec::new();
    let lhs_map: BTreeMap<String, &T> = lhs.iter().map(|x| (key(x), x)).collect();
    let rhs_map: BTreeMap<String, &T> = rhs.iter().map(|x| (key(x), x)).collect();
    for (k, l) in &lhs_map {
        match rhs_map.get(k) {
            None => out.push(Difference::new(k, "present", "removed")),
            Some(r) => out.extend(prefix_diffs(k, l.diff(r))),
        }
    }
    for k in rhs_map.keys() {
        if !lhs_map.contains_key(k) {
            out.push(Difference::new(k, "missing", "added"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column::Column;
    use crate::ir::column_type::ColumnType;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn col(name: &str, ty: ColumnType) -> Column {
        Column {
            name: id(name),
            ty,
            nullable: false,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    fn table_users() -> Table {
        Table {
            qname: qn("users"),
            columns: vec![col("id", ColumnType::BigInt)],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        }
    }

    #[test]
    fn empty_catalogs_canonical_eq() {
        assert!(Catalog::empty().canonical_eq(&Catalog::empty()));
    }

    #[test]
    fn add_table_reports() {
        let mut b = Catalog::empty();
        b.tables.push(table_users());
        let d = Catalog::empty().diff(&b);
        assert!(d.iter().any(|x| x.path.starts_with("tables.app.users")));
    }

    #[test]
    fn remove_table_reports() {
        let mut a = Catalog::empty();
        a.tables.push(table_users());
        let d = a.diff(&Catalog::empty());
        assert!(d.iter().any(|x| x.path.starts_with("tables.app.users")));
    }

    #[test]
    fn changed_column_under_table_path() {
        let mut a = Catalog::empty();
        a.tables.push(table_users());
        let mut b = Catalog::empty();
        let mut t = table_users();
        t.columns[0].ty = ColumnType::Integer;
        b.tables.push(t);
        let d = a.diff(&b);
        assert!(d.iter().any(|x| x.path == "tables.app.users.columns.id.ty"));
    }

    #[test]
    fn canonicalize_sorts_tables() {
        let mut c = Catalog::empty();
        c.tables.push(Table {
            qname: qn("zzz"),
            columns: vec![],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        c.tables.push(table_users());
        let canonical = c.canonicalize().unwrap();
        assert_eq!(canonical.tables[0].qname, qn("users"));
        assert_eq!(canonical.tables[1].qname, qn("zzz"));
    }

    #[test]
    fn canonicalize_rejects_duplicate_table() {
        let mut c = Catalog::empty();
        c.tables.push(table_users());
        c.tables.push(table_users());
        let r = c.canonicalize();
        assert!(matches!(r, Err(IrError::InvalidIdentifier(_))));
    }
}
