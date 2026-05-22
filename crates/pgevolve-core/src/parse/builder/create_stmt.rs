//! `CREATE TABLE` → [`crate::ir::table::Table`].

#![allow(clippy::similar_names, clippy::missing_const_for_fn)]

use pg_query::NodeEnum;
use pg_query::protobuf::{
    self, ColumnDef, ConstrType, Constraint as PgConstraint, CreateStmt, PartitionBoundSpec,
    PartitionSpec,
};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column::{
    Column, Compression, Generated, GeneratedKind, Identity, IdentityKind, SequenceOptions,
    StorageKind,
};
use crate::ir::constraint::{
    Constraint, ConstraintKind, Deferrable, FkMatchType, ForeignKey, ReferentialAction,
};
use crate::ir::default_expr::DefaultExpr;
use crate::ir::partition::{
    BoundDatum, PartitionBounds, PartitionBy, PartitionColumn, PartitionColumnKind, PartitionOf,
    PartitionStrategy,
};
use crate::ir::table::Table;
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};
use crate::parse::normalize_expr;

/// Build a [`Table`] from a `CREATE TABLE` AST.
pub fn build_table(
    create: &CreateStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Table, ParseError> {
    let relation = create
        .relation
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CREATE TABLE missing relation".into(),
        })?;
    let qname = shared::resolve_qname(relation, default_schema, location)?;

    let mut columns: Vec<Column> = Vec::new();
    let mut constraints: Vec<Constraint> = Vec::new();
    let mut pk_columns: Option<Vec<Identifier>> = None;

    for elt in &create.table_elts {
        let Some(node) = elt.node.as_ref() else {
            continue;
        };
        match node {
            NodeEnum::ColumnDef(col) => {
                let (column, mut col_constraints, col_pk) =
                    build_column(col, &qname, default_schema, location)?;
                columns.push(column);
                constraints.append(&mut col_constraints);
                if let Some(c) = col_pk {
                    pk_columns = Some(vec![c]);
                }
            }
            NodeEnum::Constraint(con) => {
                if let Some(built) = build_table_constraint(con, &qname, default_schema, location)?
                {
                    if let ConstraintKind::PrimaryKey { columns: cols, .. } = &built.kind {
                        pk_columns = Some(cols.clone());
                    }
                    constraints.push(built);
                }
            }
            other => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: format!("unsupported table element: {}", node_kind_name(other)),
                });
            }
        }
    }

    // Apply implicit NOT NULL to columns covered by a PRIMARY KEY.
    if let Some(pk_cols) = pk_columns.as_ref() {
        for col in &mut columns {
            if pk_cols.contains(&col.name) {
                col.nullable = false;
            }
        }
    }

    // PARTITION BY <strategy> (<key-cols>) — marks this table as a partitioned parent.
    let partition_by = create
        .partspec
        .as_ref()
        .map(|spec| build_partition_by(spec, location))
        .transpose()?;

    // PARTITION OF <parent> FOR VALUES <bounds> — marks this as a partition child.
    let partition_of = create
        .partbound
        .as_ref()
        .map(|bound| {
            let parent = create.inh_relations.first().map_or_else(
                || {
                    Err(ParseError::Structural {
                        location: location.clone(),
                        message: "PARTITION OF requires exactly one parent".into(),
                    })
                },
                |node| match node.node.as_ref() {
                    Some(NodeEnum::RangeVar(rv)) => {
                        shared::resolve_qname(rv, default_schema, location)
                    }
                    _ => Err(ParseError::Structural {
                        location: location.clone(),
                        message: "PARTITION OF parent must be a RangeVar".into(),
                    }),
                },
            )?;
            if create.inh_relations.len() > 1 {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: "PARTITION OF accepts exactly one parent".into(),
                });
            }
            let bounds = build_partition_bounds(bound, location)?;
            Ok(PartitionOf { parent, bounds })
        })
        .transpose()?;

    Ok(Table {
        qname,
        columns,
        constraints,
        partition_by,
        partition_of,
        comment: None,
        owner: None,
        grants: vec![],
    })
}

/// Decode the inline `STORAGE x` clause from a `ColumnDef.storage_name` string.
///
/// `pg_query` populates `storage_name` (not `storage`) with the lowercase keyword
/// for inline `STORAGE x` in `CREATE TABLE / ALTER TABLE ADD COLUMN`; `storage`
/// is left empty. Returns `None` if no clause was written.
fn decode_inline_storage(
    storage_name: &str,
    location: &SourceLocation,
) -> Result<Option<StorageKind>, ParseError> {
    match storage_name.to_ascii_lowercase().as_str() {
        "" => Ok(None),
        "plain" => Ok(Some(StorageKind::Plain)),
        "external" => Ok(Some(StorageKind::External)),
        "extended" => Ok(Some(StorageKind::Extended)),
        "main" => Ok(Some(StorageKind::Main)),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unknown STORAGE attribute '{other}'"),
        }),
    }
}

/// Decode `COMPRESSION codec` from `ColumnDef.compression` (empty = unset).
fn decode_inline_compression(
    s: &str,
    location: &SourceLocation,
) -> Result<Option<Compression>, ParseError> {
    match s.to_ascii_lowercase().as_str() {
        "" => Ok(None),
        "pglz" => Ok(Some(Compression::Pglz)),
        "lz4" => Ok(Some(Compression::Lz4)),
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unknown COMPRESSION codec '{other}'"),
        }),
    }
}

/// Build a [`Column`] plus any inline-constraint [`Constraint`]s the column
/// declared. Returns `(column, table_constraints, pk_inline_column)`.
///
/// `pk_inline_column` is `Some(col_name)` if this column had `PRIMARY KEY`
/// inline — used to implicitly mark the column NOT NULL after the table is
/// fully assembled.
#[allow(clippy::too_many_lines)]
fn build_column(
    col: &ColumnDef,
    table_qname: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<(Column, Vec<Constraint>, Option<Identifier>), ParseError> {
    let name = shared::ident(&col.colname, location)?;
    let type_name = col
        .type_name
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: format!("column {} missing type", col.colname),
        })?;
    let ty = shared::type_name_to_column_type(type_name, location)?;

    let mut nullable = !col.is_not_null;
    let mut default: Option<DefaultExpr> = None;
    let mut identity: Option<Identity> = None;
    let mut generated: Option<Generated> = None;
    let comment: Option<String> = None;
    let mut produced_constraints: Vec<Constraint> = Vec::new();
    let mut pk_inline: Option<Identifier> = None;

    // Column-level default in raw_default (sometimes used) — fold into the
    // constraint scan below since most defaults arrive as ConstrDefault entries.
    if let Some(raw) = col.raw_default.as_ref()
        && let Some(node) = raw.node.as_ref()
    {
        default = Some(shared::build_default_expr(
            node,
            Some(&ty),
            default_schema,
            location,
        )?);
    }

    let collation = col
        .coll_clause
        .as_ref()
        .map(|cc| collate_clause_to_qname(cc, default_schema, location))
        .transpose()?;

    // Inline STORAGE / COMPRESSION clauses (PG 16+ for STORAGE inline in CREATE TABLE;
    // COMPRESSION inline is PG 14+). pg_query populates `storage_name` (not `storage`)
    // for the inline STORAGE keyword; `compression` is populated directly.
    let storage = decode_inline_storage(&col.storage_name, location)?;
    let compression = decode_inline_compression(&col.compression, location)?;

    for c in &col.constraints {
        let Some(NodeEnum::Constraint(con)) = c.node.as_ref() else {
            continue;
        };
        let kind = ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined);
        match kind {
            ConstrType::ConstrNotnull => {
                nullable = false;
            }
            ConstrType::ConstrNull => {
                nullable = true;
            }
            ConstrType::ConstrDefault => {
                if let Some(raw) = con.raw_expr.as_ref()
                    && let Some(node) = raw.node.as_ref()
                {
                    default = Some(shared::build_default_expr(
                        node,
                        Some(&ty),
                        default_schema,
                        location,
                    )?);
                }
            }
            ConstrType::ConstrIdentity => {
                identity = Some(Identity {
                    kind: parse_identity_kind(&con.generated_when),
                    sequence: SequenceOptions {
                        start: 1,
                        increment: 1,
                        min_value: None,
                        max_value: None,
                        cache: 1,
                        cycle: false,
                    },
                });
                nullable = false;
            }
            ConstrType::ConstrGenerated => {
                if let Some(raw) = con.raw_expr.as_ref()
                    && let Some(node) = raw.node.as_ref()
                {
                    let expr = normalize_expr::from_pg_node(node, None, location)?;
                    generated = Some(Generated {
                        kind: GeneratedKind::Stored,
                        expression: expr,
                    });
                }
            }
            ConstrType::ConstrPrimary => {
                pk_inline = Some(name.clone());
                nullable = false;
                produced_constraints.push(make_pk_constraint(
                    table_qname,
                    &con.conname,
                    vec![name.clone()],
                    location,
                )?);
            }
            ConstrType::ConstrUnique => {
                produced_constraints.push(make_unique_constraint(
                    table_qname,
                    con,
                    vec![name.clone()],
                    location,
                )?);
            }
            ConstrType::ConstrForeign => {
                produced_constraints.push(make_fk_constraint(
                    table_qname,
                    con,
                    vec![name.clone()],
                    default_schema,
                    location,
                )?);
            }
            ConstrType::ConstrCheck => {
                produced_constraints.push(make_check_constraint(table_qname, con, location)?);
            }
            _ => {}
        }
    }

    Ok((
        Column {
            name,
            ty,
            nullable,
            default,
            identity,
            generated,
            collation,
            storage,
            compression,
            comment,
        },
        produced_constraints,
        pk_inline,
    ))
}

/// Build a table-level [`Constraint`] from a standalone `CONSTRAINT ...` clause
/// in the table-elements list. Returns `None` if the constraint kind is not
/// supported for table-level use (e.g., `NOT NULL` standalone — not legal
/// syntax anyway).
fn build_table_constraint(
    con: &PgConstraint,
    table_qname: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Option<Constraint>, ParseError> {
    let kind = ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined);
    let cols = key_idents(&con.keys, location)?;
    Ok(Some(match kind {
        ConstrType::ConstrPrimary => make_pk_constraint(table_qname, &con.conname, cols, location)?,
        ConstrType::ConstrUnique => make_unique_constraint(table_qname, con, cols, location)?,
        ConstrType::ConstrForeign => {
            let fk_cols = key_idents(&con.fk_attrs, location)?;
            make_fk_constraint(table_qname, con, fk_cols, default_schema, location)?
        }
        ConstrType::ConstrCheck => make_check_constraint(table_qname, con, location)?,
        _ => return Ok(None),
    }))
}

fn make_pk_constraint(
    table_qname: &QualifiedName,
    conname: &str,
    columns: Vec<Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, conname, "pkey", location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::PrimaryKey {
            columns,
            include: vec![],
        },
        deferrable: Deferrable::NotDeferrable,
        comment: None,
    })
}

fn make_unique_constraint(
    table_qname: &QualifiedName,
    con: &PgConstraint,
    columns: Vec<Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, &con.conname, "key", location)?;
    let include = key_idents(&con.including, location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::Unique {
            columns,
            include,
            nulls_distinct: !con.nulls_not_distinct,
        },
        deferrable: deferrable_from(con),
        comment: None,
    })
}

/// Build a FOREIGN KEY [`Constraint`] from an `ALTER TABLE ADD CONSTRAINT FK`.
///
/// The local columns come from `con.fk_attrs` (an inline column-spec list is
/// what populates them at parse time), so we forward straight into
/// `make_fk_constraint` after extracting them.
pub fn build_fk_for_alter(
    con: &PgConstraint,
    target_table: &QualifiedName,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let fk_cols = key_idents(&con.fk_attrs, location)?;
    make_fk_constraint(target_table, con, fk_cols, default_schema, location)
}

fn make_fk_constraint(
    table_qname: &QualifiedName,
    con: &PgConstraint,
    fk_cols: Vec<Identifier>,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, &con.conname, "fkey", location)?;
    let pk_cols = key_idents(&con.pk_attrs, location)?;
    let pktable = con.pktable.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "FOREIGN KEY missing referenced table".into(),
    })?;
    let referenced_table = shared::resolve_qname(pktable, default_schema, location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::ForeignKey(ForeignKey {
            columns: fk_cols,
            referenced_table,
            referenced_columns: pk_cols,
            on_update: parse_referential_action(&con.fk_upd_action),
            on_delete: parse_referential_action(&con.fk_del_action),
            match_type: parse_match_type(&con.fk_matchtype),
        }),
        deferrable: deferrable_from(con),
        comment: None,
    })
}

fn make_check_constraint(
    table_qname: &QualifiedName,
    con: &PgConstraint,
    location: &SourceLocation,
) -> Result<Constraint, ParseError> {
    let qname = constraint_qname(table_qname, &con.conname, "check", location)?;
    let raw = con
        .raw_expr
        .as_ref()
        .and_then(|r| r.node.as_ref())
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CHECK constraint missing expression".into(),
        })?;
    let expression = normalize_expr::from_pg_node(raw, None, location)?;
    Ok(Constraint {
        qname,
        kind: ConstraintKind::Check {
            expression,
            no_inherit: con.is_no_inherit,
        },
        deferrable: deferrable_from(con),
        comment: None,
    })
}

fn deferrable_from(con: &PgConstraint) -> Deferrable {
    if con.deferrable {
        Deferrable::Deferrable {
            initially_deferred: con.initdeferred,
        }
    } else {
        Deferrable::NotDeferrable
    }
}

fn constraint_qname(
    table_qname: &QualifiedName,
    conname: &str,
    kind_suffix: &str,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let name = if conname.is_empty() {
        format!("{}_{}", table_qname.name.as_str(), kind_suffix)
    } else {
        conname.to_string()
    };
    Ok(QualifiedName::new(
        table_qname.schema.clone(),
        shared::ident(&name, location)?,
    ))
}

fn key_idents(
    nodes: &[protobuf::Node],
    location: &SourceLocation,
) -> Result<Vec<Identifier>, ParseError> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let Some(NodeEnum::String(s)) = n.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: "expected identifier in constraint key list".into(),
            });
        };
        out.push(shared::ident(&s.sval, location)?);
    }
    Ok(out)
}

fn collate_clause_to_qname(
    cc: &protobuf::CollateClause,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<QualifiedName, ParseError> {
    let parts: Vec<&str> = cc
        .collname
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.as_str()),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        [name] => {
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "COLLATE name must be schema-qualified or carry a directive".into(),
                })?;
            Ok(QualifiedName::new(schema, shared::ident(name, location)?))
        }
        [schema, name] => Ok(QualifiedName::new(
            shared::ident(schema, location)?,
            shared::ident(name, location)?,
        )),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "COLLATE name must have one or two components".into(),
        }),
    }
}

fn parse_identity_kind(generated_when: &str) -> IdentityKind {
    // Postgres encodes `ALWAYS` as "a" and `BY DEFAULT` as "d" in this string.
    match generated_when {
        "a" => IdentityKind::Always,
        _ => IdentityKind::ByDefault,
    }
}

fn parse_referential_action(s: &str) -> ReferentialAction {
    match s {
        "r" => ReferentialAction::Restrict,
        "c" => ReferentialAction::Cascade,
        "n" => ReferentialAction::SetNull(vec![]),
        "d" => ReferentialAction::SetDefault(vec![]),
        // "a" or empty → NO ACTION (default).
        _ => ReferentialAction::NoAction,
    }
}

fn parse_match_type(s: &str) -> FkMatchType {
    if s.eq_ignore_ascii_case("f") {
        FkMatchType::Full
    } else {
        FkMatchType::Simple
    }
}

fn node_kind_name(node: &NodeEnum) -> &'static str {
    match node {
        NodeEnum::ColumnDef(_) => "column definition",
        NodeEnum::Constraint(_) => "constraint",
        NodeEnum::TableLikeClause(_) => "LIKE clause",
        _ => "table element",
    }
}

// ---------------------------------------------------------------------------
// Partitioning helpers
// ---------------------------------------------------------------------------

/// Build a [`PartitionBy`] from a `PartitionSpec` node.
///
/// `pub(crate)` so PART3 (ATTACH PARTITION builder) and PART4 (catalog reader)
/// can call it directly without re-parsing the node.
pub(crate) fn build_partition_by(
    spec: &PartitionSpec,
    location: &SourceLocation,
) -> Result<PartitionBy, ParseError> {
    use pg_query::protobuf::PartitionStrategy as PgStrategy;

    let pg_strategy = PgStrategy::try_from(spec.strategy).unwrap_or(PgStrategy::Undefined);
    let strategy = match pg_strategy {
        PgStrategy::Range => PartitionStrategy::Range,
        PgStrategy::List => PartitionStrategy::List,
        PgStrategy::Hash => PartitionStrategy::Hash,
        PgStrategy::Undefined => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("unknown partition strategy value {}", spec.strategy),
            });
        }
    };

    let mut columns = Vec::new();
    for part_elem_node in &spec.part_params {
        let Some(NodeEnum::PartitionElem(elem)) = part_elem_node.node.as_ref() else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: "PARTITION BY entry was not a PartitionElem".into(),
            });
        };
        let kind = if !elem.name.is_empty() {
            PartitionColumnKind::Column(shared::ident(&elem.name, location)?)
        } else if let Some(expr_node) = elem.expr.as_ref() {
            let inner = expr_node
                .node
                .as_ref()
                .ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "PartitionElem expr node has no inner node".into(),
                })?;
            PartitionColumnKind::Expr(normalize_expr::from_pg_node(inner, None, location)?)
        } else {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: "PartitionElem had neither name nor expr".into(),
            });
        };
        let collation = qualified_name_from_node_list(&elem.collation, location)?;
        let opclass = qualified_name_from_node_list(&elem.opclass, location)?;
        columns.push(PartitionColumn {
            kind,
            collation,
            opclass,
        });
    }

    if columns.is_empty() {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "PARTITION BY had no columns".into(),
        });
    }
    if matches!(strategy, PartitionStrategy::Hash) && columns.len() != 1 {
        return Err(ParseError::Structural {
            location: location.clone(),
            message: "HASH partition strategy supports exactly one column".into(),
        });
    }

    Ok(PartitionBy { strategy, columns })
}

/// Build a [`PartitionBounds`] from a `PartitionBoundSpec` node.
///
/// `pub(crate)` so PART3 and PART4 can call it without re-parsing.
pub(crate) fn build_partition_bounds(
    spec: &PartitionBoundSpec,
    location: &SourceLocation,
) -> Result<PartitionBounds, ParseError> {
    if spec.is_default {
        return Ok(PartitionBounds::Default);
    }

    match spec.strategy.as_str() {
        "h" | "HASH" | "hash" => {
            if spec.modulus < 1 {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: "HASH partition modulus must be >= 1".into(),
                });
            }
            if spec.remainder < 0 || spec.remainder >= spec.modulus {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: "HASH partition remainder out of range".into(),
                });
            }
            Ok(PartitionBounds::Hash {
                modulus: u32::try_from(spec.modulus).map_err(|_| ParseError::Structural {
                    location: location.clone(),
                    message: "modulus did not fit in u32".into(),
                })?,
                remainder: u32::try_from(spec.remainder).map_err(|_| ParseError::Structural {
                    location: location.clone(),
                    message: "remainder did not fit in u32".into(),
                })?,
            })
        }
        "l" | "LIST" | "list" => {
            let values = spec
                .listdatums
                .iter()
                .map(|n| build_bound_datum(n, location))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PartitionBounds::List { values })
        }
        "r" | "RANGE" | "range" => {
            let from = spec
                .lowerdatums
                .iter()
                .map(|n| build_bound_datum(n, location))
                .collect::<Result<Vec<_>, _>>()?;
            let to = spec
                .upperdatums
                .iter()
                .map(|n| build_bound_datum(n, location))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PartitionBounds::Range { from, to })
        }
        other => Err(ParseError::Structural {
            location: location.clone(),
            message: format!("unknown partition bound strategy {other:?}"),
        }),
    }
}

fn build_bound_datum(
    node: &pg_query::protobuf::Node,
    location: &SourceLocation,
) -> Result<BoundDatum, ParseError> {
    use pg_query::protobuf::PartitionRangeDatumKind;

    if let Some(NodeEnum::PartitionRangeDatum(d)) = node.node.as_ref() {
        let kind =
            PartitionRangeDatumKind::try_from(d.kind).unwrap_or(PartitionRangeDatumKind::Undefined);
        return match kind {
            PartitionRangeDatumKind::PartitionRangeDatumValue => {
                let v = d.value.as_ref().ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "PartitionRangeDatum kind=value but value is None".into(),
                })?;
                let inner = v.node.as_ref().ok_or_else(|| ParseError::Structural {
                    location: location.clone(),
                    message: "PartitionRangeDatum value node has no inner node".into(),
                })?;
                Ok(BoundDatum::Literal(normalize_expr::from_pg_node(
                    inner, None, location,
                )?))
            }
            PartitionRangeDatumKind::PartitionRangeDatumMinvalue => Ok(BoundDatum::MinValue),
            PartitionRangeDatumKind::PartitionRangeDatumMaxvalue => Ok(BoundDatum::MaxValue),
            PartitionRangeDatumKind::Undefined => Err(ParseError::Structural {
                location: location.clone(),
                message: format!("unknown PartitionRangeDatumKind {}", d.kind),
            }),
        };
    }

    // Fallback: treat it as a literal expression (e.g., a raw AConst or FuncCall
    // in a LIST partition).
    let inner = node.node.as_ref().ok_or_else(|| ParseError::Structural {
        location: location.clone(),
        message: "bound datum node has no inner node".into(),
    })?;
    Ok(BoundDatum::Literal(normalize_expr::from_pg_node(
        inner, None, location,
    )?))
}

/// Build an optional [`QualifiedName`] from a repeated `Node` list that should
/// contain one or two `String` nodes (e.g. collation or opclass overrides in a
/// `PartitionElem`).
fn qualified_name_from_node_list(
    nodes: &[pg_query::protobuf::Node],
    location: &SourceLocation,
) -> Result<Option<QualifiedName>, ParseError> {
    if nodes.is_empty() {
        return Ok(None);
    }
    let mut parts: Vec<String> = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node.node.as_ref() {
            Some(NodeEnum::String(s)) => parts.push(s.sval.clone()),
            _ => {
                return Err(ParseError::Structural {
                    location: location.clone(),
                    message: "expected String node in qualified name list".into(),
                });
            }
        }
    }
    let (schema, name) = match parts.len() {
        1 => {
            // A bare name (e.g. `text_ops`) — treat schema as "public" since opclass
            // names without a schema are resolved against search_path at runtime;
            // we store the name so the diff engine can compare them.
            ("public".to_string(), parts.remove(0))
        }
        2 => {
            // SAFETY: parts.len() == 2, so both pops yield Some.
            let n = parts
                .pop()
                .unwrap_or_else(|| unreachable!("len==2, first pop"));
            let s = parts
                .pop()
                .unwrap_or_else(|| unreachable!("len==2, second pop"));
            (s, n)
        }
        _ => {
            return Err(ParseError::Structural {
                location: location.clone(),
                message: format!("qualified name list had unexpected length {}", nodes.len()),
            });
        }
    };
    Ok(Some(QualifiedName::new(
        shared::ident(&schema, location)?,
        shared::ident(&name, location)?,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column_type::ColumnType;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn build(sql: &str) -> Table {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!("not CreateStmt")
        };
        build_table(&create, None, &loc()).expect("builds")
    }

    #[test]
    fn single_column_table() {
        let t = build("CREATE TABLE app.users (id integer);");
        assert_eq!(t.qname.to_string(), "app.users");
        assert_eq!(t.columns.len(), 1);
        assert_eq!(t.columns[0].name.as_str(), "id");
        assert_eq!(t.columns[0].ty, ColumnType::Integer);
        assert!(t.columns[0].nullable);
    }

    #[test]
    fn not_null_flips_nullable() {
        let t = build("CREATE TABLE app.t (a integer NOT NULL);");
        assert!(!t.columns[0].nullable);
    }

    #[test]
    fn inline_pk_implies_not_null_and_constraint() {
        let t = build("CREATE TABLE app.t (id integer PRIMARY KEY);");
        assert!(!t.columns[0].nullable);
        assert_eq!(t.constraints.len(), 1);
        assert!(matches!(
            t.constraints[0].kind,
            ConstraintKind::PrimaryKey { .. }
        ));
    }

    #[test]
    fn composite_pk_table_constraint() {
        let t = build("CREATE TABLE app.t (a integer, b integer, PRIMARY KEY (a, b));");
        assert_eq!(t.constraints.len(), 1);
        if let ConstraintKind::PrimaryKey { columns, .. } = &t.constraints[0].kind {
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[0].as_str(), "a");
            assert_eq!(columns[1].as_str(), "b");
        } else {
            panic!()
        }
        // PK columns implicitly NOT NULL.
        assert!(!t.columns[0].nullable);
        assert!(!t.columns[1].nullable);
    }

    #[test]
    fn inline_references() {
        let t = build(
            "CREATE TABLE app.t (org_id integer REFERENCES app.orgs (id) ON DELETE CASCADE);",
        );
        assert_eq!(t.constraints.len(), 1);
        if let ConstraintKind::ForeignKey(fk) = &t.constraints[0].kind {
            assert_eq!(fk.referenced_table.to_string(), "app.orgs");
            assert!(matches!(fk.on_delete, ReferentialAction::Cascade));
        } else {
            panic!("expected ForeignKey")
        }
    }

    #[test]
    fn inline_check() {
        let t = build("CREATE TABLE app.t (n integer CHECK (n > 0));");
        assert_eq!(t.constraints.len(), 1);
        assert!(matches!(
            t.constraints[0].kind,
            ConstraintKind::Check { .. }
        ));
    }

    #[test]
    fn default_now_is_expr() {
        let t = build("CREATE TABLE app.t (created_at timestamp DEFAULT now());");
        match &t.columns[0].default {
            Some(DefaultExpr::Expr(e)) => assert!(e.canonical_text.to_lowercase().contains("now")),
            other => panic!("expected Expr default, got {other:?}"),
        }
    }

    #[test]
    fn default_integer_literal() {
        let t = build("CREATE TABLE app.t (n integer DEFAULT 0);");
        assert!(matches!(
            t.columns[0].default,
            Some(DefaultExpr::Literal(
                crate::ir::default_expr::LiteralValue::Integer(0)
            ))
        ));
    }

    #[test]
    fn default_nextval_is_sequence() {
        let t = build("CREATE TABLE app.t (id integer DEFAULT nextval('app.seq1'));");
        match &t.columns[0].default {
            Some(DefaultExpr::Sequence(q)) => assert_eq!(q.to_string(), "app.seq1"),
            other => panic!("expected Sequence default, got {other:?}"),
        }
    }

    #[test]
    fn directive_default_schema_used() {
        let parsed = pg_query::parse("CREATE TABLE users (id integer);").unwrap();
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .unwrap();
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!()
        };
        let app = Identifier::from_unquoted("app").unwrap();
        let t = build_table(&create, Some(&app), &loc()).unwrap();
        assert_eq!(t.qname.to_string(), "app.users");
    }

    #[test]
    fn unqualified_without_directive_errors() {
        let parsed = pg_query::parse("CREATE TABLE users (id integer);").unwrap();
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|r| r.stmt)
            .and_then(|n| n.node)
            .unwrap();
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!()
        };
        let err = build_table(&create, None, &loc()).unwrap_err();
        assert!(matches!(err, ParseError::UnqualifiedName { .. }));
    }

    // ------------------------------------------------------------------
    // Helper used by the partition tests below.
    // ------------------------------------------------------------------

    fn try_build(sql: &str) -> Result<Table, ParseError> {
        let parsed = pg_query::parse(sql).expect("pg_query parse");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateStmt(create) = stmt else {
            panic!("not CreateStmt")
        };
        build_table(&create, None, &loc())
    }

    // ------------------------------------------------------------------
    // Partitioning tests
    // ------------------------------------------------------------------

    #[test]
    fn parses_partition_by_list() {
        let t = build(
            "CREATE TABLE app.orders \
             (id bigint NOT NULL, region text NOT NULL) \
             PARTITION BY LIST (region);",
        );
        let pb = t.partition_by.expect("partition_by should be Some");
        assert!(
            matches!(pb.strategy, PartitionStrategy::List),
            "expected List strategy"
        );
        assert_eq!(pb.columns.len(), 1);
        assert!(
            matches!(&pb.columns[0].kind, PartitionColumnKind::Column(id) if id.as_str() == "region")
        );
    }

    #[test]
    fn parses_partition_of_range() {
        let t = build(
            "CREATE TABLE app.orders_2024 \
             PARTITION OF app.orders \
             FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');",
        );
        let po = t.partition_of.expect("partition_of should be Some");
        assert_eq!(po.parent.name.as_str(), "orders");
        assert_eq!(po.parent.schema.as_str(), "app");
        assert!(
            matches!(po.bounds, PartitionBounds::Range { .. }),
            "expected Range bounds"
        );
    }

    #[test]
    fn rejects_hash_with_two_columns() {
        let err =
            try_build("CREATE TABLE app.t (a int, b int) PARTITION BY HASH (a, b);").unwrap_err();
        assert!(
            matches!(err, ParseError::Structural { .. }),
            "expected Structural error, got {err:?}"
        );
    }

    #[test]
    fn parses_default_partition() {
        let t = build(
            "CREATE TABLE app.orders_default \
             PARTITION OF app.orders DEFAULT;",
        );
        let po = t.partition_of.expect("partition_of should be Some");
        assert!(
            matches!(po.bounds, PartitionBounds::Default),
            "expected Default bounds"
        );
    }

    #[test]
    fn parses_hash_bound() {
        let t = build(
            "CREATE TABLE app.t0 \
             PARTITION OF app.t \
             FOR VALUES WITH (MODULUS 4, REMAINDER 0);",
        );
        match t.partition_of.unwrap().bounds {
            PartitionBounds::Hash { modulus, remainder } => {
                assert_eq!(modulus, 4);
                assert_eq!(remainder, 0);
            }
            other => panic!("expected Hash bounds, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // Inline STORAGE / COMPRESSION in CREATE TABLE (PG 14+/16+)
    // ------------------------------------------------------------------

    #[test]
    fn inline_storage_external() {
        let t = build("CREATE TABLE app.t (doc text STORAGE EXTERNAL);");
        assert_eq!(t.columns[0].storage, Some(StorageKind::External));
        assert_eq!(t.columns[0].compression, None);
    }

    #[test]
    fn inline_storage_plain() {
        let t = build("CREATE TABLE app.t (n integer STORAGE PLAIN);");
        assert_eq!(t.columns[0].storage, Some(StorageKind::Plain));
    }

    #[test]
    fn inline_storage_extended() {
        let t = build("CREATE TABLE app.t (doc text STORAGE EXTENDED);");
        assert_eq!(t.columns[0].storage, Some(StorageKind::Extended));
    }

    #[test]
    fn inline_storage_main() {
        let t = build("CREATE TABLE app.t (doc text STORAGE MAIN);");
        assert_eq!(t.columns[0].storage, Some(StorageKind::Main));
    }

    #[test]
    fn inline_compression_lz4() {
        let t = build("CREATE TABLE app.t (blob bytea COMPRESSION lz4);");
        assert_eq!(t.columns[0].storage, None);
        assert_eq!(t.columns[0].compression, Some(Compression::Lz4));
    }

    #[test]
    fn inline_compression_pglz() {
        let t = build("CREATE TABLE app.t (blob bytea COMPRESSION pglz);");
        assert_eq!(t.columns[0].compression, Some(Compression::Pglz));
    }

    #[test]
    fn inline_storage_and_compression_together() {
        let t = build("CREATE TABLE app.t (doc text STORAGE EXTERNAL COMPRESSION lz4);");
        assert_eq!(t.columns[0].storage, Some(StorageKind::External));
        assert_eq!(t.columns[0].compression, Some(Compression::Lz4));
    }

    #[test]
    fn no_storage_no_compression_leaves_none() {
        let t = build("CREATE TABLE app.t (n integer);");
        assert_eq!(t.columns[0].storage, None);
        assert_eq!(t.columns[0].compression, None);
    }
}
