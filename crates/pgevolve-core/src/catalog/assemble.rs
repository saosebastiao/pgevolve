//! Convert raw catalog rows into [`Catalog`] IR.
//!
//! This is the heart of the catalog reader: it stitches together rows from
//! [`crate::catalog::CatalogQuery`] into the same IR shape the source-side
//! parser produces.
//!
//! The strategy:
//! - Schemas, tables, sequences, columns: direct field-for-field translation.
//! - Indexes: re-parse `pg_get_indexdef` text via `pg_query` and reuse
//!   [`crate::parse::builder::index_stmt::build_index`].
//! - Constraints: build PK/UNIQUE/FK from row fields; for CHECK, extract the
//!   expression from `pg_get_constraintdef` text.
//! - Default expressions: parse the `pg_get_expr` text and run it through
//!   the same default-expr builder the source parser uses.
//! - SERIAL/IDENTITY ownership: walk the dependencies rows and populate
//!   `Sequence.owned_by` and the column-side `Identity`/`Default::Sequence`
//!   linkage so source-IR and catalog-IR converge on the same shape.

use std::collections::HashMap;
use std::path::PathBuf;

use pg_query::NodeEnum;

use crate::catalog::CatalogQuery;
use crate::catalog::DriftReport;
use crate::catalog::error::CatalogError;
use crate::catalog::filter::CatalogFilter;
use crate::catalog::rows::Row;
use crate::catalog::version::PgVersion;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::ir::column::{
    Column, Generated, GeneratedKind, Identity, IdentityKind, SequenceOptions,
};
use crate::ir::column_type::ColumnType;
use crate::ir::constraint::{
    Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
};
use crate::ir::default_expr::{DefaultExpr, NormalizedExpr};
use crate::ir::index::{Index, IndexParent};
use crate::ir::schema::Schema;
use crate::ir::sequence::{Sequence, SequenceOwner};
use crate::ir::table::Table;
use crate::ir::view::{MaterializedView, View, ViewColumn};
use crate::parse::builder;
use crate::parse::error::SourceLocation;
use crate::parse::normalize_body::NormalizedBody;

/// Bundle of rows passed to [`assemble`].
pub struct RawRows {
    pub version: PgVersion,
    pub schemas: Vec<Row>,
    pub tables: Vec<Row>,
    pub columns: Vec<Row>,
    pub constraints: Vec<Row>,
    pub indexes: Vec<Row>,
    pub sequences: Vec<Row>,
    pub dependencies: Vec<Row>,
    pub views_and_mvs: Vec<Row>,
    pub view_columns: Vec<Row>,
}

/// Convert raw rows into a [`Catalog`] and a [`DriftReport`]. Caller is
/// responsible for canonicalization.
pub fn assemble(
    raw: RawRows,
    filter: &CatalogFilter,
) -> Result<(Catalog, DriftReport), CatalogError> {
    let RawRows {
        version,
        schemas,
        tables,
        columns,
        constraints,
        indexes,
        sequences,
        dependencies,
        views_and_mvs,
        view_columns,
    } = raw;

    let mut catalog = Catalog::empty();
    let mut drift = DriftReport::default();

    catalog.schemas = build_schemas(&schemas, filter)?;

    // Attach constraints to their tables. Also collect drift from NOT VALID constraints.
    let mut tables_mut: HashMap<i64, Table> = build_tables(tables, &columns, filter)?;
    apply_constraints(&mut tables_mut, &constraints, filter, &mut drift)?;

    // Build indexes (re-parsing pg_get_indexdef). Also collect drift from INVALID indexes.
    catalog.indexes = build_indexes(&indexes, version, filter, &mut drift)?;

    // Build sequences.
    let mut sequence_by_qname: HashMap<String, Sequence> = HashMap::new();
    for r in &sequences {
        if let Some(s) = build_sequence(r, filter)? {
            sequence_by_qname.insert(s.qname.to_string(), s);
        }
    }

    // Wire SERIAL/IDENTITY ownership: link sequences to their owning columns,
    // and rewrite the column default to `DefaultExpr::Sequence(seq.qname)`
    // whenever the introspected default points at the owned sequence.
    apply_dependencies(&dependencies, &mut tables_mut, &mut sequence_by_qname)?;

    catalog.tables = tables_mut.into_values().collect();
    catalog.sequences = sequence_by_qname.into_values().collect();

    // Build views and materialized views.
    let (views, materialized_views) = build_views_and_mvs(&views_and_mvs, &view_columns, filter)?;
    catalog.views = views;
    catalog.materialized_views = materialized_views;

    Ok((catalog, drift))
}

fn build_schemas(rows: &[Row], filter: &CatalogFilter) -> Result<Vec<Schema>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let name = Identifier::from_unquoted(&r.get_text(CatalogQuery::Schemas, "name")?)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        if !filter.includes_schema(&name) {
            continue;
        }
        out.push(Schema {
            name,
            comment: r.get_opt_text(CatalogQuery::Schemas, "comment")?,
        });
    }
    Ok(out)
}

fn build_tables(
    table_rows: Vec<Row>,
    column_rows: &[Row],
    filter: &CatalogFilter,
) -> Result<HashMap<i64, Table>, CatalogError> {
    let mut tables: HashMap<i64, Table> = HashMap::with_capacity(table_rows.len());

    for r in table_rows {
        let oid = r.get_int(CatalogQuery::Tables, "oid")?;
        let qname = qname_from(&r, CatalogQuery::Tables, "schema", "name")?;
        if !filter.allows(&qname) {
            continue;
        }
        let comment = r.get_opt_text(CatalogQuery::Tables, "comment")?;
        tables.insert(
            oid,
            Table {
                qname,
                columns: vec![],
                constraints: vec![],
                comment,
            },
        );
    }

    // Attach columns by oid, in attnum order. Column rows are already ordered
    // by (schema, table, attnum) in the SQL.
    for cr in column_rows {
        let table_oid = cr.get_int(CatalogQuery::Columns, "table_oid")?;
        let Some(table) = tables.get_mut(&table_oid) else {
            continue;
        };
        let column = build_column(cr)?;
        table.columns.push(column);
    }

    Ok(tables)
}

fn build_column(r: &Row) -> Result<Column, CatalogError> {
    let q = CatalogQuery::Columns;
    let name = ident_required(&r.get_text(q, "name")?)?;
    let pg_ty = r.get_text(q, "pg_type_string")?;
    let ty = ColumnType::parse_from_pg_type_string(&pg_ty).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
            "{pg_ty}: {e}"
        )))
    })?;
    let not_null = r.get_bool(q, "not_null")?;

    let attidentity = r.get_opt_text(q, "attidentity")?.unwrap_or_default();
    let attgenerated = r.get_opt_text(q, "attgenerated")?.unwrap_or_default();

    let identity = if attidentity.is_empty() {
        None
    } else {
        Some(Identity {
            kind: match attidentity.as_str() {
                "a" => IdentityKind::Always,
                _ => IdentityKind::ByDefault,
            },
            sequence: SequenceOptions {
                start: r.get_int(q, "identity_start")?,
                increment: r.get_int(q, "identity_increment")?,
                min_value: r.get_opt_int(q, "identity_min")?,
                max_value: r.get_opt_int(q, "identity_max")?,
                cache: r.get_int(q, "identity_cache")?,
                cycle: r.get_bool(q, "identity_cycle").unwrap_or(false),
            },
        })
    };

    let default_text = r.get_opt_text(q, "default_expr")?;
    let default = if attgenerated == "s" {
        // Generated columns also carry their expression in `default_expr`; we'll
        // materialize the `Generated` instead.
        None
    } else if let Some(text) = &default_text {
        Some(parse_default_expr_text(text, &ty)?)
    } else {
        None
    };

    let generated =
        if attgenerated == "s" {
            let text = default_text.as_deref().ok_or(CatalogError::Ir(
                crate::ir::IrError::MissingField("generated column missing expression"),
            ))?;
            let expr = reparse_expression_text(text)?;
            Some(Generated {
                kind: GeneratedKind::Stored,
                expression: expr,
            })
        } else {
            None
        };

    let collation = match (
        r.get_opt_text(q, "collation_schema")?,
        r.get_opt_text(q, "collation_name")?,
    ) {
        // `pg_catalog.default` is PG's implicit collation for every
        // text-typed column. Treat it as "no explicit collation" so that
        // the IR doesn't gain a phantom collation that nobody declared.
        (Some(s), Some(n)) if s == "pg_catalog" && n == "default" => None,
        (Some(s), Some(n)) => Some(QualifiedName::new(ident_required(&s)?, ident_required(&n)?)),
        _ => None,
    };

    let comment = r.get_opt_text(q, "comment")?;

    Ok(Column {
        name,
        ty,
        nullable: !not_null,
        default,
        identity,
        generated,
        collation,
        comment,
    })
}

fn apply_constraints(
    tables: &mut HashMap<i64, Table>,
    rows: &[Row],
    filter: &CatalogFilter,
    drift: &mut DriftReport,
) -> Result<(), CatalogError> {
    let q = CatalogQuery::Constraints;

    // Build attnum→name maps per table for `conkey` resolution.
    let mut attnum_map: HashMap<i64, Vec<Identifier>> = HashMap::new();
    for (oid, table) in tables.iter() {
        let names = table.columns.iter().map(|c| c.name.clone()).collect();
        attnum_map.insert(*oid, names);
    }

    for r in rows {
        let table_qname = qname_from(r, q, "table_schema", "table_name")?;
        if !filter.allows(&table_qname) {
            continue;
        }
        let Some((oid, _)) = tables
            .iter()
            .find(|(_, t)| t.qname == table_qname)
            .map(|(o, t)| (*o, t.clone()))
        else {
            continue;
        };

        // Check for NOT VALID state before building the constraint IR.
        // `convalidated` defaults to true for most constraint types; false means
        // the constraint was added NOT VALID and has not been validated yet.
        let convalidated = r.get_bool(q, "convalidated").unwrap_or(true);
        if !convalidated {
            let constraint_name = ident_required(&r.get_text(q, "name")?)?;
            drift
                .pending_validation
                .push((table_qname.clone(), constraint_name));
        }

        let cons = build_constraint(r, attnum_map.get(&oid))?;
        if let Some(c) = cons {
            tables
                .get_mut(&oid)
                .expect("present above")
                .constraints
                .push(c);
        }
    }
    Ok(())
}

fn build_constraint(
    r: &Row,
    columns_by_attnum: Option<&Vec<Identifier>>,
) -> Result<Option<Constraint>, CatalogError> {
    let q = CatalogQuery::Constraints;
    let name = ident_required(&r.get_text(q, "name")?)?;
    let schema = ident_required(&r.get_text(q, "schema")?)?;
    let qname = QualifiedName::new(schema, name);
    let contype = r.get_char(q, "contype")?;
    let deferrable = if r.get_bool(q, "deferrable")? {
        Deferrable::Deferrable {
            initially_deferred: r.get_bool(q, "deferred")?,
        }
    } else {
        Deferrable::NotDeferrable
    };
    let comment = r.get_opt_text(q, "comment")?;

    let conkey = r.get_int_array(q, "conkey").unwrap_or_default();
    let columns = resolve_attnums(&conkey, columns_by_attnum)?;

    let kind = match contype {
        'p' => ConstraintKind::PrimaryKey {
            columns,
            include: vec![],
        },
        'u' => {
            // PG 15+ exposes nulls_not_distinct via pg_index; for now we only
            // know whether the constraint is `NULLS NOT DISTINCT` by parsing
            // pg_get_constraintdef. Default to nulls_distinct=true.
            let def = r.get_opt_text(q, "constraint_def")?.unwrap_or_default();
            let nulls_distinct = !def.to_uppercase().contains("NULLS NOT DISTINCT");
            ConstraintKind::Unique {
                columns,
                include: vec![],
                nulls_distinct,
            }
        }
        'f' => {
            let fk_attnums = r.get_int_array(q, "confkey").unwrap_or_default();
            let fk_table_schema = ident_required(&r.get_text(q, "fk_schema")?)?;
            let fk_table_name = ident_required(&r.get_text(q, "fk_table")?)?;
            let referenced_table = QualifiedName::new(fk_table_schema, fk_table_name);
            // We don't have the referenced table's columns indexed here; reparse
            // pg_get_constraintdef for the column list and on_update/on_delete.
            let def = r.get_opt_text(q, "constraint_def")?.unwrap_or_default();
            let referenced_columns = parse_fk_referenced_columns(&def)
                .unwrap_or_else(|| placeholder_idents(fk_attnums.len()));
            let on_update = parse_referential_action(&r.get_text(q, "on_update")?);
            let on_delete = parse_referential_action(&r.get_text(q, "on_delete")?);
            let match_type = parse_match_type(&r.get_text(q, "match_type")?);
            ConstraintKind::ForeignKey(ForeignKey {
                columns,
                referenced_table,
                referenced_columns,
                on_update,
                on_delete,
                match_type,
            })
        }
        'c' => {
            let def = r
                .get_opt_text(q, "constraint_def")?
                .unwrap_or_else(|| String::from("CHECK (true)"));
            let expr = parse_check_expression(&def)?;
            ConstraintKind::Check {
                expression: expr,
                no_inherit: r.get_bool(q, "no_inherit").unwrap_or(false),
            }
        }
        other => {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                format!("unknown constraint kind: {other:?}"),
            )));
        }
    };

    Ok(Some(Constraint {
        qname,
        kind,
        deferrable,
        comment,
    }))
}

fn resolve_attnums(
    conkey: &[i64],
    columns: Option<&Vec<Identifier>>,
) -> Result<Vec<Identifier>, CatalogError> {
    let Some(cols) = columns else {
        return Ok(vec![]);
    };
    let mut out = Vec::with_capacity(conkey.len());
    for k in conkey {
        let idx = usize::try_from(*k - 1).unwrap_or(0);
        if let Some(c) = cols.get(idx) {
            out.push(c.clone());
        }
    }
    Ok(out)
}

fn placeholder_idents(n: usize) -> Vec<Identifier> {
    (0..n)
        .map(|i| Identifier::from_unquoted(&format!("col{i}")).expect("valid"))
        .collect()
}

fn parse_referential_action(s: &str) -> ReferentialAction {
    match s {
        "r" => ReferentialAction::Restrict,
        "c" => ReferentialAction::Cascade,
        "n" => ReferentialAction::SetNull(vec![]),
        "d" => ReferentialAction::SetDefault(vec![]),
        // `a` (default) or empty/space.
        _ => ReferentialAction::NoAction,
    }
}

const fn parse_match_type(s: &str) -> FkMatchType {
    let b = s.as_bytes();
    if b.len() == 1 && (b[0] == b'f' || b[0] == b'F') {
        FkMatchType::Full
    } else {
        FkMatchType::Simple
    }
}

/// Extract `(col, col, ...)` after `REFERENCES schema.tab` from a constraint
/// definition. Returns `None` if the structure is unrecognized.
fn parse_fk_referenced_columns(def: &str) -> Option<Vec<Identifier>> {
    let lower = def.to_ascii_lowercase();
    let refs_pos = lower.find("references")?;
    let after_refs = &def[refs_pos + "references".len()..];
    let lparen = after_refs.find('(')?;
    let rparen_offset = after_refs[lparen + 1..].find(')')?;
    let inner = &after_refs[lparen + 1..lparen + 1 + rparen_offset];
    Some(
        inner
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .filter_map(|s| Identifier::from_unquoted(s).ok())
            .collect(),
    )
}

/// Strip the outer `CHECK (` / `)` from a `pg_get_constraintdef` payload and
/// reparse the inner predicate.
fn parse_check_expression(def: &str) -> Result<NormalizedExpr, CatalogError> {
    let s = def.trim();
    let inner = s
        .strip_prefix("CHECK")
        .or_else(|| s.strip_prefix("check"))
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(s);
    reparse_expression_text(inner)
}

/// Parse `text` as a SQL expression by wrapping it in `SELECT (...) AS x` and
/// extracting the resulting expression node, then normalize it.
fn reparse_expression_text(text: &str) -> Result<NormalizedExpr, CatalogError> {
    let sql = format!("SELECT ({text}) AS __pgevolve_expr__");
    let parsed = pg_query::parse(&sql).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not reparse expression {text:?}: {e}"
        )))
    })?;
    let stmt = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|raw| raw.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "reparsed expression had no statement".into(),
            ))
        })?;
    let NodeEnum::SelectStmt(s) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "expression scaffold did not yield SelectStmt".into(),
        )));
    };
    let target = s
        .target_list
        .into_iter()
        .next()
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "expression scaffold had no target".into(),
            ))
        })?;
    let NodeEnum::ResTarget(rt) = target else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "expression scaffold target was not a ResTarget".into(),
        )));
    };
    let inner = rt.val.and_then(|n| n.node).ok_or_else(|| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "expression scaffold ResTarget missing value".into(),
        ))
    })?;
    let location = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
    crate::parse::normalize_expr::from_pg_node(&inner, None, &location).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not normalize expression: {e}"
        )))
    })
}

/// Parse a default-expression text from `pg_get_expr`. Recognizes `nextval` as
/// [`DefaultExpr::Sequence`] and bare literals as [`DefaultExpr::Literal`];
/// anything else becomes [`DefaultExpr::Expr`].
fn parse_default_expr_text(
    text: &str,
    target_type: &ColumnType,
) -> Result<DefaultExpr, CatalogError> {
    let sql = format!("SELECT ({text}) AS __pgevolve_default__");
    let parsed = pg_query::parse(&sql).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not reparse default expression {text:?}: {e}"
        )))
    })?;
    let stmt = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|raw| raw.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "default scaffold had no statement".into(),
            ))
        })?;
    let NodeEnum::SelectStmt(s) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "default scaffold not SelectStmt".into(),
        )));
    };
    let target = s
        .target_list
        .into_iter()
        .next()
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "default scaffold missing target".into(),
            ))
        })?;
    let NodeEnum::ResTarget(rt) = target else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "default scaffold target not ResTarget".into(),
        )));
    };
    let inner = rt.val.and_then(|n| n.node).ok_or_else(|| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "default scaffold ResTarget missing value".into(),
        ))
    })?;
    let location = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
    builder::shared::build_default_expr(&inner, Some(target_type), None, &location).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not build default: {e}"
        )))
    })
}

fn build_indexes(
    rows: &[Row],
    _version: PgVersion,
    filter: &CatalogFilter,
    drift: &mut DriftReport,
) -> Result<Vec<Index>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Indexes;
        let qname = qname_from(r, q, "schema", "name")?;
        let table_qname = qname_from(r, q, "table_schema", "table_name")?;
        if !filter.allows(&qname) || !filter.allows(&table_qname) {
            continue;
        }

        // Check for INVALID state. `indisvalid` is false when a concurrent
        // index build failed and left an INVALID index behind.
        let indisvalid = r.get_bool(q, "indisvalid").unwrap_or(true);
        if !indisvalid {
            drift.invalid_indexes.push(qname.clone());
        }

        let indexdef = r.get_text(q, "indexdef")?;
        let mut idx = parse_index_def(&indexdef)?;
        // `pg_get_indexdef` always returns fully-qualified names; trust them.
        idx.qname = qname;
        // Resolve the parent kind: 'm' = materialized view, everything else = table.
        let parent_relkind = r.get_opt_text(q, "parent_relkind")?.unwrap_or_default();
        idx.on = if parent_relkind == "m" {
            IndexParent::Mv(table_qname)
        } else {
            IndexParent::Table(table_qname)
        };
        idx.comment = r.get_opt_text(q, "comment")?;
        idx.nulls_not_distinct = r.get_bool(q, "nulls_not_distinct").unwrap_or(false);
        out.push(idx);
    }
    Ok(out)
}

fn parse_index_def(sql: &str) -> Result<Index, CatalogError> {
    let parsed = pg_query::parse(sql).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not reparse indexdef {sql:?}: {e}"
        )))
    })?;
    let stmt = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|raw| raw.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "indexdef had no statement".into(),
            ))
        })?;
    let NodeEnum::IndexStmt(idx_stmt) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            "indexdef scaffold did not yield IndexStmt".into(),
        )));
    };
    let location = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
    builder::index_stmt::build_index(&idx_stmt, None, &location).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "indexdef → IR failed: {e}"
        )))
    })
}

fn build_sequence(r: &Row, filter: &CatalogFilter) -> Result<Option<Sequence>, CatalogError> {
    let q = CatalogQuery::Sequences;
    let qname = qname_from(r, q, "schema", "name")?;
    if !filter.allows(&qname) {
        return Ok(None);
    }
    let data_type_string = r.get_text(q, "data_type_string")?;
    let data_type = ColumnType::parse_from_pg_type_string(&data_type_string).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
            "{data_type_string}: {e}"
        )))
    })?;
    let start = r.get_int(q, "start")?;
    let increment = r.get_int(q, "increment")?;
    let cache = r.get_int(q, "cache")?;
    let cycle = r.get_bool(q, "cycle")?;
    let comment = r.get_opt_text(q, "comment")?;

    // PG stores explicit `min_value`/`max_value` even when the source didn't
    // specify them — the value defaults to the type's full range plus
    // direction-aware start adjustments. Treat the type-default values as
    // "unspecified" so the IR doesn't gain phantom MIN/MAX clauses.
    let raw_min = r.get_int(q, "min_value")?;
    let raw_max = r.get_int(q, "max_value")?;
    let (default_min, default_max) = sequence_default_bounds(&data_type, increment);
    let min_value = (raw_min != default_min).then_some(raw_min);
    let max_value = (raw_max != default_max).then_some(raw_max);

    Ok(Some(Sequence {
        qname,
        data_type,
        start,
        increment,
        min_value,
        max_value,
        cache,
        cycle,
        owned_by: None,
        comment,
    }))
}

/// PG's per-type defaults for `MINVALUE`/`MAXVALUE` when not explicitly set.
///
/// For ascending sequences (`increment > 0`), `MINVALUE` defaults to `1` and
/// `MAXVALUE` to the type's max. For descending sequences, the roles flip.
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

fn apply_dependencies(
    rows: &[Row],
    tables: &mut HashMap<i64, Table>,
    sequences: &mut HashMap<String, Sequence>,
) -> Result<(), CatalogError> {
    let q = CatalogQuery::Dependencies;
    for r in rows {
        let seq_qname = qname_from(r, q, "sequence_schema", "sequence_name")?;
        let owner_table_qname = qname_from(r, q, "owner_schema", "owner_table")?;
        let owner_column_name = ident_required(&r.get_text(q, "owner_column")?)?;

        // Set owned_by on the sequence.
        if let Some(seq) = sequences.get_mut(&seq_qname.to_string()) {
            seq.owned_by = Some(SequenceOwner {
                table: owner_table_qname.clone(),
                column: owner_column_name.clone(),
            });
        }

        // Convert the column's default to DefaultExpr::Sequence(seq_qname) when
        // the existing default is an Expr referencing the same sequence text.
        // We also handle the case where pg_get_expr returned `nextval('...')`
        // and parse_default_expr_text already produced DefaultExpr::Sequence —
        // in that case there's nothing to do.
        for table in tables.values_mut() {
            if table.qname != owner_table_qname {
                continue;
            }
            for col in &mut table.columns {
                if col.name != owner_column_name {
                    continue;
                }
                if let Some(DefaultExpr::Sequence(_)) = col.default.as_ref() {
                    // Already correct (parse_default_expr_text did its job).
                    continue;
                }
                if col.default.is_some()
                    && default_references_sequence(col.default.as_ref(), &seq_qname)
                {
                    col.default = Some(DefaultExpr::Sequence(seq_qname.clone()));
                }
            }
        }
    }
    Ok(())
}

fn default_references_sequence(default: Option<&DefaultExpr>, seq: &QualifiedName) -> bool {
    let Some(DefaultExpr::Expr(e)) = default else {
        return false;
    };
    let needle = format!("'{seq}'");
    e.canonical_text.contains("nextval")
        && (e.canonical_text.contains(&needle) || e.canonical_text.contains(seq.name.as_str()))
}

/// Parse a `reloptions` text array (`["security_barrier=true", ...]`) and
/// extract a named boolean option. Returns `None` if the option is absent.
fn parse_bool_reloption(reloptions: &[String], key: &str) -> Option<bool> {
    let prefix = format!("{key}=");
    for opt in reloptions {
        if let Some(val) = opt.strip_prefix(&prefix) {
            return Some(val.eq_ignore_ascii_case("true") || val == "1" || val == "on");
        }
    }
    None
}

/// Build a [`NormalizedBody`] from a `body_text` returned by
/// `pg_get_viewdef`. Returns an error if the text cannot be parsed.
fn build_body(body_text: &str, qname: &QualifiedName) -> Result<NormalizedBody, CatalogError> {
    NormalizedBody::from_sql(body_text).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "could not canonicalize body of view {qname}: {e}"
        )))
    })
}

/// Build all views and materialized views from catalog rows.
///
/// Returns `(views, materialized_views)`.
fn build_views_and_mvs(
    view_rows: &[Row],
    column_rows: &[Row],
    filter: &CatalogFilter,
) -> Result<(Vec<View>, Vec<MaterializedView>), CatalogError> {
    // Group view-column rows by (schema_name, view_name) preserving attnum order.
    let mut columns_by_key: HashMap<(String, String), Vec<ViewColumn>> = HashMap::new();
    for cr in column_rows {
        let q = CatalogQuery::ViewColumns;
        let schema = cr.get_text(q, "schema_name")?;
        let view_name = cr.get_text(q, "view_name")?;
        let col_name_str = cr.get_text(q, "column_name")?;
        let col_type_str = cr.get_text(q, "column_type")?;
        let comment = cr.get_opt_text(q, "column_comment")?;
        let name = ident_required(&col_name_str)?;
        let column_type = ColumnType::parse_from_pg_type_string(&col_type_str).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                "view column {col_name_str} type {col_type_str:?}: {e}"
            )))
        })?;
        columns_by_key
            .entry((schema, view_name))
            .or_default()
            .push(ViewColumn {
                name,
                column_type,
                comment,
            });
    }

    let mut views: Vec<View> = Vec::new();
    let mut materialized_views: Vec<MaterializedView> = Vec::new();

    for r in view_rows {
        let q = CatalogQuery::ViewsAndMvs;
        let qname = qname_from(r, q, "schema_name", "name")?;
        if !filter.allows(&qname) {
            continue;
        }
        let relkind = r.get_text(q, "relkind")?;
        let body_text = r.get_text(q, "body_text")?;
        let reloptions = r.get_text_array(q, "reloptions").unwrap_or_default();

        let body_canonical = build_body(&body_text, &qname)?;

        let col_key = (
            qname.schema.as_str().to_string(),
            qname.name.as_str().to_string(),
        );
        let columns = columns_by_key.remove(&col_key).unwrap_or_default();

        // Extract dependencies by walking the body AST. On the catalog side we
        // don't need to resolve against a KnownObjects set — the DB is the ground
        // truth. We call the internal walker without the resolution guard by
        // building an unrestricted KnownObjects that always returns true.
        let body_dependencies = extract_deps_from_body(&body_text, &qname);

        match relkind.as_str() {
            "v" => {
                let security_barrier = parse_bool_reloption(&reloptions, "security_barrier");
                let security_invoker = parse_bool_reloption(&reloptions, "security_invoker");
                views.push(View {
                    qname,
                    columns,
                    body_canonical,
                    body_dependencies,
                    security_barrier,
                    security_invoker,
                    comment: None,
                    raw_body: String::new(),
                });
            }
            "m" => {
                materialized_views.push(MaterializedView {
                    qname,
                    columns,
                    body_canonical,
                    body_dependencies,
                    comment: None,
                    raw_body: String::new(),
                });
            }
            other => {
                return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                    format!("unexpected relkind {other:?} in view catalog query"),
                )));
            }
        }
    }

    Ok((views, materialized_views))
}

/// Walk a single AST node, collecting schema-qualified `RangeVar` references.
///
/// Used by [`extract_deps_from_body`] to avoid nesting a `fn` after statements
/// (which Clippy forbids with `items_after_statements`).
fn walk_node_for_deps(
    node: &pg_query::protobuf::Node,
    view_qname: &QualifiedName,
    deps: &mut Vec<crate::plan::edges::DepEdge>,
) {
    use crate::plan::edges::{DepEdge, DepSource, NodeId};
    use pg_query::NodeEnum as N;

    let Some(inner) = &node.node else { return };
    match inner {
        N::SelectStmt(sel) => {
            for from in &sel.from_clause {
                walk_node_for_deps(from, view_qname, deps);
            }
            if let Some(wc) = &sel.where_clause {
                walk_node_for_deps(wc, view_qname, deps);
            }
            if let Some(larg) = &sel.larg {
                let n = pg_query::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(larg.as_ref().clone()))),
                };
                walk_node_for_deps(&n, view_qname, deps);
            }
            if let Some(rarg) = &sel.rarg {
                let n = pg_query::protobuf::Node {
                    node: Some(N::SelectStmt(Box::new(rarg.as_ref().clone()))),
                };
                walk_node_for_deps(&n, view_qname, deps);
            }
            if let Some(with) = &sel.with_clause {
                for cte in &with.ctes {
                    walk_node_for_deps(cte, view_qname, deps);
                }
            }
        }
        N::RangeVar(rv) if !rv.schemaname.is_empty() && !rv.relname.is_empty() => {
            if let (Ok(s), Ok(n)) = (
                Identifier::from_unquoted(&rv.schemaname)
                    .or_else(|_| Identifier::from_quoted(&rv.schemaname)),
                Identifier::from_unquoted(&rv.relname)
                    .or_else(|_| Identifier::from_quoted(&rv.relname)),
            ) {
                let ref_qname = QualifiedName::new(s, n);
                deps.push(DepEdge {
                    from: NodeId::Table(view_qname.clone()),
                    to: NodeId::Table(ref_qname),
                    source: DepSource::AstExtracted,
                });
            }
        }
        N::JoinExpr(j) => {
            if let Some(l) = &j.larg {
                walk_node_for_deps(l, view_qname, deps);
            }
            if let Some(r) = &j.rarg {
                walk_node_for_deps(r, view_qname, deps);
            }
        }
        N::RangeSubselect(sub) => {
            if let Some(sq) = &sub.subquery {
                walk_node_for_deps(sq, view_qname, deps);
            }
        }
        N::CommonTableExpr(cte) => {
            if let Some(q) = &cte.ctequery {
                walk_node_for_deps(q, view_qname, deps);
            }
        }
        _ => {}
    }
}

/// Extract [`DepEdge`]s from a view body on the catalog side.
///
/// On the catalog side we are the ground truth — there is no "unknown object"
/// error. We perform a best-effort extraction: any schema-qualified `RangeVar`
/// nodes become dep edges. Unresolvable or unqualified references are silently
/// skipped.
fn extract_deps_from_body(
    body_text: &str,
    view_qname: &QualifiedName,
) -> Vec<crate::plan::edges::DepEdge> {
    use crate::plan::edges::DepEdge;

    let Ok(parsed) = pg_query::parse(body_text) else {
        return vec![];
    };

    let mut deps: Vec<DepEdge> = Vec::new();
    for raw_stmt in &parsed.protobuf.stmts {
        if let Some(node) = &raw_stmt.stmt {
            walk_node_for_deps(node, view_qname, &mut deps);
        }
    }

    deps.sort();
    deps.dedup();
    deps
}

fn qname_from(
    r: &Row,
    q: CatalogQuery,
    schema_key: &str,
    name_key: &str,
) -> Result<QualifiedName, CatalogError> {
    let schema = ident_required(&r.get_text(q, schema_key)?)?;
    let name = ident_required(&r.get_text(q, name_key)?)?;
    Ok(QualifiedName::new(schema, name))
}

fn ident_required(s: &str) -> Result<Identifier, CatalogError> {
    Identifier::from_unquoted(s)
        .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fk_referenced_columns_parsed() {
        let def = "FOREIGN KEY (org_id) REFERENCES app.orgs(id) ON DELETE CASCADE";
        let cols = parse_fk_referenced_columns(def).unwrap();
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].as_str(), "id");
    }

    #[test]
    fn fk_referenced_columns_multi() {
        let def = "FOREIGN KEY (a, b) REFERENCES app.t(x, y)";
        let cols = parse_fk_referenced_columns(def).unwrap();
        assert_eq!(cols.len(), 2);
    }

    #[test]
    fn check_expression_strips_outer_check() {
        let e = parse_check_expression("CHECK ((n > 0))").unwrap();
        assert!(e.canonical_text.contains('n') || e.canonical_text.contains('>'));
    }

    #[test]
    fn parse_default_recognizes_nextval() {
        let d =
            parse_default_expr_text("nextval('app.seq1'::regclass)", &ColumnType::BigInt).unwrap();
        match d {
            DefaultExpr::Sequence(q) => assert_eq!(q.to_string(), "app.seq1"),
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn parse_default_integer_literal() {
        let d = parse_default_expr_text("0", &ColumnType::Integer).unwrap();
        assert!(matches!(
            d,
            DefaultExpr::Literal(crate::ir::default_expr::LiteralValue::Integer(0))
        ));
    }
}
