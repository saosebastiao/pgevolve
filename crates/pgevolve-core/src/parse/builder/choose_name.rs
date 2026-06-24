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
    /// Extended statistics object: suffix `stat`. Verified to match PG's
    /// `ChooseExtendedStatisticName` across PG 14–18 by the live round-trip
    /// test `tests/table_like_round_trip.rs::like_statistics_name_matches_live_pg`.
    Stat,
    /// CHECK constraint: suffix `check`.
    Check,
}

impl IndexNameKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Pkey => "pkey",
            Self::Unique => "key",
            Self::Plain => "idx",
            Self::Exclude => "excl",
            Self::Stat => "stat",
            Self::Check => "check",
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

/// Port of Postgres's `makeObjectName`: given `name1`, `name2`, and
/// `label_full` (label + collision suffix), compute the truncated name that
/// fits within `NAMEDATALEN` bytes.
///
/// The algorithm mirrors PG's alternating shrink: shrink whichever of
/// `name1chars` / `name2chars` is currently longer (ties go to `name2`) until
/// the sum fits within `availchars`.  This differs from the old approach that
/// only shrank `name1`, which produced names longer than 63 bytes when `name2`
/// alone was very long.
///
/// `label_full` is the complete label string (base label + any collision
/// counter suffix) that PG calls "label" internally.
fn make_object_name<'a>(name1: &'a str, name2: &'a str, label_full: &str) -> String {
    // Separators: '_' before label_full always; '_' between name1 and name2
    // only when name2 is non-empty.
    let overhead = usize::from(!name2.is_empty()) // '_' before name2 (0 when name2 empty)
        + label_full.len()
        + 1; // '_' before label_full
    let availchars = NAMEDATALEN.saturating_sub(overhead);

    let mut name1chars = name1.len();
    let mut name2chars = name2.len();

    // Alternating shrink: reduce whichever side is longer (ties → name2).
    while name1chars + name2chars > availchars {
        if name1chars > name2chars {
            name1chars -= 1;
        } else {
            name2chars -= 1;
        }
    }

    let n1 = truncate_bytes(name1, name1chars);
    if name2.is_empty() {
        format!("{n1}_{label_full}")
    } else {
        let n2 = truncate_bytes(name2, name2chars);
        format!("{n1}_{n2}_{label_full}")
    }
}

/// Port of `ChooseRelationName`: build `name1[_name2]_label`, appending a
/// decimal counter (`label1`, `label2`, …) while the candidate is already
/// in `taken`.  Each component is truncated so the whole name fits within
/// `NAMEDATALEN` bytes using Postgres's alternating-shrink algorithm
/// (`makeObjectName`): whichever of name1/name2 is longer is trimmed first
/// (ties go to name2) until both fit.
fn choose_relation_name(name1: &str, name2: &str, label: &str, taken: &TakenNames) -> String {
    let build = |suffix: &str| {
        let label_full = format!("{label}{suffix}");
        make_object_name(name1, name2, &label_full)
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
    /// pgevolve's NAMEDATALEN truncation behaviour using PG's alternating-shrink
    /// `makeObjectName` algorithm; live-PG byte-fidelity is verified separately
    /// in CI (#46).
    ///
    /// With table = "a"×40, col = "b"×40:
    ///   overhead   = 1("_" sep) + 1("_" before label) + 3("idx") = 5
    ///   availchars = 63 - 5 = 58
    ///   n1=40, n2=40; sum=80; must shrink by 22.
    ///   Ties go to n2 (else branch in PG); 11 shrinks each → n1=29, n2=29.
    ///   result = "a"×29 + "_" + "b"×29 + "_idx" = 63 bytes.
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
        let expected = format!("{}_{}_{}", "a".repeat(29), "b".repeat(29), "idx");
        assert_eq!(name, expected, "truncated name did not match expected");
    }

    /// When the truncated name is already taken, a counter is appended and the
    /// result must still fit within NAMEDATALEN.
    ///
    /// With table = "a"×40, col = "b"×40, counter "1":
    ///   overhead   = 1 + 1 + 4("idx1") = 6
    ///   availchars = 63 - 6 = 57
    ///   n1=40, n2=40; sum=80; must shrink by 23.
    ///   Alternating (tie → n2): 11 pairs → n1=29, n2=29 (sum=58 > 57);
    ///   one more tie-break → n2=28.  Final: n1=29, n2=28.
    ///   result = "a"×29 + "_" + "b"×28 + "_idx1" = 63 bytes.
    #[test]
    fn truncated_collision_appends_counter() {
        let table = "a".repeat(40);
        let col = "b".repeat(40);
        // Seed taken with the first (truncated) name so the counter path is taken.
        let first_name = format!("{}_{}_{}", "a".repeat(29), "b".repeat(29), "idx");
        let mut t = taken(&[&first_name]);
        let name = choose_index_name(&table, &[Some(&col)], IndexNameKind::Plain, &mut t);
        assert!(
            name.len() <= NAMEDATALEN,
            "counter name length {} > {NAMEDATALEN}: {name:?}",
            name.len()
        );
        let expected = format!("{}_{}_{}", "a".repeat(29), "b".repeat(28), "idx1");
        assert_eq!(name, expected, "counter name did not match expected");
    }

    /// When `name2` (column addition) is long enough that truncating `name1` to
    /// zero would still leave the combined name over budget, PG's
    /// `makeObjectName` also truncates `name2`.  The alternating-shrink
    /// algorithm shrinks whichever of name1/name2 is currently longer until
    /// both fit.
    ///
    /// Byte math:
    ///   name1 = "t" (1 byte), name2 = "c"×70 (70 bytes), label = "idx"
    ///   overhead  = 1("_" sep) + 1("_" before label) + 3("idx") = 5
    ///   availchars = 63 - 5 = 58
    ///   n1=1, n2=70; sum=71 > 58; must shrink by 13.
    ///   Since n2 (70) > n1 (1) on every step, all 13 reductions go to n2.
    ///   Final: n1=1, n2=57 → "t" + "_" + "c"×57 + "_idx" = 63 bytes.
    #[test]
    fn long_column_addition_truncates_name2() {
        let table = "t";
        let col = "c".repeat(70);
        let mut t = TakenNames::default();
        let name = choose_index_name(table, &[Some(&col)], IndexNameKind::Plain, &mut t);
        assert!(
            name.len() <= NAMEDATALEN,
            "name length {} > {NAMEDATALEN}: {name:?}",
            name.len()
        );
        // name2 must have been truncated (col was 70 bytes, only 57 should appear).
        let expected = format!("t_{}_idx", "c".repeat(57));
        assert_eq!(
            name, expected,
            "alternating-shrink on long name2 did not match expected"
        );
    }

    /// When both name1 and name2 are long (~40 bytes each), PG's alternating
    /// shrink trims each by roughly equal amounts.
    ///
    /// Byte math:
    ///   name1 = "a"×40, name2 = "b"×40, label = "idx"
    ///   overhead  = 1 + 1 + 3 = 5
    ///   availchars = 63 - 5 = 58
    ///   n1=40, n2=40; sum=80; must shrink by 22.
    ///   Tie ⇒ shrink n2 first (else branch in PG), then n1 alternately.
    ///   After 22 steps (11 to n2, 11 to n1): n1=29, n2=29.
    ///   Result = "a"×29 + "_" + "b"×29 + "_idx" = 63 bytes.
    #[test]
    fn both_long_alternating_truncation() {
        let table = "a".repeat(40);
        let col = "b".repeat(40);
        let mut t = TakenNames::default();
        let name = choose_index_name(&table, &[Some(&col)], IndexNameKind::Plain, &mut t);
        assert!(
            name.len() <= NAMEDATALEN,
            "name length {} > {NAMEDATALEN}: {name:?}",
            name.len()
        );
        let expected = format!("{}_{}_{}", "a".repeat(29), "b".repeat(29), "idx");
        assert_eq!(
            name, expected,
            "both-long alternating truncation did not match expected"
        );
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
