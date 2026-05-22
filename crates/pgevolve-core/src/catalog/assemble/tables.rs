//! Table, column, constraint, index, sequence, and dependency assembly.
//!
//! Called from [`super::assemble`] to build the table-family IR from raw
//! catalog rows.

use std::collections::HashMap;
use std::path::PathBuf;

use pg_query::NodeEnum;

use crate::catalog::CatalogQuery;
use crate::catalog::DriftReport;
use crate::catalog::error::CatalogError;
use crate::catalog::filter::CatalogFilter;
use crate::catalog::rows::Row;
use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column::{
    Column, Compression, Generated, GeneratedKind, Identity, IdentityKind, SequenceOptions,
    StorageKind,
};
use crate::ir::column_type::ColumnType;
use crate::ir::constraint::{Constraint, ConstraintKind, Deferrable, ForeignKey};
use crate::ir::default_expr::{DefaultExpr, NormalizedExpr};
use crate::ir::index::{Index, IndexParent};
use crate::ir::schema::Schema;
use crate::ir::sequence::{Sequence, SequenceOwner};
use crate::ir::table::Table;
use crate::parse::builder;
use crate::parse::error::SourceLocation;

use super::{
    ident_required, parse_check_expression, parse_fk_referenced_columns, parse_match_type,
    parse_referential_action, qname_from,
};

/// Decode `pg_attribute.attstorage` single-char text into
/// [`Option<StorageKind>`].
///
/// Postgres stores `'p'`, `'e'`, `'x'`, or `'m'`. The decoder wraps each
/// known value in `Some(…)` so that the call site receives the `Option`
/// directly without a redundant `Some(…)` wrap. Any other value is a catalog
/// error surfaced as [`CatalogError::BadColumnType`].
fn decode_attstorage(raw: &str) -> Result<Option<StorageKind>, CatalogError> {
    match raw {
        "p" => Ok(Some(StorageKind::Plain)),
        "e" => Ok(Some(StorageKind::External)),
        "x" => Ok(Some(StorageKind::Extended)),
        "m" => Ok(Some(StorageKind::Main)),
        other => Err(CatalogError::BadColumnType {
            query: CatalogQuery::Columns,
            column: "attstorage".to_string(),
            message: format!("unexpected attstorage value {other:?} (expected p/e/x/m)"),
        }),
    }
}

/// Decode `pg_attribute.attcompression` single-char text into
/// [`Option<Compression>`].
///
/// `'\0'` (empty string after the `::text` cast) or any unrecognised char
/// means "use the cluster default" → [`None`]. `'p'` = pglz, `'l'` = lz4.
/// The empty-string case covers the null-char that Postgres stores when no
/// explicit codec has been set. Unknown chars are treated as `None` so the
/// reader remains forward-compatible with future PG codecs.
fn decode_attcompression(raw: &str) -> Option<Compression> {
    match raw {
        "p" => Some(Compression::Pglz),
        "l" => Some(Compression::Lz4),
        // Empty string is how '\0' appears after ::text cast (cluster default).
        // Any other unrecognised char is also treated as cluster default so
        // we stay forward-compatible with future codecs.
        _ => None,
    }
}

pub(super) fn build_schemas(
    rows: &[Row],
    filter: &CatalogFilter,
) -> Result<Vec<Schema>, CatalogError> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let q = CatalogQuery::Schemas;
        let name = Identifier::from_unquoted(&r.get_text(q, "name")?)
            .map_err(|e| CatalogError::Ir(crate::ir::IrError::InvalidIdentifier(e.to_string())))?;
        if !filter.includes_schema(&name) {
            continue;
        }
        let owner_str = r.get_text(q, "owner")?;
        let owner_ident =
            Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::BadColumnType {
                query: q,
                column: "owner".to_string(),
                message: format!("invalid owner {owner_str:?}: {e}"),
            })?;
        let acl_strings = r.get_text_array(q, "acl")?;
        let raw_grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;
        let grants = crate::catalog::grants::strip_owner_self_grants(raw_grants, &owner_ident);
        let owner = Some(owner_ident);
        out.push(Schema {
            name,
            comment: r.get_opt_text(q, "comment")?,
            owner,
            grants,
        });
    }
    Ok(out)
}

pub(super) fn build_tables(
    table_rows: Vec<Row>,
    column_rows: &[Row],
    filter: &CatalogFilter,
) -> Result<HashMap<i64, Table>, CatalogError> {
    let mut tables: HashMap<i64, Table> = HashMap::with_capacity(table_rows.len());

    for r in table_rows {
        let q = CatalogQuery::Tables;
        let oid = r.get_int(q, "oid")?;
        let qname = qname_from(&r, q, "schema", "name")?;
        if !filter.allows(&qname) {
            continue;
        }
        let comment = r.get_opt_text(q, "comment")?;
        let owner_str = r.get_text(q, "owner")?;
        let owner_ident =
            Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::BadColumnType {
                query: q,
                column: "owner".to_string(),
                message: format!("invalid owner {owner_str:?}: {e}"),
            })?;
        let acl_strings = r.get_text_array(q, "acl")?;
        let raw_grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;
        let grants = crate::catalog::grants::strip_owner_self_grants(raw_grants, &owner_ident);
        let owner = Some(owner_ident);
        tables.insert(
            oid,
            Table {
                qname,
                columns: vec![],
                constraints: vec![],
                partition_by: None,
                partition_of: None,
                comment,
                owner,
                grants,
            },
        );
    }

    // Attach columns by oid, in attnum order. Column rows are already ordered
    // by (schema, table, attnum) in the SQL.
    // Also collect column-level ACLs from attacl and append them to the table's
    // grants list with the column name set.
    for cr in column_rows {
        let table_oid = cr.get_int(CatalogQuery::Columns, "table_oid")?;
        let Some(table) = tables.get_mut(&table_oid) else {
            continue;
        };
        let column = build_column(cr)?;
        // Decode column-level ACL entries and attach the column name.
        // Strip owner self-grants from attacl for the same reason as relacl.
        let col_acl_strings = cr.get_text_array(CatalogQuery::Columns, "attacl")?;
        if !col_acl_strings.is_empty() {
            let raw_col_grants = crate::catalog::grants::decode_aclitem_array(&col_acl_strings)?;
            let col_grants = if let Some(owner) = table.owner.as_ref() {
                crate::catalog::grants::strip_owner_self_grants(raw_col_grants, owner)
            } else {
                raw_col_grants
            };
            for mut g in col_grants {
                g.columns = Some(vec![column.name.clone()]);
                table.grants.push(g);
            }
        }
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

    let attstorage = r.get_text(q, "attstorage")?;
    let storage = decode_attstorage(&attstorage)?;

    let attcompression = r.get_text(q, "attcompression")?;
    let compression = decode_attcompression(&attcompression);

    Ok(Column {
        name,
        ty,
        nullable: !not_null,
        default,
        identity,
        generated,
        collation,
        storage,
        compression,
        comment,
    })
}

pub(super) fn apply_constraints(
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
                .ok_or_else(|| CatalogError::DanglingReference {
                    kind: "constraint table oid",
                    what: oid.to_string(),
                })?
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
        .map(|i| {
            // `col` followed by a decimal integer produces only ASCII alphanumeric
            // characters, which always satisfies `Identifier::from_unquoted`.
            Identifier::from_unquoted(&format!("col{i}"))
                .unwrap_or_else(|e| unreachable!("'col{{i}}' is always a valid identifier: {e}"))
        })
        .collect()
}

pub(super) fn build_indexes(
    rows: &[Row],
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

pub(super) fn build_sequence(
    r: &Row,
    filter: &CatalogFilter,
) -> Result<Option<Sequence>, CatalogError> {
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

    let owner_str = r.get_text(q, "owner")?;
    let owner_ident =
        Identifier::from_unquoted(&owner_str).map_err(|e| CatalogError::BadColumnType {
            query: q,
            column: "owner".to_string(),
            message: format!("invalid owner {owner_str:?}: {e}"),
        })?;
    let acl_strings = r.get_text_array(q, "acl")?;
    let raw_grants = crate::catalog::grants::decode_aclitem_array(&acl_strings)?;
    let grants = crate::catalog::grants::strip_owner_self_grants(raw_grants, &owner_ident);
    let owner = Some(owner_ident);

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
        owner,
        grants,
    }))
}

pub(super) fn apply_dependencies(
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

/// Parse a default-expression text from `pg_get_expr`. Recognizes `nextval` as
/// [`DefaultExpr::Sequence`] and bare literals as [`DefaultExpr::Literal`];
/// anything else becomes [`DefaultExpr::Expr`].
pub(super) fn parse_default_expr_text(
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

/// Re-parse a SQL expression text by wrapping it in `SELECT (…) AS x` and
/// extracting the resulting expression node, then normalizing it.
///
/// This is a local alias — the canonical implementation lives in
/// [`super::reparse_expression_text`]. We keep it here so `build_column` can
/// call it without a cross-module path.
pub(super) fn reparse_expression_text(text: &str) -> Result<NormalizedExpr, CatalogError> {
    super::reparse_expression_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;

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
