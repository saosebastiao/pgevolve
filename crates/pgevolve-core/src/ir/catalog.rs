//! `Catalog` — a complete schema snapshot.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::IrError;
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, prefix_differences};
use crate::ir::extension::Extension;
use crate::ir::function::Function;
use crate::ir::index::Index;
use crate::ir::procedure::Procedure;
use crate::ir::publication::Publication;
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
    /// Publications (logical-replication source-side metadata).
    pub publications: Vec<Publication>,
    /// Database-global event triggers (lenient drop policy).
    pub event_triggers: Vec<crate::ir::event_trigger::EventTrigger>,
    /// Multi-column statistics objects (CREATE STATISTICS).
    pub statistics: Vec<crate::ir::statistic::Statistic>,
    /// Subscriptions (logical-replication subscriber-side metadata).
    pub subscriptions: Vec<crate::ir::subscription::Subscription>,
    /// User-defined collations (v0.3.8+).
    pub collations: Vec<crate::ir::collation::Collation>,
    /// `ALTER DEFAULT PRIVILEGES` rules. Canonicalized.
    pub default_privileges: Vec<crate::ir::default_privileges::DefaultPrivilegeRule>,
    /// User-defined aggregates (`CREATE AGGREGATE`).
    pub aggregates: Vec<crate::ir::aggregate::Aggregate>,
    /// User-defined casts (`CREATE CAST`).
    pub casts: Vec<crate::ir::cast::Cast>,
    /// `TEXT SEARCH DICTIONARY` objects.
    pub ts_dictionaries: Vec<crate::ir::text_search::TsDictionary>,
    /// `TEXT SEARCH CONFIGURATION` objects.
    pub ts_configurations: Vec<crate::ir::text_search::TsConfiguration>,
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
            publications: Vec::new(),
            event_triggers: Vec::new(),
            statistics: Vec::new(),
            subscriptions: Vec::new(),
            collations: Vec::new(),
            default_privileges: Vec::new(),
            aggregates: Vec::new(),
            casts: Vec::new(),
            ts_dictionaries: Vec::new(),
            ts_configurations: Vec::new(),
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

impl Equiv for Catalog {
    #[allow(clippy::too_many_lines)] // one diff_keyed block per collection — extracting would obscure the table.
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a collection is
        // added to `Catalog` without being handled below. Every one of the
        // 21 collections is diffed; bindings are unused (read via `self`).
        let Self {
            schemas: _,
            extensions: _,
            tables: _,
            indexes: _,
            sequences: _,
            views: _,
            materialized_views: _,
            types: _,
            functions: _,
            procedures: _,
            triggers: _,
            publications: _,
            event_triggers: _,
            statistics: _,
            subscriptions: _,
            collations: _,
            default_privileges: _,
            aggregates: _,
            casts: _,
            ts_dictionaries: _,
            ts_configurations: _,
        } = self;
        let mut out = Vec::new();
        out.extend(prefix_differences(
            "schemas",
            diff_keyed(&self.schemas, &other.schemas, |s| s.name.to_string()),
        ));
        out.extend(prefix_differences(
            "extensions",
            diff_keyed(&self.extensions, &other.extensions, |e| e.name.to_string()),
        ));
        out.extend(prefix_differences(
            "tables",
            diff_keyed(&self.tables, &other.tables, |t| t.qname.to_string()),
        ));
        out.extend(prefix_differences(
            "indexes",
            diff_keyed(&self.indexes, &other.indexes, |i| i.qname.to_string()),
        ));
        out.extend(prefix_differences(
            "sequences",
            diff_keyed(&self.sequences, &other.sequences, |s| s.qname.to_string()),
        ));
        out.extend(prefix_differences(
            "views",
            diff_keyed(&self.views, &other.views, |v| v.qname.to_string()),
        ));
        out.extend(prefix_differences(
            "materialized_views",
            diff_keyed(&self.materialized_views, &other.materialized_views, |m| {
                m.qname.to_string()
            }),
        ));
        out.extend(prefix_differences(
            "types",
            diff_keyed(&self.types, &other.types, |t| t.qname.to_string()),
        ));
        out.extend(prefix_differences(
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
        out.extend(prefix_differences(
            "procedures",
            diff_keyed(&self.procedures, &other.procedures, |p| p.qname.to_string()),
        ));
        out.extend(prefix_differences(
            "triggers",
            diff_keyed(&self.triggers, &other.triggers, |t| t.qname.to_string()),
        ));
        out.extend(prefix_differences(
            "default_privileges",
            diff_keyed(&self.default_privileges, &other.default_privileges, |r| {
                format!(
                    "{}.{}.{}",
                    r.target_role,
                    r.schema.as_ref().map_or("*", |s| s.as_str()),
                    r.object_type.sql_keyword(),
                )
            }),
        ));
        // Publications: keyed by name (canon dedup key — global namespace).
        out.extend(prefix_differences(
            "publications",
            diff_keyed(&self.publications, &other.publications, |p| {
                p.name.to_string()
            }),
        ));
        // Subscriptions: keyed by name (canon dedup key — global namespace).
        out.extend(prefix_differences(
            "subscriptions",
            diff_keyed(&self.subscriptions, &other.subscriptions, |s| {
                s.name.to_string()
            }),
        ));
        // Statistics: keyed by qname (canon dedup key).
        out.extend(prefix_differences(
            "statistics",
            diff_keyed(&self.statistics, &other.statistics, |s| s.qname.to_string()),
        ));
        // Event triggers: keyed by name (canon dedup key — global namespace).
        out.extend(prefix_differences(
            "event_triggers",
            diff_keyed(&self.event_triggers, &other.event_triggers, |et| {
                et.name.to_string()
            }),
        ));
        // Collations: keyed by qname (canon dedup key).
        out.extend(prefix_differences(
            "collations",
            diff_keyed(&self.collations, &other.collations, |c| c.qname.to_string()),
        ));
        // Aggregates: keyed by (qname, arg_types) — overloadable; mirrors the
        // canon dedup identity `(qname, arg_types_key)`.
        out.extend(prefix_differences(
            "aggregates",
            diff_keyed(&self.aggregates, &other.aggregates, |a| {
                format!(
                    "{}({})",
                    a.qname,
                    a.arg_types
                        .iter()
                        .map(crate::ir::column_type::ColumnType::render_sql)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }),
        ));
        // Casts: keyed by (source, target) — canon dedup identity.
        out.extend(prefix_differences(
            "casts",
            diff_keyed(&self.casts, &other.casts, |c| {
                format!("{}->{}", c.source.render_sql(), c.target.render_sql())
            }),
        ));
        // Text-search dictionaries: keyed by qname (canon dedup key).
        out.extend(prefix_differences(
            "ts_dictionaries",
            diff_keyed(&self.ts_dictionaries, &other.ts_dictionaries, |d| {
                d.qname.to_string()
            }),
        ));
        // Text-search configurations: keyed by qname (canon dedup key).
        out.extend(prefix_differences(
            "ts_configurations",
            diff_keyed(&self.ts_configurations, &other.ts_configurations, |c| {
                c.qname.to_string()
            }),
        ));
        out
    }
}

fn diff_keyed<T: Equiv, K: Fn(&T) -> String>(lhs: &[T], rhs: &[T], key: K) -> Vec<Difference> {
    let mut out = Vec::new();
    let lhs_map: BTreeMap<String, &T> = lhs.iter().map(|x| (key(x), x)).collect();
    let rhs_map: BTreeMap<String, &T> = rhs.iter().map(|x| (key(x), x)).collect();
    for (k, l) in &lhs_map {
        match rhs_map.get(k) {
            None => out.push(Difference::new(k, "present", "removed")),
            Some(r) => out.extend(prefix_differences(k, l.differences(r))),
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
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
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
        let d = Catalog::empty().differences(&b);
        assert!(d.iter().any(|x| x.path.starts_with("tables.app.users")));
    }

    #[test]
    fn remove_table_reports() {
        let mut a = Catalog::empty();
        a.tables.push(table_users());
        let d = a.differences(&Catalog::empty());
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
        let d = a.differences(&b);
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
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
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
        assert!(matches!(
            r,
            Err(IrError::DuplicateObject { kind: "table", .. })
        ));
    }

    #[test]
    fn publication_change_diffs() {
        use crate::ir::publication::{Publication, PublicationScope, PublishKinds};
        let mut b = Catalog::empty();
        b.publications.push(Publication {
            name: id("p"),
            scope: PublicationScope::AllTables,
            publish: PublishKinds::pg_default(),
            publish_via_partition_root: false,
            owner: None,
            comment: None,
        });
        assert!(
            Catalog::empty()
                .differences(&b)
                .iter()
                .any(|x| x.path.starts_with("publications")),
            "adding a publication must be reported (was silently ignored before)",
        );
    }

    #[test]
    fn cast_change_diffs() {
        use crate::ir::cast::{Cast, CastContext, CastMethod};
        let mut b = Catalog::empty();
        b.casts.push(Cast {
            source: qn("src"),
            target: qn("tgt"),
            method: CastMethod::Binary,
            context: CastContext::Explicit,
            comment: None,
        });
        assert!(
            Catalog::empty()
                .differences(&b)
                .iter()
                .any(|x| x.path.starts_with("casts")),
            "adding a cast must be reported (was silently ignored before)",
        );
    }

    #[test]
    fn aggregate_overload_keyed_by_arg_types() {
        use crate::ir::aggregate::Aggregate;
        let agg = |args: Vec<ColumnType>| Aggregate {
            qname: qn("my_agg"),
            arg_types: args,
            state_type: ColumnType::BigInt,
            sfunc: qn("sfunc"),
            finalfunc: None,
            initcond: None,
            owner: None,
            comment: None,
        };
        let mut a = Catalog::empty();
        a.aggregates.push(agg(vec![ColumnType::Integer]));
        let mut b = Catalog::empty();
        b.aggregates.push(agg(vec![ColumnType::BigInt]));
        // Different arg types => distinct identities => present/removed + added.
        let d = a.differences(&b);
        assert!(
            d.iter().any(|x| x.path.starts_with("aggregates")),
            "differing aggregate overloads must be reported: {d:?}",
        );
    }

    #[test]
    fn default_privileges_change_diffs() {
        use crate::ir::default_privileges::{DefaultPrivObjectType, DefaultPrivilegeRule};
        let mut b = Catalog::empty();
        b.default_privileges.push(DefaultPrivilegeRule {
            target_role: id("app_owner"),
            schema: None,
            object_type: DefaultPrivObjectType::Tables,
            grants: vec![],
        });
        assert!(
            Catalog::empty()
                .differences(&b)
                .iter()
                .any(|x| x.path.starts_with("default_privileges"))
        );
    }
}
