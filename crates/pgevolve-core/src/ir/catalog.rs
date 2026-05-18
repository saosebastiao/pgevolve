//! `Catalog` — a complete schema snapshot.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::IrError;
use crate::ir::difference::Difference;
use crate::ir::eq::{Diff, prefix_diffs};
use crate::ir::index::Index;
use crate::ir::schema::Schema;
use crate::ir::sequence::Sequence;
use crate::ir::table::Table;
use crate::ir::view::{MaterializedView, View};

/// A whole-database schema snapshot.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Catalog {
    /// Schemas (namespaces).
    pub schemas: Vec<Schema>,
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
}

impl Catalog {
    /// Construct an empty catalog.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            schemas: Vec::new(),
            tables: Vec::new(),
            indexes: Vec::new(),
            sequences: Vec::new(),
            views: Vec::new(),
            materialized_views: Vec::new(),
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
        self.schemas.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(dupe) = first_duplicate(self.schemas.iter().map(|s| s.name.as_str())) {
            return Err(IrError::InvalidIdentifier(format!(
                "duplicate schema: {dupe}"
            )));
        }

        self.tables.sort_by(|a, b| a.qname.cmp(&b.qname));
        if let Some(dupe) = first_duplicate(self.tables.iter().map(|t| t.qname.to_string())) {
            return Err(IrError::InvalidIdentifier(format!(
                "duplicate table: {dupe}"
            )));
        }

        self.indexes.sort_by(|a, b| a.qname.cmp(&b.qname));
        if let Some(dupe) = first_duplicate(self.indexes.iter().map(|i| i.qname.to_string())) {
            return Err(IrError::InvalidIdentifier(format!(
                "duplicate index: {dupe}"
            )));
        }

        self.sequences.sort_by(|a, b| a.qname.cmp(&b.qname));
        if let Some(dupe) = first_duplicate(self.sequences.iter().map(|s| s.qname.to_string())) {
            return Err(IrError::InvalidIdentifier(format!(
                "duplicate sequence: {dupe}"
            )));
        }

        self.views.sort_by(|a, b| a.qname.cmp(&b.qname));
        if let Some(dupe) = first_duplicate(self.views.iter().map(|v| v.qname.to_string())) {
            return Err(IrError::InvalidIdentifier(format!(
                "duplicate view: {dupe}"
            )));
        }

        self.materialized_views
            .sort_by(|a, b| a.qname.cmp(&b.qname));
        if let Some(dupe) =
            first_duplicate(self.materialized_views.iter().map(|m| m.qname.to_string()))
        {
            return Err(IrError::InvalidIdentifier(format!(
                "duplicate materialized view: {dupe}"
            )));
        }

        Ok(self)
    }
}

fn first_duplicate<T: Ord, I: IntoIterator<Item = T>>(items: I) -> Option<T> {
    let mut seen: Vec<T> = items.into_iter().collect();
    seen.sort();
    let mut iter = seen.into_iter();
    let mut prev = iter.next()?;
    for cur in iter {
        if cur == prev {
            return Some(cur);
        }
        prev = cur;
    }
    None
}

impl Diff for Catalog {
    fn diff(&self, other: &Self) -> Vec<Difference> {
        let mut out = Vec::new();
        out.extend(prefix_diffs(
            "schemas",
            diff_keyed(&self.schemas, &other.schemas, |s| s.name.to_string()),
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
            comment: None,
        }
    }

    fn table_users() -> Table {
        Table {
            qname: qn("users"),
            columns: vec![col("id", ColumnType::BigInt)],
            constraints: vec![],
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
