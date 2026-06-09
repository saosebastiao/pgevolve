//! Publication IR — declarative logical-replication source-side metadata.
//!
//! A `Publication` is a Postgres `CREATE PUBLICATION` object. It lives at
//! the Catalog top level (not schema-qualified) because Postgres treats
//! publications as a per-database global namespace.
//!
//! Spec: `docs/superpowers/specs/2026-05-26-publications-design.md`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::difference::Difference;
use crate::ir::eq::{Equiv, field_difference};

/// Declarative model of a Postgres `PUBLICATION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publication {
    /// Publication name (not schema-qualified — publications are global).
    pub name: Identifier,
    /// Which tables / schemas are published.
    pub scope: PublicationScope,
    /// Which DML kinds are replicated.
    pub publish: PublishKinds,
    /// Whether `INSERT`/`UPDATE`/`DELETE` on partition children should be
    /// reported using the partition root's identity (PG 13+).
    pub publish_via_partition_root: bool,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER PUBLICATION ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Optional comment.
    pub comment: Option<String>,
}

impl Equiv for Publication {
    fn differences(&self, other: &Self) -> Vec<Difference> {
        // Field-completeness guard: the compiler errors if a field is added
        // without being handled below. Bindings are unused (read via `self`).
        let Self {
            name: _,
            scope: _,
            publish: _,
            publish_via_partition_root: _,
            owner: _,
            comment: _,
        } = self;
        let mut out = Vec::new();
        out.extend(field_difference("name", &self.name, &other.name));
        out.extend(field_difference(
            "scope",
            &format!("{:?}", self.scope),
            &format!("{:?}", other.scope),
        ));
        out.extend(field_difference(
            "publish",
            &format!("{:?}", self.publish),
            &format!("{:?}", other.publish),
        ));
        out.extend(field_difference(
            "publish_via_partition_root",
            &format!("{:?}", self.publish_via_partition_root),
            &format!("{:?}", other.publish_via_partition_root),
        ));
        out.extend(field_difference(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(field_difference(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out
    }
}

/// Target set of a publication. Encodes PG's mutual exclusion of
/// `FOR ALL TABLES` and the selective forms at the type level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicationScope {
    /// `CREATE PUBLICATION p FOR ALL TABLES`. Implicitly captures every
    /// current and future table in the database.
    AllTables,
    /// `CREATE PUBLICATION p FOR TABLE ..., TABLES IN SCHEMA ...`.
    /// Either list may be empty (but not both — canon rejects empty
    /// Selective). Schema-scope is PG 15+ only.
    Selective {
        /// Schemas published in their entirety. PG 15+.
        schemas: BTreeSet<Identifier>,
        /// Per-table publication entries with optional row filter and
        /// column list. Sorted by `qname` after canon.
        tables: Vec<PublishedTable>,
    },
}

/// A single table entry inside `PublicationScope::Selective`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedTable {
    /// Schema-qualified table name.
    pub qname: QualifiedName,
    /// Optional `WHERE` row filter (PG 15+). Canonicalized via
    /// `NormalizedExpr`.
    pub row_filter: Option<NormalizedExpr>,
    /// Optional explicit column list (PG 15+). Sorted by name after canon.
    /// `None` = all columns; `Some(empty)` is rejected by canon.
    pub columns: Option<Vec<Identifier>>,
}

/// Which DML kinds a publication replicates. Maps to PG's four
/// `pg_publication.pub{insert,update,delete,truncate}` booleans, and the
/// source SQL `publish = 'insert, update, delete, truncate'` parameter.
///
/// The four `bool` fields directly model the four PG catalog columns; a
/// bitset enum would be less readable and harder to (de)serialize from
/// `pg_publication`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishKinds {
    /// `INSERT` is replicated.
    pub insert: bool,
    /// `UPDATE` is replicated.
    pub update: bool,
    /// `DELETE` is replicated.
    pub delete: bool,
    /// `TRUNCATE` is replicated.
    pub truncate: bool,
}

impl PublishKinds {
    /// PG's `CREATE PUBLICATION` default when `publish` is unspecified:
    /// all four DML kinds enabled.
    #[must_use]
    pub const fn pg_default() -> Self {
        Self {
            insert: true,
            update: true,
            delete: true,
            truncate: true,
        }
    }

    /// True iff at least one DML kind is enabled. An empty bitset is
    /// illegal at the IR level (canon rejects).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.insert && !self.update && !self.delete && !self.truncate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn publish_kinds_default_all_true() {
        let k = PublishKinds::pg_default();
        assert!(k.insert && k.update && k.delete && k.truncate);
        assert!(!k.is_empty());
    }

    #[test]
    fn publish_kinds_is_empty_when_all_false() {
        let k = PublishKinds {
            insert: false,
            update: false,
            delete: false,
            truncate: false,
        };
        assert!(k.is_empty());
    }

    #[test]
    fn scope_all_tables_does_not_equal_empty_selective() {
        let a = PublicationScope::AllTables;
        let b = PublicationScope::Selective {
            schemas: BTreeSet::new(),
            tables: Vec::new(),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn selective_with_a_schema_equals_itself() {
        let s = PublicationScope::Selective {
            schemas: BTreeSet::from([id("app")]),
            tables: Vec::new(),
        };
        assert_eq!(s.clone(), s);
    }
}
