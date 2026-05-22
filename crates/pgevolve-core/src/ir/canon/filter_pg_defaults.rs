//! Drop IR field values that match PG's documented defaults — turning
//! them into `None`.
//!
//! Why: PG stores explicit values for things the user often didn't
//! declare (e.g., `MINVALUE`/`MAXVALUE` derived from the sequence's
//! type; `COST 100` for SQL/PLpgSQL functions; the implicit
//! `pg_catalog.default` collation for every text column). The source
//! parser uses `None` to mean "no explicit clause." This pass
//! normalizes both sides so a function declared without `COST` and the
//! catalog reading of the same function are byte-equal.
//!
//! Rules are order-insensitive — each runs over disjoint IR fields.

use crate::ir::catalog::Catalog;
use crate::ir::column::StorageKind;
use crate::ir::column_type::ColumnType;
use crate::ir::function::Function;
use crate::ir::sequence::Sequence;

/// Run every default-elision rule.
pub fn run(cat: &mut Catalog) {
    for seq in &mut cat.sequences {
        normalize_sequence_defaults(seq);
    }
    for table in &mut cat.tables {
        for col in &mut table.columns {
            normalize_column_collation(col);
            normalize_column_storage(col);
        }
    }
    for f in &mut cat.functions {
        normalize_function_defaults(f);
    }
}

/// Normalize `min_value` / `max_value` to `None` when they equal the
/// PG-implied default for the sequence's `(data_type, increment)`.
fn normalize_sequence_defaults(seq: &mut Sequence) {
    let (default_min, default_max) = sequence_default_bounds(&seq.data_type, seq.increment);
    if seq.min_value == Some(default_min) {
        seq.min_value = None;
    }
    if seq.max_value == Some(default_max) {
        seq.max_value = None;
    }
}

/// PG's per-type defaults for `MINVALUE`/`MAXVALUE` when not explicitly
/// set. For ascending sequences (`increment > 0`), `MINVALUE` defaults
/// to `1` and `MAXVALUE` to the type's max. For descending sequences,
/// the roles flip.
fn sequence_default_bounds(ty: &ColumnType, increment: i64) -> (i64, i64) {
    let (ty_min, ty_max) = match ty {
        ColumnType::SmallInt => (i64::from(i16::MIN), i64::from(i16::MAX)),
        ColumnType::Integer => (i64::from(i32::MIN), i64::from(i32::MAX)),
        // BigInt or anything else we treat as bigint-shaped.
        _ => (i64::MIN, i64::MAX),
    };
    if increment >= 0 {
        (1, ty_max)
    } else {
        (ty_min, -1)
    }
}

/// PG defaults `procost = 100` for SQL/PLpgSQL functions and
/// `prorows = 1000` for SETOF (`0` otherwise). Source IR uses `None`
/// for the default in both cases; this pass aligns the catalog read.
fn normalize_function_defaults(f: &mut Function) {
    if let Some(v) = f.cost
        && (v - 100.0).abs() <= f32::EPSILON
    {
        f.cost = None;
    }
    if let Some(v) = f.rows
        && (v <= 0.0 || (v - 1000.0).abs() <= f32::EPSILON)
    {
        f.rows = None;
    }
}

/// Strip the implicit `pg_catalog.default` collation that PG attaches
/// to every text-typed column. Source IR uses `None` to mean "no
/// explicit COLLATE clause"; this pass aligns the catalog read.
fn normalize_column_collation(col: &mut crate::ir::column::Column) {
    if let Some(qname) = &col.collation
        && qname.schema.as_str() == "pg_catalog"
        && qname.name.as_str() == "default"
    {
        col.collation = None;
    }
}

/// Postgres's default `attstorage` for the column's type.
///
/// Derived from `pg_type.typstorage`. The mapping is stable across all
/// supported PG versions for built-in types. Adding a new `ColumnType`
/// variant requires extending this match — the compiler will catch it.
pub(crate) const fn type_default_storage(ty: &ColumnType) -> StorageKind {
    use crate::ir::column_type::NetAddressKind;
    match ty {
        // Fixed-width by-value types: typstorage = 'p' (PLAIN).
        // time/timestamp/interval are also fixed-width on disk (8/8/16 bytes).
        // macaddr/macaddr8 are fixed 6/8 bytes: typstorage = 'p'.
        ColumnType::Boolean
        | ColumnType::SmallInt
        | ColumnType::Integer
        | ColumnType::BigInt
        | ColumnType::Real
        | ColumnType::DoublePrecision
        | ColumnType::Date
        | ColumnType::Uuid
        | ColumnType::Time { .. }
        | ColumnType::Timestamp { .. }
        | ColumnType::Interval { .. }
        | ColumnType::NetAddress(NetAddressKind::MacAddr | NetAddressKind::MacAddr8) => {
            StorageKind::Plain
        }
        // numeric: typstorage = 'm' (MAIN).
        // inet/cidr: variable up to ~32 bytes, typstorage = 'm'.
        ColumnType::Numeric { .. }
        | ColumnType::NetAddress(NetAddressKind::Inet | NetAddressKind::Cidr) => StorageKind::Main,
        // bit/bit varying: typstorage = 'e' (EXTERNAL) — out-of-line allowed,
        // compression forbidden. Distinct from 'x' (EXTENDED) which allows both.
        ColumnType::Bit { .. } => StorageKind::External,
        // Variable-width/toastable types: typstorage = 'x' (EXTENDED).
        // text/varchar/char: character varying data.
        // bytea: binary data.
        // json/jsonb: JSON data.
        // arrays: always variable-length toastable.
        // User-defined and unknown types: conservatively EXTENDED; enums are
        // PLAIN but we cannot distinguish them here, so we do not strip.
        ColumnType::Text
        | ColumnType::Varchar { .. }
        | ColumnType::Char { .. }
        | ColumnType::Bytea
        | ColumnType::Json
        | ColumnType::Jsonb
        | ColumnType::Array { .. }
        | ColumnType::UserDefined(_)
        | ColumnType::Other { .. } => StorageKind::Extended,
    }
}

/// If `col.storage` equals the type default, strip it to `None`.
fn normalize_column_storage(col: &mut crate::ir::column::Column) {
    if let Some(s) = col.storage
        && s == type_default_storage(&col.ty)
    {
        col.storage = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column::Column;
    use crate::ir::table::Table;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn ascending_bigint_seq() -> Sequence {
        Sequence {
            qname: qn("app", "s"),
            data_type: ColumnType::BigInt,
            start: 1,
            increment: 1,
            min_value: Some(1),
            max_value: Some(i64::MAX),
            cache: 1,
            cycle: false,
            owned_by: None,
            comment: None,
        }
    }

    #[test]
    fn strips_pg_default_min_max_on_ascending_bigint() {
        let mut cat = Catalog::empty();
        cat.sequences.push(ascending_bigint_seq());
        run(&mut cat);
        let s = &cat.sequences[0];
        assert_eq!(s.min_value, None);
        assert_eq!(s.max_value, None);
    }

    #[test]
    fn keeps_explicit_non_default_min_max() {
        let mut cat = Catalog::empty();
        let mut s = ascending_bigint_seq();
        s.min_value = Some(5);
        s.max_value = Some(1000);
        cat.sequences.push(s);
        run(&mut cat);
        assert_eq!(cat.sequences[0].min_value, Some(5));
        assert_eq!(cat.sequences[0].max_value, Some(1000));
    }

    #[test]
    fn strips_pg_catalog_default_collation() {
        let mut cat = Catalog::empty();
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
                name: id("email"),
                ty: ColumnType::Text,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: Some(QualifiedName::new(id("pg_catalog"), id("default"))),
                storage: None,
                compression: None,
                comment: None,
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        run(&mut cat);
        assert_eq!(cat.tables[0].columns[0].collation, None);
    }

    #[test]
    fn keeps_explicit_collation() {
        let mut cat = Catalog::empty();
        let explicit = QualifiedName::new(id("pg_catalog"), id("ucs_basic"));
        cat.tables.push(Table {
            qname: qn("app", "users"),
            columns: vec![Column {
                name: id("email"),
                ty: ColumnType::Text,
                nullable: false,
                default: None,
                identity: None,
                generated: None,
                collation: Some(explicit.clone()),
                storage: None,
                compression: None,
                comment: None,
            }],
            constraints: vec![],
            partition_by: None,
            partition_of: None,
            comment: None,
        });
        run(&mut cat);
        assert_eq!(cat.tables[0].columns[0].collation, Some(explicit));
    }

    use crate::ir::function::{
        Function, FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType, SecurityMode,
        Volatility,
    };
    use crate::parse::normalize_body::NormalizedBody;

    fn sample_function() -> Function {
        Function {
            qname: qn("app", "f"),
            args: vec![],
            arg_types_normalized: NormalizedArgTypes::from_args(&[]),
            return_type: ReturnType::Void,
            language: FunctionLanguage::Sql,
            body: NormalizedBody::empty(),
            body_dependencies: vec![],
            volatility: Volatility::Volatile,
            strict: false,
            security: SecurityMode::Invoker,
            parallel: ParallelSafety::Unsafe,
            leakproof: false,
            cost: None,
            rows: None,
            comment: None,
        }
    }

    #[test]
    fn strips_pg_default_cost() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.cost = Some(100.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].cost, None);
    }

    #[test]
    fn keeps_non_default_cost() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.cost = Some(50.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].cost, Some(50.0));
    }

    #[test]
    fn strips_pg_default_rows_setof() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.rows = Some(1000.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].rows, None);
    }

    #[test]
    fn strips_pg_default_rows_zero() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.rows = Some(0.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].rows, None);
    }

    #[test]
    fn keeps_non_default_rows() {
        let mut cat = Catalog::empty();
        let mut f = sample_function();
        f.rows = Some(42.0);
        cat.functions.push(f);
        run(&mut cat);
        assert_eq!(cat.functions[0].rows, Some(42.0));
    }

    // ── normalize_column_storage tests ───────────────────────────────────────

    use crate::ir::column::{Compression, StorageKind};

    fn col(name: &str, ty: ColumnType) -> Column {
        Column {
            name: Identifier::from_unquoted(name).unwrap(),
            ty,
            nullable: true,
            default: None,
            identity: None,
            generated: None,
            collation: None,
            storage: None,
            compression: None,
            comment: None,
        }
    }

    #[test]
    fn type_default_storage_stripped() {
        let mut c = col("body", ColumnType::Text);
        c.storage = Some(StorageKind::Extended); // text default
        normalize_column_storage(&mut c);
        assert_eq!(c.storage, None, "EXTENDED on text should normalize to None");
    }

    #[test]
    fn non_default_storage_preserved() {
        let mut c = col("body", ColumnType::Text);
        c.storage = Some(StorageKind::External); // text default is EXTENDED
        normalize_column_storage(&mut c);
        assert_eq!(c.storage, Some(StorageKind::External));
    }

    #[test]
    fn type_default_for_int_is_plain() {
        let mut c = col("id", ColumnType::BigInt);
        c.storage = Some(StorageKind::Plain);
        normalize_column_storage(&mut c);
        assert_eq!(c.storage, None);
    }

    #[test]
    fn type_default_for_bit_is_external() {
        let mut c = col("flags", ColumnType::Bit { len: 8, varying: false });
        c.storage = Some(StorageKind::External);
        normalize_column_storage(&mut c);
        assert_eq!(c.storage, None, "External on bit should normalize to None (bit's typstorage is 'e' = External)");
    }

    #[test]
    fn compression_is_not_stripped_by_canon() {
        let mut c = col("body", ColumnType::Text);
        c.compression = Some(Compression::Pglz);
        normalize_column_storage(&mut c);
        assert_eq!(
            c.compression,
            Some(Compression::Pglz),
            "canon does not touch compression"
        );
    }
}
