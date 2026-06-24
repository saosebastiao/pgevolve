//! Port of Postgres `ChooseRelationName` / `ChooseIndexName` from
//! `src/backend/commands/indexcmds.c`.
//!
//! Used by the LIKE-copy path to assign names to copied indexes and constraints
//! that match what a live Postgres server would assign.

use std::collections::BTreeSet;

use crate::identifier::Identifier;
use crate::ir::catalog::Catalog;

/// The kind of index/constraint being named, determining the label suffix.
#[derive(Clone, Copy)]
pub enum IndexNameKind {
    /// Primary-key constraint: suffix `pkey`.
    Pkey,
    /// Unique constraint: suffix `key`.
    Unique,
    /// Plain (non-unique, non-exclusion) index: suffix `idx`.
    Plain,
    /// Exclusion constraint: suffix `excl`.
    Exclude,
    /// Extended statistics object: suffix `stat`.
    // TODO(#43 Task 11): verify extended-statistics naming vs live PG
    Stat,
}

impl IndexNameKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Pkey => "pkey",
            Self::Unique => "key",
            Self::Plain => "idx",
            Self::Exclude => "excl",
            Self::Stat => "stat",
        }
    }
}

/// A set of names already taken in a schema (relation names, index names,
/// constraint names, statistics names).  Used to detect collisions when
/// choosing a new name.
#[derive(Default)]
pub struct TakenNames(BTreeSet<String>);

impl TakenNames {
    /// Seed a `TakenNames` from every name in `schema` that shares the
    /// `pg_class` relation namespace — tables, indexes, views, materialized
    /// views, sequences, plus per-table constraint names and statistics
    /// objects.  Postgres's `ChooseRelationName` checks the whole relation
    /// namespace, so we must seed all of these to match its collision
    /// behaviour.
    pub fn from_schema(catalog: &Catalog, schema: &Identifier) -> Self {
        let mut s = BTreeSet::new();
        for t in &catalog.tables {
            if &t.qname.schema == schema {
                s.insert(t.qname.name.as_str().to_string());
                for c in &t.constraints {
                    // Constraint qnames carry the owning table's schema, so this
                    // re-check is normally true; kept defensively in case a
                    // constraint's recorded schema ever diverges from its table's.
                    if &c.qname.schema == schema {
                        s.insert(c.qname.name.as_str().to_string());
                    }
                }
            }
        }
        for i in &catalog.indexes {
            if &i.qname.schema == schema {
                s.insert(i.qname.name.as_str().to_string());
            }
        }
        for v in &catalog.views {
            if &v.qname.schema == schema {
                s.insert(v.qname.name.as_str().to_string());
            }
        }
        for mv in &catalog.materialized_views {
            if &mv.qname.schema == schema {
                s.insert(mv.qname.name.as_str().to_string());
            }
        }
        for seq in &catalog.sequences {
            if &seq.qname.schema == schema {
                s.insert(seq.qname.name.as_str().to_string());
            }
        }
        for st in &catalog.statistics {
            if &st.qname.schema == schema {
                s.insert(st.qname.name.as_str().to_string());
            }
        }
        Self(s)
    }

    /// Insert a name as taken.
    pub fn insert(&mut self, name: &str) {
        self.0.insert(name.to_string());
    }

    fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

/// Maximum identifier length (NAMEDATALEN - 1).
const NAMEDATALEN: usize = 63;

/// Truncate `s` to at most `max` bytes, respecting UTF-8 char boundaries.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Maximum identifier length including the trailing NUL, i.e. Postgres's
/// `NAMEDATALEN` (64).  `ChooseIndexNameAddition` stops appending once the
/// running buffer reaches this length.
const NAMEDATALEN_FULL: usize = 64;

/// Port of `ChooseIndexNameAddition`: build a column-name fragment by joining
/// column names with `_`.  Postgres appends each name IN FULL, then breaks
/// only once the running buffer has reached `NAMEDATALEN` (64) — so the check
/// is post-append, not pre-check.  The first column is therefore always
/// included regardless of length.  Expression columns contribute `"expr"`,
/// `"expr2"`, `"expr3"`, … .
fn name_addition(col_names: &[Option<&str>]) -> String {
    let mut out = String::new();
    let mut expr_n: u32 = 0;
    for c in col_names {
        let part: String = c.map_or_else(
            || {
                expr_n += 1;
                if expr_n == 1 {
                    "expr".to_string()
                } else {
                    format!("expr{expr_n}")
                }
            },
            |name| (*name).to_string(),
        );
        if !out.is_empty() {
            out.push('_');
        }
        out.push_str(&part);
        // Append-then-break: Postgres includes the whole part, then stops once
        // the buffer reaches NAMEDATALEN. Columns after the one that crossed the
        // threshold are dropped (matches PG's `break`). Do NOT change this to a
        // `continue` to fit a later, shorter column — that would diverge from
        // the names a live server assigns.
        if out.len() >= NAMEDATALEN_FULL {
            break;
        }
    }
    out
}

/// Port of `ChooseRelationName`: build `name1[_name2]_label`, appending a
/// decimal counter (`label1`, `label2`, …) while the candidate is already
/// in `taken`.  Each component is truncated so the whole name fits within
/// `NAMEDATALEN` bytes.
fn choose_relation_name(name1: &str, name2: &str, label: &str, taken: &TakenNames) -> String {
    let build = |suffix: &str| {
        let label_full = format!("{label}{suffix}");
        // overhead = '_' + optional 'name2_' + label_full
        let overhead = 1 + (if name2.is_empty() { 0 } else { name2.len() + 1 }) + label_full.len();
        let budget1 = NAMEDATALEN.saturating_sub(overhead);
        let n1 = truncate_bytes(name1, budget1);
        if name2.is_empty() {
            format!("{n1}_{label_full}")
        } else {
            format!("{n1}_{name2}_{label_full}")
        }
    };

    let mut candidate = build("");
    let mut i: u32 = 0;
    while taken.contains(&candidate) {
        i += 1;
        candidate = build(&i.to_string());
    }
    candidate
}

/// Choose a name for a copied index or constraint, mirroring what a live
/// Postgres server assigns via `ChooseIndexName`.
///
/// The chosen name is inserted into `taken` before returning, so successive
/// calls will not produce duplicates.
pub fn choose_index_name(
    table: &str,
    col_names: &[Option<&str>],
    kind: IndexNameKind,
    taken: &mut TakenNames,
) -> String {
    let name = match kind {
        IndexNameKind::Pkey => choose_relation_name(table, "", kind.label(), taken),
        _ => choose_relation_name(table, &name_addition(col_names), kind.label(), taken),
    };
    taken.insert(&name);
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> TakenNames {
        let mut t = TakenNames::default();
        for n in names {
            t.insert(n);
        }
        t
    }

    #[test]
    fn pkey_name() {
        let mut t = TakenNames::default();
        assert_eq!(
            choose_index_name("clone", &[], IndexNameKind::Pkey, &mut t),
            "clone_pkey"
        );
    }

    #[test]
    fn unique_two_columns() {
        let mut t = TakenNames::default();
        assert_eq!(
            choose_index_name(
                "clone",
                &[Some("a"), Some("b")],
                IndexNameKind::Unique,
                &mut t
            ),
            "clone_a_b_key"
        );
    }

    #[test]
    fn plain_index_suffix_idx() {
        let mut t = TakenNames::default();
        assert_eq!(
            choose_index_name("clone", &[Some("a")], IndexNameKind::Plain, &mut t),
            "clone_a_idx"
        );
    }

    #[test]
    fn collision_appends_counter() {
        let mut t = taken(&["clone_a_key"]);
        assert_eq!(
            choose_index_name("clone", &[Some("a")], IndexNameKind::Unique, &mut t),
            "clone_a_key1"
        );
    }

    #[test]
    fn expression_columns_use_expr() {
        let mut t = TakenNames::default();
        assert_eq!(
            choose_index_name("clone", &[None, Some("a")], IndexNameKind::Plain, &mut t),
            "clone_expr_a_idx"
        );
    }

    #[test]
    fn name_addition_includes_multiple_long_columns() {
        // Two 25-char columns must BOTH be included: Postgres appends each in
        // full and only stops once the running buffer reaches NAMEDATALEN (64).
        // `t_<c1>_<c2>_idx` is 57 chars, well under the outer 63-byte truncation.
        let c1 = "abcdefghijklmnopqrstuvwxy";
        let c2 = "zyxwvutsrqponmlkjihgfedcb";
        let mut t = TakenNames::default();
        assert_eq!(
            choose_index_name("t", &[Some(c1), Some(c2)], IndexNameKind::Plain, &mut t),
            format!("t_{c1}_{c2}_idx")
        );
    }

    #[test]
    fn stat_name() {
        let mut t = TakenNames::default();
        assert_eq!(
            choose_index_name("c", &[Some("a"), Some("b")], IndexNameKind::Stat, &mut t),
            "c_a_b_stat"
        );
    }

    /// `{table}_{col}_idx` that would exceed 63 bytes must be truncated so the
    /// total name is ≤ 63 bytes.  The expected string is pinned here to lock
    /// pgevolve's NAMEDATALEN truncation behaviour; live-PG byte-fidelity is
    /// verified separately in CI (#46).
    ///
    /// With table = "a"×40, col = "b"×40:
    ///   overhead = 1("_") + 40("b"×40) + 1("_") + 3("idx") = 45
    ///   budget1  = 63 - 45 = 18  → name1 = "a"×18
    ///   result   = "a"×18 + "_" + "b"×40 + "_idx" = 63 bytes
    #[test]
    fn long_name_truncates_to_namedatalen() {
        let table = "a".repeat(40);
        let col = "b".repeat(40);
        let mut t = TakenNames::default();
        let name = choose_index_name(&table, &[Some(&col)], IndexNameKind::Plain, &mut t);
        // Must fit within NAMEDATALEN.
        assert!(
            name.len() <= NAMEDATALEN,
            "name length {} > {NAMEDATALEN}: {name:?}",
            name.len()
        );
        // Must be a valid UTF-8 prefix (char-boundary).
        assert!(
            name.starts_with('a'),
            "must start with the table name prefix"
        );
        assert!(name.ends_with("_idx"), "must end with _idx suffix");
        // Pin the exact string produced by the implementation.
        let expected = format!("{}_{}_{}", "a".repeat(18), "b".repeat(40), "idx");
        assert_eq!(name, expected, "truncated name did not match expected");
    }

    /// When the truncated name is already taken, a counter is appended and the
    /// result must still fit within NAMEDATALEN.
    ///
    /// With table = "a"×40, col = "b"×40, counter "1":
    ///   overhead = 1("_") + 40("b"×40) + 1("_") + 4("idx1") = 46
    ///   budget1  = 63 - 46 = 17  → name1 = "a"×17
    ///   result   = "a"×17 + "_" + "b"×40 + "_idx1" = 63 bytes
    #[test]
    fn truncated_collision_appends_counter() {
        let table = "a".repeat(40);
        let col = "b".repeat(40);
        // Seed taken with the first (truncated) name so the counter path is taken.
        let first_name = format!("{}_{}_{}", "a".repeat(18), "b".repeat(40), "idx");
        let mut t = taken(&[&first_name]);
        let name = choose_index_name(&table, &[Some(&col)], IndexNameKind::Plain, &mut t);
        assert!(
            name.len() <= NAMEDATALEN,
            "counter name length {} > {NAMEDATALEN}: {name:?}",
            name.len()
        );
        let expected = format!("{}_{}_{}", "a".repeat(17), "b".repeat(40), "idx1");
        assert_eq!(name, expected, "counter name did not match expected");
    }

    #[test]
    fn from_schema_seeds_relation_namespace() {
        // A sequence named `foo_a_key` occupies the relation namespace in schema
        // `pub`, so a freshly-chosen `foo_a_key` must collide and bump to
        // `foo_a_key1` — proving from_schema seeds non-index/non-table relations
        // (views, materialized views, sequences) too.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("pub")).expect("mkdir");
        std::fs::write(dir.path().join("pub/_schema.sql"), "CREATE SCHEMA pub;\n")
            .expect("write schema");
        std::fs::write(
            dir.path().join("pub/seq.sql"),
            "CREATE SEQUENCE pub.foo_a_key;\n",
        )
        .expect("write seq");
        let (cat, _) =
            crate::parse::parse_directory_with_locations(dir.path(), &[]).expect("parse");
        let schema = Identifier::from_unquoted("pub").expect("ident");
        let mut taken = TakenNames::from_schema(&cat, &schema);
        assert_eq!(
            choose_index_name("foo", &[Some("a")], IndexNameKind::Unique, &mut taken),
            "foo_a_key1"
        );
    }
}
