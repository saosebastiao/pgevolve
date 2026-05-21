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
use crate::ir::extension::Extension;
use crate::ir::function::{
    ArgMode, Function, FunctionArg, FunctionLanguage, NormalizedArgTypes, ParallelSafety,
    ReturnType, SecurityMode, TableColumn, Volatility,
};
use crate::ir::index::{Index, IndexParent};
use crate::ir::procedure::Procedure;
use crate::ir::schema::Schema;
use crate::ir::sequence::{Sequence, SequenceOwner};
use crate::ir::table::Table;
use crate::ir::trigger::Trigger;
use crate::ir::user_type::{CompositeAttribute, DomainCheck, EnumValue, UserType, UserTypeKind};
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
    pub user_types: Vec<Row>,
    pub enum_values: Vec<Row>,
    pub domain_details: Vec<Row>,
    pub domain_checks: Vec<Row>,
    pub composite_attributes: Vec<Row>,
    pub functions: Vec<Row>,
    pub extensions: Vec<Row>,
    pub triggers: Vec<Row>,
    pub partitioned_tables: Vec<Row>,
    pub partitions: Vec<Row>,
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
        user_types,
        enum_values,
        domain_details,
        domain_checks,
        composite_attributes,
        functions,
        extensions,
        triggers,
        partitioned_tables,
        partitions,
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

    // Build user-defined types (enums, domains, composites).
    catalog.types = build_user_types(
        &user_types,
        &enum_values,
        &domain_details,
        &domain_checks,
        &composite_attributes,
        filter,
    )?;

    // Build functions and procedures from pg_proc.
    let (fns, procs) = build_functions_and_procedures(&functions, filter, &mut drift)?;
    catalog.functions = fns;
    catalog.procedures = procs;

    // Build extensions from pg_extension.
    catalog.extensions = build_extensions(&extensions)?;

    // Build triggers from pg_trigger (re-parses pg_get_triggerdef output).
    catalog.triggers = build_triggers(&triggers)?;

    // Merge partition metadata: re-parse pg_get_partkeydef / pg_get_expr(relpartbound).
    merge_partition_metadata(&mut catalog, &partitioned_tables, &partitions)?;

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
                partition_by: None,
                partition_of: None,
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

    // PG stores explicit `min_value`/`max_value` even when the source
    // didn't specify them. The catalog reader returns those raw values;
    // `ir::canon::filter_pg_defaults` normalizes the type-default
    // values to None on both sides.
    let min_value = Some(r.get_int(q, "min_value")?);
    let max_value = Some(r.get_int(q, "max_value")?);

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
        let comment = r.get_opt_text(q, "comment")?;

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
                    comment: comment.clone(),
                    raw_body: String::new(),
                });
            }
            "m" => {
                materialized_views.push(MaterializedView {
                    qname,
                    columns,
                    body_canonical,
                    body_dependencies,
                    comment: comment.clone(),
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

/// Build user-defined types (enums, domains, composites) from raw catalog rows.
#[allow(clippy::too_many_lines)]
fn build_user_types(
    type_rows: &[Row],
    enum_value_rows: &[Row],
    domain_detail_rows: &[Row],
    domain_check_rows: &[Row],
    comp_attr_rows: &[Row],
    filter: &CatalogFilter,
) -> Result<Vec<UserType>, CatalogError> {
    use crate::catalog::CatalogQuery as Q;
    use std::collections::HashMap;

    // ---- group enum values by (schema, type_name) ----
    let mut enum_values: HashMap<(String, String), Vec<(f32, String)>> = HashMap::new();
    for r in enum_value_rows {
        let schema = r.get_text(Q::EnumValues, "schema_name")?;
        let type_name = r.get_text(Q::EnumValues, "type_name")?;
        let value_name = r.get_text(Q::EnumValues, "value_name")?;
        let sort_order_text = r.get_text(Q::EnumValues, "sort_order")?;
        let sort_order: f32 = sort_order_text.parse().map_err(|_| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "cannot parse enum sort_order as f32: {sort_order_text:?}"
            )))
        })?;
        enum_values
            .entry((schema, type_name))
            .or_default()
            .push((sort_order, value_name));
    }

    // ---- group domain details by (schema, name) ----
    // Each domain has exactly one details row; store the whole row.
    let mut domain_details: HashMap<(String, String), (String, bool, Option<String>)> =
        HashMap::new();
    for r in domain_detail_rows {
        let schema = r.get_text(Q::DomainDetails, "schema_name")?;
        let name = r.get_text(Q::DomainDetails, "name")?;
        let base_type = r.get_text(Q::DomainDetails, "base_type")?;
        let not_null = r.get_bool(Q::DomainDetails, "not_null")?;
        let default_expr = r.get_opt_text(Q::DomainDetails, "default_expr")?;
        domain_details.insert((schema, name), (base_type, not_null, default_expr));
    }

    // ---- group domain checks by (schema, type_name) ----
    let mut domain_checks: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();
    for r in domain_check_rows {
        let schema = r.get_text(Q::DomainChecks, "schema_name")?;
        let type_name = r.get_text(Q::DomainChecks, "type_name")?;
        let constraint_name = r.get_text(Q::DomainChecks, "constraint_name")?;
        let expression = r.get_text(Q::DomainChecks, "expression")?;
        domain_checks
            .entry((schema, type_name))
            .or_default()
            .push((constraint_name, expression));
    }

    // ---- group composite attributes by (schema, type_name) ----
    // Rows arrive ordered by attnum from SQL, so we just append.
    let mut comp_attrs: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();
    for r in comp_attr_rows {
        let schema = r.get_text(Q::CompositeAttributes, "schema_name")?;
        let type_name = r.get_text(Q::CompositeAttributes, "type_name")?;
        let attr_name = r.get_text(Q::CompositeAttributes, "attribute_name")?;
        let attr_type = r.get_text(Q::CompositeAttributes, "attribute_type")?;
        comp_attrs
            .entry((schema, type_name))
            .or_default()
            .push((attr_name, attr_type));
    }

    // ---- assemble per type header ----
    let mut out: Vec<UserType> = Vec::with_capacity(type_rows.len());
    for r in type_rows {
        let schema_name = r.get_text(Q::UserTypes, "schema_name")?;
        let name = r.get_text(Q::UserTypes, "name")?;
        let kind_str = r.get_text(Q::UserTypes, "kind")?;
        let comment = r.get_opt_text(Q::UserTypes, "comment")?;

        let qname = qname_from(r, Q::UserTypes, "schema_name", "name")?;
        if !filter.allows(&qname) {
            continue;
        }

        let kind_char = kind_str.chars().next().ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "empty kind for type {qname}"
            )))
        })?;

        let key = (schema_name, name);

        let kind = match kind_char {
            'e' => {
                let mut values: Vec<EnumValue> = enum_values
                    .get(&key)
                    .into_iter()
                    .flatten()
                    .map(|(sort_order, value_name)| EnumValue {
                        name: value_name.clone(),
                        sort_order: *sort_order,
                    })
                    .collect();
                // Already ordered by enumsortorder from SQL, but sort
                // explicitly for safety.
                values.sort_by(|a, b| {
                    a.sort_order
                        .partial_cmp(&b.sort_order)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                UserTypeKind::Enum { values }
            }
            'd' => {
                let (base_type_str, not_null, default_expr_text) =
                    domain_details.get(&key).ok_or_else(|| {
                        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                            "domain {qname} has no details row"
                        )))
                    })?;
                let base = ColumnType::parse_from_pg_type_string(base_type_str).map_err(|e| {
                    CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                        "domain {qname} base type {base_type_str:?}: {e}"
                    )))
                })?;
                let default = default_expr_text
                    .as_deref()
                    .map(reparse_expression_text)
                    .transpose()?;
                let checks: Vec<DomainCheck> = domain_checks
                    .get(&key)
                    .into_iter()
                    .flatten()
                    .map(|(constraint_name, expression)| {
                        let body = strip_check_wrapper(expression);
                        let expression = reparse_expression_text(body)?;
                        let name = ident_required(constraint_name)?;
                        Ok(DomainCheck { name, expression })
                    })
                    .collect::<Result<_, CatalogError>>()?;
                UserTypeKind::Domain {
                    base,
                    nullable: !not_null,
                    default,
                    check_constraints: checks,
                    collation: None,
                }
            }
            'c' => {
                let attributes: Vec<CompositeAttribute> = comp_attrs
                    .get(&key)
                    .into_iter()
                    .flatten()
                    .map(|(attr_name, attr_type)| {
                        let name = ident_required(attr_name)?;
                        let ty = ColumnType::parse_from_pg_type_string(attr_type).map_err(|e| {
                            CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                                "composite {qname} attr {attr_name} type {attr_type:?}: {e}"
                            )))
                        })?;
                        Ok(CompositeAttribute {
                            name,
                            ty,
                            collation: None,
                        })
                    })
                    .collect::<Result<_, CatalogError>>()?;
                UserTypeKind::Composite { attributes }
            }
            other => {
                return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                    format!("unknown user type kind {other:?} for {qname}"),
                )));
            }
        };

        out.push(UserType {
            qname,
            kind,
            comment,
        });
    }

    // canonicalize sorts by qname — no pre-sort needed here, but do it for
    // consistent ordering before the catalog-level canonicalize call.
    out.sort_by(|a, b| a.qname.cmp(&b.qname));
    Ok(out)
}

/// Build functions and procedures from `pg_proc` rows.
///
/// Rows with an unsupported language are skipped and reported in the
/// [`DriftReport::unmanaged_language_routines`] list. The SQL fragment used
/// to fetch these rows is [`crate::catalog::queries::functions::SELECT_FUNCTIONS`].
///
/// Returns `(functions, procedures)`.
#[allow(clippy::too_many_lines)]
fn build_functions_and_procedures(
    rows: &[Row],
    filter: &CatalogFilter,
    drift: &mut DriftReport,
) -> Result<(Vec<Function>, Vec<Procedure>), CatalogError> {
    use crate::catalog::CatalogQuery as Q;

    let mut functions: Vec<Function> = Vec::new();
    let mut procedures: Vec<Procedure> = Vec::new();

    let catalog_location = SourceLocation::new(std::path::PathBuf::from("<catalog>"), 1, 1);

    for r in rows {
        let schema_name = r.get_text(Q::Functions, "schema_name")?;
        let name = r.get_text(Q::Functions, "name")?;
        let kind = r.get_text(Q::Functions, "kind")?;
        let language_str = r.get_text(Q::Functions, "language")?;
        let arg_full = r.get_text(Q::Functions, "arg_full")?;
        let return_type_str = r
            .get_opt_text(Q::Functions, "return_type")?
            .unwrap_or_default();
        let full_def = r.get_text(Q::Functions, "full_def")?;
        let comment = r.get_opt_text(Q::Functions, "comment")?;
        let security_definer = r.get_bool(Q::Functions, "security_definer")?;

        let qname = QualifiedName::new(ident_required(&schema_name)?, ident_required(&name)?);

        if !filter.allows(&qname) {
            continue;
        }

        // Determine language — skip unsupported ones.
        let language = match language_str.to_lowercase().as_str() {
            "sql" => FunctionLanguage::Sql,
            "plpgsql" => FunctionLanguage::PlPgSql,
            other => {
                drift
                    .unmanaged_language_routines
                    .push((qname, other.to_string()));
                continue;
            }
        };

        let security = if security_definer {
            SecurityMode::Definer
        } else {
            SecurityMode::Invoker
        };

        // Parse the arg list from the "full arguments" string provided by
        // pg_get_function_arguments. We synthesize a wrapper CREATE FUNCTION
        // and re-parse it to get a properly typed argument list.
        let args = parse_arg_full(&arg_full, &qname, &catalog_location)?;

        // Extract the body from full_def via dollar-quote matching.
        let body_text = extract_body_from_functiondef(&full_def, &qname)?;

        // Re-parse the body through the same pipeline as the source side.
        let (body, body_dependencies, commits_in_body) =
            crate::parse::builder::plpgsql::parse_routine_body(
                &body_text,
                language,
                &qname,
                &catalog_location,
            )
            .map_err(|e| {
                CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                    "catalog body parse for {qname}: {e}"
                )))
            })?;

        match kind.as_str() {
            "f" => {
                // Function-only columns.
                let volatility_str = r.get_text(Q::Functions, "volatility")?;
                let strict = r.get_bool(Q::Functions, "strict")?;
                let parallel_str = r.get_text(Q::Functions, "parallel")?;
                let leakproof = r.get_bool(Q::Functions, "leakproof")?;
                let cost_str = r.get_opt_text(Q::Functions, "cost")?;
                let rows_str = r.get_opt_text(Q::Functions, "rows")?;

                let volatility = match volatility_str.as_str() {
                    "i" => Volatility::Immutable,
                    "s" => Volatility::Stable,
                    _ => Volatility::Volatile,
                };
                let parallel = match parallel_str.as_str() {
                    "s" => ParallelSafety::Safe,
                    "r" => ParallelSafety::Restricted,
                    _ => ParallelSafety::Unsafe,
                };
                // Catalog returns raw cost/rows; `ir::canon::filter_pg_defaults`
                // normalizes the PG-defaults (procost=100, prorows=1000 for
                // SETOF, prorows=0 otherwise) to None on both sides.
                let cost: Option<f32> = cost_str.as_deref().and_then(|s| s.parse::<f32>().ok());
                let rows: Option<f32> = rows_str.as_deref().and_then(|s| s.parse::<f32>().ok());

                let return_type = parse_return_type_from_string(&return_type_str, &qname)?;
                let arg_types_normalized = NormalizedArgTypes::from_args(&args);

                functions.push(Function {
                    qname,
                    args,
                    arg_types_normalized,
                    return_type,
                    language,
                    body,
                    body_dependencies,
                    volatility,
                    strict,
                    security,
                    parallel,
                    leakproof,
                    cost,
                    rows,
                    comment,
                });
            }
            "p" => {
                procedures.push(Procedure {
                    qname,
                    args,
                    language,
                    body,
                    body_dependencies,
                    security,
                    commits_in_body,
                    comment,
                });
            }
            other => {
                return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                    format!("unexpected prokind {other:?} in functions query"),
                )));
            }
        }
    }

    Ok((functions, procedures))
}

/// Parse a `pg_get_function_arguments` string (e.g. `"x integer, y text DEFAULT 'a'"`)
/// into a `Vec<FunctionArg>`.
///
/// The strategy: synthesize a wrapper `CREATE FUNCTION` and re-parse via
/// `pg_query`, then walk the resulting `CreateFunctionStmt.parameters`.
fn parse_arg_full(
    arg_full: &str,
    qname: &QualifiedName,
    location: &SourceLocation,
) -> Result<Vec<FunctionArg>, CatalogError> {
    use pg_query::NodeEnum;
    use pg_query::protobuf::FunctionParameterMode;

    // Empty argument list.
    if arg_full.trim().is_empty() {
        return Ok(vec![]);
    }

    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp({arg_full}) RETURNS void LANGUAGE sql AS $$ SELECT NULL $$;"
    );
    let parsed = pg_query::parse(&wrapper).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "catalog arg parse for {qname} ({arg_full:?}): {e}"
        )))
    })?;

    let stmt = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|r| r.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog arg parse for {qname}: no statement"
            )))
        })?;
    let NodeEnum::CreateFunctionStmt(stmt) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            format!("catalog arg parse for {qname}: unexpected stmt kind"),
        )));
    };

    let mut args: Vec<FunctionArg> = Vec::new();

    for param_node in &stmt.parameters {
        let Some(NodeEnum::FunctionParameter(p)) = param_node.node.as_ref() else {
            continue;
        };
        let raw_mode =
            FunctionParameterMode::try_from(p.mode).unwrap_or(FunctionParameterMode::Undefined);

        // TABLE mode params are OUT columns in the argument string — treat as Out.
        let mode = match raw_mode {
            FunctionParameterMode::FuncParamIn
            | FunctionParameterMode::FuncParamDefault
            | FunctionParameterMode::Undefined => ArgMode::In,
            FunctionParameterMode::FuncParamOut | FunctionParameterMode::FuncParamTable => {
                ArgMode::Out
            }
            FunctionParameterMode::FuncParamInout => ArgMode::InOut,
            FunctionParameterMode::FuncParamVariadic => ArgMode::Variadic,
        };

        let Some(tn) = p.arg_type.as_ref() else {
            continue;
        };
        let ty =
            crate::parse::builder::shared::type_name_to_column_type(tn, location).map_err(|e| {
                CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                    "catalog arg type for {qname}: {e}"
                )))
            })?;
        let arg_name = if p.name.is_empty() {
            None
        } else {
            Some(ident_required(&p.name)?)
        };

        let default = p
            .defexpr
            .as_ref()
            .and_then(|defexpr| defexpr.node.as_ref())
            .and_then(|node_enum| {
                crate::parse::normalize_expr::from_pg_node(node_enum, Some(&ty), location).ok()
            });

        args.push(FunctionArg {
            name: arg_name,
            mode,
            ty,
            default,
        });
    }

    Ok(args)
}

/// Extract the body text from the output of `pg_get_functiondef`.
///
/// The format is:
/// ```text
/// CREATE OR REPLACE FUNCTION schema.name(args) ...
///     AS $<tag>$
/// body text
/// $<tag>$
///   LANGUAGE ...;
/// ```
///
/// Or for SQL functions with `BEGIN ATOMIC` syntax (PG 14+):
/// ```text
/// CREATE OR REPLACE FUNCTION ... BEGIN ATOMIC ...  END
/// ```
///
/// We locate `AS $<tag>$`, then find the matching `$<tag>$` close.
fn extract_body_from_functiondef(
    full_def: &str,
    qname: &QualifiedName,
) -> Result<String, CatalogError> {
    // Find the dollar-quote open: "AS $tag$" or "AS $$".
    // We scan case-insensitively for "AS " then try to find a dollar-quote tag.
    let upper = full_def.to_ascii_uppercase();

    // Find " AS $" (dollar-quote style body).
    if let Some(as_pos) = find_as_dollar_quote_pos(&upper) {
        let rest = &full_def[as_pos..];
        // Find the tag: everything between the first $ and the next $.
        let tag_end = rest[1..].find('$').ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog body for {qname}: malformed dollar-quote open"
            )))
        })?;
        // tag is the text between the two dollar signs (possibly empty for $$).
        let tag = &rest[1..=tag_end]; // e.g. "function" or ""
        let open = format!("${tag}$");
        let body_start = as_pos + open.len();
        let body_text_after_open = &full_def[body_start..];
        // Find the closing $tag$.
        let close_pos = body_text_after_open.find(open.as_str()).ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog body for {qname}: closing dollar-quote {open:?} not found"
            )))
        })?;
        let body = &body_text_after_open[..close_pos];
        return Ok(body.to_string());
    }

    // SQL functions with BEGIN ATOMIC style (no dollar-quote).
    // pg_get_functiondef omits dollar-quotes for `BEGIN ATOMIC` SQL functions.
    // In this case the body is between "BEGIN ATOMIC" and the final "END".
    if let Some(begin_pos) = upper.find("BEGIN ATOMIC") {
        let body_start = begin_pos; // include BEGIN ... END
        // Find the matching final END before the semicolon.
        let tail = &full_def[body_start..];
        // strip trailing semicolon and find last END
        let tail_upper = tail.to_ascii_uppercase();
        let end_pos = tail_upper.rfind("END").ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "catalog body for {qname}: BEGIN ATOMIC without END"
            )))
        })?;
        let body = &tail[..end_pos + "END".len()];
        return Ok(body.to_string());
    }

    Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
        format!("catalog body for {qname}: could not extract body from pg_get_functiondef"),
    )))
}

/// Locate the position in `upper` (an uppercase version of the `full_def`) of
/// the start of the dollar-quote opening tag (i.e. at the `$`), scanning for
/// `" AS $"`.
///
/// Returns the byte offset in `upper` pointing at the first `$` of the
/// dollar-quote open tag.
fn find_as_dollar_quote_pos(upper: &str) -> Option<usize> {
    // We want "AS $tag$" where tag may be empty.
    // Scan for occurrences of " AS $" or "\nAS $" or "\tAS $".
    let bytes = upper.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        // Look for 'A' 'S' then whitespace then '$'
        if bytes[i] == b'A' && bytes[i + 1] == b'S' {
            // Check there's whitespace before (or it's at start)
            if i > 0 && !bytes[i - 1].is_ascii_whitespace() {
                continue;
            }
            // Skip whitespace after "AS"
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'$' {
                return Some(j);
            }
        }
    }
    None
}

/// Parse a return-type string from `pg_get_function_result` into a
/// [`ReturnType`].
///
/// The string is one of:
/// - `"trigger"` or `"event_trigger"`
/// - `"void"`
/// - `"SETOF <type>"`
/// - `"TABLE(<col> <type>, ...)"`
/// - a scalar type string
fn parse_return_type_from_string(
    s: &str,
    qname: &QualifiedName,
) -> Result<ReturnType, CatalogError> {
    let trimmed = s.trim();
    let lower = trimmed.to_ascii_lowercase();

    if lower == "trigger" {
        return Ok(ReturnType::Trigger);
    }
    if lower == "event_trigger" {
        return Ok(ReturnType::EventTrigger);
    }
    if lower == "void" {
        return Ok(ReturnType::Void);
    }

    if let Some(inner) = lower.strip_prefix("setof ") {
        let ty = parse_return_scalar_type(inner, trimmed, qname)?;
        return Ok(ReturnType::SetOf { ty });
    }

    if lower.starts_with("table(") && lower.ends_with(')') {
        let inner = &trimmed[6..trimmed.len() - 1]; // strip "TABLE(" and ")"
        let columns = parse_table_return_columns(inner, qname)?;
        return Ok(ReturnType::Table { columns });
    }

    let ty = parse_return_scalar_type(&lower, trimmed, qname)?;
    Ok(ReturnType::Scalar { ty })
}

/// Parse a scalar type string, using the original (non-lowercased) string for
/// user-defined type names to preserve case.
fn parse_return_scalar_type(
    lower: &str,
    original: &str,
    _qname: &QualifiedName,
) -> Result<crate::ir::column_type::ColumnType, CatalogError> {
    crate::ir::column_type::ColumnType::parse_from_pg_type_string(lower)
        .or_else(|_| crate::ir::column_type::ColumnType::parse_from_pg_type_string(original))
        .map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                "return type {original:?}: {e}"
            )))
        })
}

/// Parse `"col1 type1, col2 type2, ..."` into `Vec<TableColumn>`.
fn parse_table_return_columns(
    inner: &str,
    qname: &QualifiedName,
) -> Result<Vec<TableColumn>, CatalogError> {
    use pg_query::protobuf::FunctionParameterMode;

    // We synthesize a CREATE FUNCTION with a RETURNS TABLE clause and re-parse.
    let wrapper = format!(
        "CREATE FUNCTION pgevolve_temp() RETURNS TABLE({inner}) LANGUAGE sql AS $$ SELECT NULL $$;"
    );
    let parsed = pg_query::parse(&wrapper).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "RETURNS TABLE parse for {qname} ({inner:?}): {e}"
        )))
    })?;
    let stmt = parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|r| r.stmt)
        .and_then(|n| n.node)
        .ok_or_else(|| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "RETURNS TABLE parse for {qname}: no stmt"
            )))
        })?;
    let pg_query::NodeEnum::CreateFunctionStmt(stmt) = stmt else {
        return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
            format!("RETURNS TABLE parse for {qname}: unexpected stmt kind"),
        )));
    };

    let catalog_location = SourceLocation::new(std::path::PathBuf::from("<catalog>"), 1, 1);
    let mut columns: Vec<TableColumn> = Vec::new();
    for param_node in &stmt.parameters {
        let Some(pg_query::NodeEnum::FunctionParameter(p)) = param_node.node.as_ref() else {
            continue;
        };
        let raw_mode =
            FunctionParameterMode::try_from(p.mode).unwrap_or(FunctionParameterMode::Undefined);
        if raw_mode != FunctionParameterMode::FuncParamTable {
            continue;
        }
        let Some(tn) = p.arg_type.as_ref() else {
            continue;
        };
        let ty = crate::parse::builder::shared::type_name_to_column_type(tn, &catalog_location)
            .map_err(|e| {
                CatalogError::Ir(crate::ir::IrError::InvalidColumnType(format!(
                    "RETURNS TABLE col type for {qname}: {e}"
                )))
            })?;
        let nm = ident_required(&p.name)?;
        columns.push(TableColumn { name: nm, ty });
    }

    Ok(columns)
}

/// Strip the outer `CHECK (…)` wrapper that `pg_get_constraintdef` prepends.
///
/// Handles both `CHECK (x)` and `CHECK ((x))` forms. The resulting slice
/// points into the original string (no allocation).
fn strip_check_wrapper(text: &str) -> &str {
    let t = text.trim();
    let t = t.strip_prefix("CHECK").unwrap_or(t).trim_start();
    let t = t.strip_prefix('(').unwrap_or(t);
    let t = t.strip_suffix(')').unwrap_or(t);
    t.trim()
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

fn build_extensions(rows: &[Row]) -> Result<Vec<Extension>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Extensions;
        let name_str = r.get_text(q, "name")?;
        let name = Identifier::from_unquoted(&name_str)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        let schema_str = r.get_text(q, "schema")?;
        let schema = Identifier::from_unquoted(&schema_str)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        let version = r.get_text(q, "version")?;
        let comment = r.get_opt_text(q, "comment")?;
        out.push(Extension {
            name,
            schema: Some(schema),
            version: Some(version),
            comment,
        });
    }
    Ok(out)
}

/// Re-parse `pg_get_partkeydef` and `pg_get_expr(relpartbound)` output and
/// merge the resulting [`PartitionBy`] / [`PartitionOf`] onto the matching
/// [`Table`] entries that were already loaded by the main table query.
fn merge_partition_metadata(
    catalog: &mut Catalog,
    partitioned_rows: &[Row],
    partition_rows: &[Row],
) -> Result<(), CatalogError> {
    use crate::parse::error::SourceLocation;

    let loc = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
    apply_partitioned_parents(catalog, partitioned_rows, &loc)?;
    apply_partition_children(catalog, partition_rows, &loc)?;
    Ok(())
}

/// Apply `PARTITION BY` metadata to partitioned-table parents.
fn apply_partitioned_parents(
    catalog: &mut Catalog,
    rows: &[Row],
    loc: &crate::parse::error::SourceLocation,
) -> Result<(), CatalogError> {
    use crate::parse::builder::create_stmt::build_partition_by;

    for r in rows {
        let schema_name = r.get_text(CatalogQuery::PartitionedTables, "schema_name")?;
        let table_name = r.get_text(CatalogQuery::PartitionedTables, "table_name")?;
        let partkey_def = r.get_text(CatalogQuery::PartitionedTables, "partkey_def")?;

        let qname = qname_from_strings(&schema_name, &table_name)?;
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == qname)
            .ok_or_else(|| CatalogError::DanglingReference {
                kind: "partitioned-table parent",
                what: qname.to_string(),
            })?;

        // Wrap the raw partkey_def back into a synthetic CREATE TABLE so
        // pg_query can parse it, then extract the PartitionSpec node.
        let synthetic = format!(
            "CREATE TABLE _pgevolve_synth () PARTITION BY {};",
            partkey_def.trim()
        );
        let parsed = pg_query::parse(&synthetic).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "could not re-parse partkey_def {partkey_def:?}: {e}"
            )))
        })?;
        let Some(raw_stmt) = parsed.protobuf.stmts.into_iter().next() else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "synthetic partkey CREATE TABLE yielded no statement".into(),
            )));
        };
        let Some(node) = raw_stmt.stmt.and_then(|n| n.node) else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "synthetic partkey CREATE TABLE node was empty".into(),
            )));
        };
        let NodeEnum::CreateStmt(create_stmt) = node else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "expected CreateStmt for partkey re-parse".into(),
            )));
        };
        let spec = create_stmt
            .partspec
            .as_ref()
            .ok_or_else(|| {
                CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                    "synthetic CREATE TABLE lost partspec".into(),
                ))
            })?;

        table.partition_by = Some(build_partition_by(spec, loc)?);
    }
    Ok(())
}

/// Apply `PARTITION OF` / bound metadata to child-partition tables.
fn apply_partition_children(
    catalog: &mut Catalog,
    rows: &[Row],
    loc: &crate::parse::error::SourceLocation,
) -> Result<(), CatalogError> {
    use crate::ir::partition::PartitionOf;
    use crate::parse::builder::create_stmt::build_partition_bounds;

    for r in rows {
        let schema_name = r.get_text(CatalogQuery::Partitions, "schema_name")?;
        let table_name = r.get_text(CatalogQuery::Partitions, "table_name")?;
        let parent_schema = r.get_text(CatalogQuery::Partitions, "parent_schema")?;
        let parent_name = r.get_text(CatalogQuery::Partitions, "parent_name")?;
        let partbound_def = r.get_text(CatalogQuery::Partitions, "partbound_def")?;

        let qname = qname_from_strings(&schema_name, &table_name)?;
        let parent = qname_from_strings(&parent_schema, &parent_name)?;
        let table = catalog
            .tables
            .iter_mut()
            .find(|t| t.qname == qname)
            .ok_or_else(|| CatalogError::DanglingReference {
                kind: "child partition",
                what: qname.to_string(),
            })?;

        // Wrap the raw partbound_def in a synthetic ATTACH PARTITION so
        // pg_query can parse it, then extract the PartitionBoundSpec node.
        let synthetic = format!(
            "ALTER TABLE _pgevolve_synth ATTACH PARTITION _pgevolve_synth_child {};",
            partbound_def.trim()
        );
        let parsed = pg_query::parse(&synthetic).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "could not re-parse partbound_def {partbound_def:?}: {e}"
            )))
        })?;
        let bound_spec = extract_partition_bound_spec(parsed)?;
        let bounds = build_partition_bounds(&bound_spec, loc)?;
        table.partition_of = Some(PartitionOf { parent, bounds });
    }
    Ok(())
}

/// Walk the protobuf AST of a synthetic `ALTER TABLE … ATTACH PARTITION … <bound>`
/// statement and return the [`pg_query::protobuf::PartitionBoundSpec`] node.
fn extract_partition_bound_spec(
    parsed: pg_query::ParseResult,
) -> Result<pg_query::protobuf::PartitionBoundSpec, CatalogError> {
    let ir_err = |msg: &str| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(msg.to_string()))
    };

    let Some(raw_stmt) = parsed.protobuf.stmts.into_iter().next() else {
        return Err(ir_err("synthetic partbound ALTER TABLE yielded no statement"));
    };
    let Some(node) = raw_stmt.stmt.and_then(|n| n.node) else {
        return Err(ir_err("synthetic partbound ALTER TABLE node was empty"));
    };
    let NodeEnum::AlterTableStmt(alter_stmt) = node else {
        return Err(ir_err("expected AlterTableStmt for partbound re-parse"));
    };
    let cmd_node = alter_stmt
        .cmds
        .into_iter()
        .next()
        .and_then(|n| n.node)
        .ok_or_else(|| ir_err("AlterTableStmt had no commands for partbound re-parse"))?;
    let NodeEnum::AlterTableCmd(cmd) = cmd_node else {
        return Err(ir_err("expected AlterTableCmd in partbound re-parse"));
    };
    let part_cmd_node = cmd
        .def
        .and_then(|n| n.node)
        .ok_or_else(|| ir_err("AlterTableCmd had no def node for partbound re-parse"))?;
    let NodeEnum::PartitionCmd(part_cmd) = part_cmd_node else {
        return Err(ir_err("expected PartitionCmd in partbound re-parse"));
    };
    part_cmd
        .bound
        .ok_or_else(|| ir_err("PartitionCmd missing bound spec"))
}

/// Build a [`QualifiedName`] from two raw string slices, emitting a
/// [`CatalogError::Ir`] on invalid identifier.
fn qname_from_strings(schema: &str, name: &str) -> Result<QualifiedName, CatalogError> {
    let s = Identifier::from_unquoted(schema).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "bad schema identifier {schema:?}: {e}"
        )))
    })?;
    let n = Identifier::from_unquoted(name).map_err(|e| {
        CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
            "bad name identifier {name:?}: {e}"
        )))
    })?;
    Ok(QualifiedName::new(s, n))
}

fn build_triggers(rows: &[Row]) -> Result<Vec<Trigger>, CatalogError> {
    use crate::parse::builder::create_trigger_stmt::build_trigger;
    use crate::parse::error::SourceLocation;
    use pg_query::NodeEnum;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Triggers;
        let triggerdef = r.get_text(q, "triggerdef")?;
        let parsed = pg_query::parse(&triggerdef).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "pg_get_triggerdef returned invalid SQL: {e}: {triggerdef}"
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
                    "pg_get_triggerdef returned no statement".into(),
                ))
            })?;
        let NodeEnum::CreateTrigStmt(trig_stmt) = stmt else {
            return Err(CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(
                "pg_get_triggerdef did not yield CreateTrigStmt".into(),
            )));
        };
        let location = SourceLocation::new(PathBuf::from("<catalog>"), 1, 1);
        let mut trigger = build_trigger(&trig_stmt, &location).map_err(|e| {
            CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(format!(
                "rebuild trigger from pg_get_triggerdef: {e}"
            )))
        })?;
        trigger.comment = r.get_opt_text(q, "comment")?;
        out.push(trigger);
    }
    Ok(out)
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
    fn strip_check_wrapper_unwraps_single_paren_form() {
        assert_eq!(strip_check_wrapper("CHECK (VALUE > 0)"), "VALUE > 0");
    }

    #[test]
    fn strip_check_wrapper_unwraps_double_paren_form() {
        // pg_get_constraintdef sometimes emits `CHECK ((expr))`; we strip
        // exactly one layer, leaving inner parens for the parser to handle.
        assert_eq!(strip_check_wrapper("CHECK ((VALUE > 0))"), "(VALUE > 0)");
    }

    #[test]
    fn strip_check_wrapper_preserves_inner_function_parens() {
        // Stripping a single trailing `)` must not eat the closing paren
        // of an inner function call.
        assert_eq!(
            strip_check_wrapper("CHECK (length(x) > 0)"),
            "length(x) > 0",
        );
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
