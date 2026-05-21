//! `SERIAL` desugaring.
//!
//! Replaces a `serial`-family column with `(integer NOT NULL DEFAULT nextval('<seq>'),
//! CREATE SEQUENCE <seq> OWNED BY ...)`.
//!
//! The catalog reader (phase 3) walks `pg_class` + `pg_attribute` + `pg_depend`
//! and produces the desugared form natively, so source files using `serial` and
//! source files using the explicit `integer + sequence + default` pair must
//! produce identical IR. That equality is exercised by the Tier-2 corpus.
//!
//! Postgres's exact naming convention for the synthesized sequence is
//! `<table>_<column>_seq` in the table's schema. We match that.

#![allow(clippy::similar_names)]

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::DefaultExpr;
use crate::ir::sequence::{Sequence, SequenceOwner};
use crate::ir::table::Table;
use crate::parse::error::{ParseError, SourceLocation};

/// Desugar every `serial`-family column in `table`.
///
/// For each `serial` column: replace the type with the matching integer width,
/// flip nullable to false, attach a `nextval(<seq>)` default, and synthesize
/// one [`Sequence`] per column. The returned vector is what the caller appends
/// to the catalog's sequence list.
pub fn desugar_serials_in_table(
    table: &mut Table,
    location: &SourceLocation,
) -> Result<Vec<Sequence>, ParseError> {
    let mut produced: Vec<Sequence> = Vec::new();
    let table_qname = table.qname.clone();
    for col in &mut table.columns {
        let raw = match &col.ty {
            ColumnType::Other { raw } => raw.to_ascii_lowercase(),
            _ => continue,
        };
        let Some(integer_ty) = serial_to_integer(&raw) else {
            continue;
        };

        // Rewrite column.
        col.ty = integer_ty.clone();
        col.nullable = false;
        let seq_name = format!("{}_{}_seq", table_qname.name.as_str(), col.name.as_str());
        let seq_qname = QualifiedName::new(
            table_qname.schema.clone(),
            Identifier::from_unquoted(&seq_name).map_err(|e| ParseError::Ir {
                location: location.clone(),
                source: crate::ir::IrError::InvalidIdentifier(e.to_string()),
            })?,
        );
        col.default = Some(DefaultExpr::Sequence(seq_qname.clone()));

        // Produce sequence.
        produced.push(Sequence {
            qname: seq_qname,
            data_type: integer_ty,
            start: 1,
            increment: 1,
            min_value: None,
            max_value: None,
            cache: 1,
            cycle: false,
            owned_by: Some(SequenceOwner {
                table: table_qname.clone(),
                column: col.name.clone(),
            }),
            comment: None,
        });
    }
    Ok(produced)
}

fn serial_to_integer(raw: &str) -> Option<ColumnType> {
    match raw {
        "serial" | "serial4" => Some(ColumnType::Integer),
        "bigserial" | "serial8" => Some(ColumnType::BigInt),
        "smallserial" | "serial2" => Some(ColumnType::SmallInt),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::column::Column;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(name: &str) -> QualifiedName {
        QualifiedName::new(id("app"), id(name))
    }

    fn table_with_serial(raw_type: &str) -> Table {
        Table {
            qname: qn("users"),
            columns: vec![Column {
                name: id("id"),
                ty: ColumnType::Other {
                    raw: raw_type.into(),
                },
                nullable: true,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                comment: None,
            }],
            constraints: vec![],
                        partition_by: None,
            partition_of: None,
comment: None,
        }
    }

    #[test]
    fn desugars_serial_to_integer() {
        let mut t = table_with_serial("serial");
        let seqs = desugar_serials_in_table(&mut t, &loc()).unwrap();
        assert_eq!(t.columns[0].ty, ColumnType::Integer);
        assert!(!t.columns[0].nullable);
        match &t.columns[0].default {
            Some(DefaultExpr::Sequence(q)) => assert_eq!(q.to_string(), "app.users_id_seq"),
            other => panic!("expected sequence default, got {other:?}"),
        }
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0].qname.to_string(), "app.users_id_seq");
        assert_eq!(seqs[0].data_type, ColumnType::Integer);
        let owner = seqs[0].owned_by.as_ref().expect("owned_by");
        assert_eq!(owner.table.to_string(), "app.users");
        assert_eq!(owner.column.as_str(), "id");
    }

    #[test]
    fn desugars_bigserial_to_bigint() {
        let mut t = table_with_serial("bigserial");
        let seqs = desugar_serials_in_table(&mut t, &loc()).unwrap();
        assert_eq!(t.columns[0].ty, ColumnType::BigInt);
        assert_eq!(seqs[0].data_type, ColumnType::BigInt);
    }

    #[test]
    fn desugars_smallserial_to_smallint() {
        let mut t = table_with_serial("smallserial");
        let seqs = desugar_serials_in_table(&mut t, &loc()).unwrap();
        assert_eq!(t.columns[0].ty, ColumnType::SmallInt);
        assert_eq!(seqs[0].data_type, ColumnType::SmallInt);
    }

    #[test]
    fn desugars_serial4_alias() {
        let mut t = table_with_serial("serial4");
        let seqs = desugar_serials_in_table(&mut t, &loc()).unwrap();
        assert_eq!(t.columns[0].ty, ColumnType::Integer);
        assert_eq!(seqs.len(), 1);
    }

    #[test]
    fn ignores_non_serial_other() {
        let mut t = table_with_serial("custom_type");
        let seqs = desugar_serials_in_table(&mut t, &loc()).unwrap();
        assert!(seqs.is_empty());
        assert!(matches!(t.columns[0].ty, ColumnType::Other { .. }));
    }

    #[test]
    fn ignores_already_canonical_types() {
        let mut t = Table {
            qname: qn("t"),
            columns: vec![Column {
                name: id("id"),
                ty: ColumnType::Integer,
                nullable: true,
                default: None,
                identity: None,
                generated: None,
                collation: None,
                comment: None,
            }],
            constraints: vec![],
                        partition_by: None,
            partition_of: None,
comment: None,
        };
        let seqs = desugar_serials_in_table(&mut t, &loc()).unwrap();
        assert!(seqs.is_empty());
        assert!(t.columns[0].nullable);
    }
}
