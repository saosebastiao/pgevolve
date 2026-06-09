//! Drop IR field values that match PG's documented defaults — turning
//! them into `None`.
//!
//! Why: PG stores explicit values for things the user often didn't
//! declare (e.g., `MINVALUE`/`MAXVALUE` derived from the sequence's
//! type; `COST 100` for SQL/PLpgSQL functions; the implicit
//! `pg_catalog.default` collation for every text column; the
//! `<subtype>_ops` opclass that PG resolves when `SUBTYPE_OPCLASS` is
//! omitted from `CREATE TYPE … AS RANGE`). The source parser uses `None`
//! to mean "no explicit clause." This pass normalizes both sides so a
//! function declared without `COST` and the catalog reading of the same
//! function are byte-equal.
//!
//! Rules are order-insensitive — each runs over disjoint IR fields.

use crate::identifier::QualifiedName;
use crate::ir::catalog::Catalog;
use crate::ir::column::StorageKind;
use crate::ir::column_type::ColumnType;
use crate::ir::function::Function;
use crate::ir::sequence::Sequence;
use crate::ir::table::Table;
use crate::ir::user_type::{UserType, UserTypeKind};

/// Run every default-elision rule.
pub fn run(cat: &mut Catalog) {
    for seq in &mut cat.sequences {
        normalize_sequence_defaults(seq);
    }
    for table in &mut cat.tables {
        normalize_table_access_method(table);
        normalize_table_tablespace(table);
        for col in &mut table.columns {
            normalize_column_collation(col);
            normalize_column_storage(col);
        }
    }
    for f in &mut cat.functions {
        normalize_function_defaults(f);
    }
    for ut in &mut cat.types {
        normalize_range_defaults(ut);
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
///
/// Exposed as `pub` so that `pgevolve-testkit`'s arbitrary generators can
/// make type-aware decisions (e.g. only offer TOAST storage variants for
/// toastable types).
pub const fn type_default_storage(ty: &ColumnType) -> StorageKind {
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

/// `heap` is PG's default table access method; strip it so a `USING heap`
/// source matches a live default-heap table — no spurious diff.
fn normalize_table_access_method(table: &mut Table) {
    if table
        .access_method
        .as_ref()
        .is_some_and(|am| am.as_str() == "heap")
    {
        table.access_method = None;
    }
}

/// `pg_default` is PG's default tablespace; strip it so an explicit
/// `TABLESPACE pg_default` in source round-trips equal to the reader's `None`.
fn normalize_table_tablespace(table: &mut Table) {
    if table
        .tablespace
        .as_ref()
        .is_some_and(|ts| ts.as_str() == "pg_default")
    {
        table.tablespace = None;
    }
}

/// Strip `subtype_opclass` and `collation` on a Range type when they match
/// PG's resolved defaults.
///
/// When `CREATE TYPE … AS RANGE` omits `SUBTYPE_OPCLASS`, PG resolves the
/// default B-tree opclass for the subtype and stores it in `pg_range`. The
/// catalog reader therefore returns the resolved opclass; the source parser
/// leaves the field `None`. Keeping the resolved value in the IR creates a
/// spurious divergence during round-trip comparison.
///
/// Similarly, when `COLLATION` is omitted, PG records `pg_catalog.default`;
/// the source parser leaves the field `None`.
///
/// Canon rule:
/// * Strip `subtype_opclass` to `None` when it equals the subtype's
///   well-known default opclass (hard-coded table of PG built-in types).
/// * Strip `collation` to `None` when it equals `pg_catalog.default`.
///
/// Non-Range variants are left unchanged.
fn normalize_range_defaults(ut: &mut UserType) {
    let UserTypeKind::Range {
        subtype,
        subtype_opclass,
        collation,
        ..
    } = &mut ut.kind
    else {
        return;
    };

    // Strip collation when it equals the universal default.
    if let Some(col) = collation.as_ref()
        && col.schema.as_str() == "pg_catalog"
        && col.name.as_str() == "default"
    {
        *collation = None;
    }

    // Strip subtype_opclass when it matches the subtype's default B-tree opclass.
    if let Some(opc) = subtype_opclass.as_ref()
        && opc.schema.as_str() == "pg_catalog"
        && is_default_opclass_for_subtype(subtype, opc)
    {
        *subtype_opclass = None;
    }
}

/// Return `true` if `opclass` is the PG-resolved default B-tree opclass for
/// `subtype`.
///
/// The mapping is derived from `pg_opclass` for built-in types. Only types
/// that actually appear in `CREATE TYPE … AS RANGE` definitions in practice
/// are listed; the function is intentionally conservative (unknown subtype →
/// `false`, so the opclass is kept rather than incorrectly stripped).
fn is_default_opclass_for_subtype(subtype: &QualifiedName, opclass: &QualifiedName) -> bool {
    if subtype.schema.as_str() != "pg_catalog" {
        // User-defined subtypes have user-defined opclasses — never strip.
        return false;
    }
    let expected_ops = match subtype.name.as_str() {
        // Temporal types
        "timestamptz" | "timestamp with time zone" => "timestamptz_ops",
        "timestamp" | "timestamp without time zone" => "timestamp_ops",
        "date" => "date_ops",
        // Exact numeric
        "numeric" | "decimal" => "numeric_ops",
        // Integer types (all map to <typename>_ops under their canonical name)
        "int4" | "integer" | "int" => "int4_ops",
        "int8" | "bigint" => "int8_ops",
        "int2" | "smallint" => "int2_ops",
        // Floating point
        "float4" | "real" => "float4_ops",
        "float8" | "double precision" => "float8_ops",
        // Text-like — varchar inherits text's B-tree opclass.
        "text" | "varchar" | "character varying" => "text_ops",
        // Other common range subtypes
        "uuid" => "uuid_ops",
        "inet" => "inet_ops",
        _ => return false,
    };
    opclass.name.as_str() == expected_ops
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
            owner: None,
            grants: vec![],
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
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
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
            owner: None,
            grants: vec![],
            rls_enabled: false,
            rls_forced: false,
            policies: vec![],
            storage: crate::ir::reloptions::TableStorageOptions::default(),
            access_method: None,
            tablespace: None,
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
            owner: None,
            grants: vec![],
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
        let mut c = col(
            "flags",
            ColumnType::Bit {
                len: 8,
                varying: false,
            },
        );
        c.storage = Some(StorageKind::External);
        normalize_column_storage(&mut c);
        assert_eq!(
            c.storage, None,
            "External on bit should normalize to None (bit's typstorage is 'e' = External)"
        );
    }

    #[test]
    fn type_default_for_numeric_is_main() {
        let mut c = col("amount", ColumnType::Numeric { precision: None });
        c.storage = Some(StorageKind::Main);
        normalize_column_storage(&mut c);
        assert_eq!(
            c.storage, None,
            "Main on numeric should normalize to None (numeric's typstorage is 'm' = Main)"
        );
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

    // ── normalize_range_defaults tests ───────────────────────────────────────

    use crate::ir::user_type::{UserType, UserTypeKind};

    fn range_type(
        subtype_schema: &str,
        subtype_name: &str,
        subtype_opclass: Option<(&str, &str)>,
        collation: Option<(&str, &str)>,
    ) -> UserType {
        UserType {
            qname: qn("app", "myrange"),
            kind: UserTypeKind::Range {
                subtype: QualifiedName::new(id(subtype_schema), id(subtype_name)),
                subtype_opclass: subtype_opclass.map(|(s, n)| QualifiedName::new(id(s), id(n))),
                collation: collation.map(|(s, n)| QualifiedName::new(id(s), id(n))),
                canonical: None,
                subtype_diff: None,
                multirange_type_name: None,
            },
            comment: None,
            owner: None,
            grants: vec![],
        }
    }

    fn range_subtype_opclass(ut: &UserType) -> Option<String> {
        let UserTypeKind::Range {
            subtype_opclass, ..
        } = &ut.kind
        else {
            panic!("not a range");
        };
        subtype_opclass.as_ref().map(ToString::to_string)
    }

    fn range_collation(ut: &UserType) -> Option<String> {
        let UserTypeKind::Range { collation, .. } = &ut.kind else {
            panic!("not a range");
        };
        collation.as_ref().map(ToString::to_string)
    }

    /// Regression for issue #35.
    ///
    /// PG resolves `timestamptz_ops` as the default opclass for a `timestamptz`
    /// range when `SUBTYPE_OPCLASS` is omitted from `CREATE TYPE … AS RANGE`.
    /// The catalog reader returns the resolved opclass; the source parser leaves
    /// it `None`. Canon must strip the resolved value to `None` so both sides
    /// converge.
    #[test]
    fn strips_default_opclass_for_timestamptz() {
        let mut cat = Catalog::empty();
        cat.types.push(range_type(
            "pg_catalog",
            "timestamptz",
            Some(("pg_catalog", "timestamptz_ops")),
            None,
        ));
        run(&mut cat);
        assert_eq!(
            range_subtype_opclass(&cat.types[0]),
            None,
            "timestamptz_ops is the default for timestamptz — should be stripped"
        );
    }

    #[test]
    fn strips_default_opclass_for_int4() {
        let mut cat = Catalog::empty();
        cat.types.push(range_type(
            "pg_catalog",
            "int4",
            Some(("pg_catalog", "int4_ops")),
            None,
        ));
        run(&mut cat);
        assert_eq!(range_subtype_opclass(&cat.types[0]), None);
    }

    #[test]
    fn strips_default_opclass_for_text() {
        let mut cat = Catalog::empty();
        cat.types.push(range_type(
            "pg_catalog",
            "text",
            Some(("pg_catalog", "text_ops")),
            None,
        ));
        run(&mut cat);
        assert_eq!(range_subtype_opclass(&cat.types[0]), None);
    }

    #[test]
    fn strips_default_opclass_for_varchar_via_text_ops() {
        // varchar inherits text's B-tree opclass.
        let mut cat = Catalog::empty();
        cat.types.push(range_type(
            "pg_catalog",
            "varchar",
            Some(("pg_catalog", "text_ops")),
            None,
        ));
        run(&mut cat);
        assert_eq!(range_subtype_opclass(&cat.types[0]), None);
    }

    /// A non-default opclass (e.g. a custom one) must not be stripped.
    #[test]
    fn keeps_custom_opclass() {
        let mut cat = Catalog::empty();
        cat.types.push(range_type(
            "pg_catalog",
            "timestamptz",
            Some(("app", "custom_ops")),
            None,
        ));
        run(&mut cat);
        assert_eq!(
            range_subtype_opclass(&cat.types[0]),
            Some("app.custom_ops".into()),
            "custom opclass must be preserved"
        );
    }

    /// `subtype_opclass: None` must stay `None` (idempotent / no-op).
    #[test]
    fn already_none_opclass_unchanged() {
        let mut cat = Catalog::empty();
        cat.types
            .push(range_type("pg_catalog", "timestamptz", None, None));
        run(&mut cat);
        assert_eq!(range_subtype_opclass(&cat.types[0]), None);
    }

    /// A range with a user-defined subtype — opclass must not be stripped
    /// even if the name looks like a default opclass name.
    #[test]
    fn keeps_opclass_for_user_defined_subtype() {
        let mut cat = Catalog::empty();
        // Subtype lives in "app" schema, not "pg_catalog".
        cat.types.push(range_type(
            "app",
            "mytype",
            Some(("pg_catalog", "timestamptz_ops")),
            None,
        ));
        run(&mut cat);
        assert_eq!(
            range_subtype_opclass(&cat.types[0]),
            Some("pg_catalog.timestamptz_ops".into()),
            "opclass for user-defined subtype must be preserved"
        );
    }

    /// Regression for PG 18 round-trip: `pg_catalog.default` collation on a
    /// text range should be stripped to `None`.
    #[test]
    fn strips_pg_catalog_default_collation_on_range() {
        let mut cat = Catalog::empty();
        cat.types.push(range_type(
            "pg_catalog",
            "text",
            None,
            Some(("pg_catalog", "default")),
        ));
        run(&mut cat);
        assert_eq!(
            range_collation(&cat.types[0]),
            None,
            "pg_catalog.default collation must be stripped"
        );
    }

    /// A non-default explicit collation must be preserved.
    #[test]
    fn keeps_explicit_collation_on_range() {
        let mut cat = Catalog::empty();
        // "C" is lowercased to "c" by Identifier::from_unquoted (PG folds unquoted names).
        cat.types.push(range_type(
            "pg_catalog",
            "text",
            None,
            Some(("pg_catalog", "c")),
        ));
        run(&mut cat);
        assert_eq!(
            range_collation(&cat.types[0]),
            Some("pg_catalog.c".into()),
            "explicit non-default collation must be preserved"
        );
    }

    /// Non-range user types must not be touched.
    #[test]
    fn non_range_types_unaffected() {
        use crate::ir::user_type::EnumValue;
        let mut cat = Catalog::empty();
        cat.types.push(UserType {
            qname: qn("app", "status"),
            kind: UserTypeKind::Enum {
                values: vec![EnumValue {
                    name: "active".into(),
                    sort_order: 1.0,
                }],
            },
            comment: None,
            owner: None,
            grants: vec![],
        });
        let before = format!("{:?}", cat.types[0]);
        run(&mut cat);
        let after = format!("{:?}", cat.types[0]);
        assert_eq!(before, after, "non-range types must not be mutated");
    }

    // ── normalize_table_access_method / normalize_table_tablespace tests ────

    fn bare_table(name: &str, access_method: Option<Identifier>) -> Table {
        Table {
            qname: qn("app", name),
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
            access_method,
            tablespace: None,
        }
    }

    #[test]
    fn strips_heap_access_method() {
        let mut cat = Catalog::empty();
        cat.tables.push(bare_table("t_heap", Some(id("heap"))));
        cat.tables
            .push(bare_table("t_columnar", Some(id("columnar"))));
        run(&mut cat);
        assert_eq!(
            cat.tables[0].access_method, None,
            "`heap` is PG's default — should be stripped to None"
        );
        assert_eq!(
            cat.tables[1].access_method,
            Some(id("columnar")),
            "non-default access method must be preserved"
        );
    }

    #[test]
    fn strips_pg_default_tablespace() {
        let mut cat = Catalog::empty();
        let mut t = bare_table("t_pgdefault", None);
        t.tablespace = Some(id("pg_default"));
        cat.tables.push(t);
        run(&mut cat);
        assert_eq!(
            cat.tables[0].tablespace, None,
            "`pg_default` is PG's default tablespace — should be stripped to None"
        );
    }

    #[test]
    fn keeps_non_default_tablespace() {
        let mut cat = Catalog::empty();
        let mut t = bare_table("t_fast", None);
        t.tablespace = Some(id("fast"));
        cat.tables.push(t);
        run(&mut cat);
        assert_eq!(
            cat.tables[0].tablespace,
            Some(id("fast")),
            "non-default tablespace must be preserved"
        );
    }

    /// Run is idempotent: applying canon twice yields the same result.
    #[test]
    fn range_normalize_is_idempotent() {
        let mut cat = Catalog::empty();
        cat.types.push(range_type(
            "pg_catalog",
            "timestamptz",
            Some(("pg_catalog", "timestamptz_ops")),
            Some(("pg_catalog", "default")),
        ));
        run(&mut cat);
        let snap1 = format!("{:?}", cat.types[0]);
        run(&mut cat);
        let snap2 = format!("{:?}", cat.types[0]);
        assert_eq!(snap1, snap2, "normalize_range_defaults must be idempotent");
    }
}
