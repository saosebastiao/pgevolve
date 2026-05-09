//! `CREATE SEQUENCE` → [`crate::ir::sequence::Sequence`].

use pg_query::protobuf::{a_const, AConst, CreateSeqStmt, DefElem};
use pg_query::NodeEnum;

use crate::identifier::Identifier;
use crate::ir::column_type::ColumnType;
use crate::ir::sequence::{Sequence, SequenceOwner};
use crate::parse::builder::shared;
use crate::parse::error::{ParseError, SourceLocation};

/// Build a [`Sequence`] from a `CREATE SEQUENCE` AST.
pub fn build_sequence(
    stmt: &CreateSeqStmt,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Sequence, ParseError> {
    let range = stmt
        .sequence
        .as_ref()
        .ok_or_else(|| ParseError::Structural {
            location: location.clone(),
            message: "CREATE SEQUENCE missing sequence name".into(),
        })?;
    let qname = shared::resolve_qname(range, default_schema, location)?;

    // Postgres defaults — `bigint`, start=1, increment=1, cache=1, no cycle.
    let mut data_type = ColumnType::BigInt;
    let mut start: i64 = 1;
    let mut increment: i64 = 1;
    let mut min_value: Option<i64> = None;
    let mut max_value: Option<i64> = None;
    let mut cache: i64 = 1;
    let mut cycle = false;
    let mut owned_by: Option<SequenceOwner> = None;

    for opt in &stmt.options {
        let Some(NodeEnum::DefElem(de)) = opt.node.as_ref() else {
            continue;
        };
        match de.defname.as_str() {
            "as" => {
                if let Some(t) = type_name_of(de) {
                    if let Ok(parsed) = ColumnType::parse_from_pg_type_string(&t) {
                        data_type = parsed;
                    }
                }
            }
            "start" => {
                if let Some(v) = i64_of(de) {
                    start = v;
                }
            }
            "increment" => {
                if let Some(v) = i64_of(de) {
                    increment = v;
                }
            }
            "minvalue" => {
                min_value = i64_of(de);
            }
            "maxvalue" => {
                max_value = i64_of(de);
            }
            "cache" => {
                if let Some(v) = i64_of(de) {
                    cache = v;
                }
            }
            "cycle" => {
                cycle = bool_of(de).unwrap_or(true);
            }
            "owned_by" => {
                owned_by = parse_owned_by(de, default_schema, location)?;
            }
            _ => {}
        }
    }

    // If start is unset (option absent) and the type is signed, Postgres defaults
    // to 1 — already the value above. Likewise for `min/max_value` we keep `None`
    // meaning "use the type's min/max".

    Ok(Sequence {
        qname,
        data_type,
        start,
        increment,
        min_value,
        max_value,
        cache,
        cycle,
        owned_by,
        comment: None,
    })
}

fn i64_of(de: &DefElem) -> Option<i64> {
    let arg = de.arg.as_ref()?.node.as_ref()?;
    match arg {
        NodeEnum::Integer(i) => Some(i64::from(i.ival)),
        NodeEnum::Float(f) => f.fval.parse::<i64>().ok(),
        NodeEnum::AConst(c) => aconst_int(c),
        _ => None,
    }
}

fn bool_of(de: &DefElem) -> Option<bool> {
    let arg = de.arg.as_ref()?.node.as_ref()?;
    match arg {
        NodeEnum::Boolean(b) => Some(b.boolval),
        NodeEnum::Integer(i) => Some(i.ival != 0),
        NodeEnum::AConst(c) => match c.val.as_ref()? {
            a_const::Val::Boolval(b) => Some(b.boolval),
            a_const::Val::Ival(i) => Some(i.ival != 0),
            _ => None,
        },
        _ => None,
    }
}

fn aconst_int(c: &AConst) -> Option<i64> {
    match c.val.as_ref()? {
        a_const::Val::Ival(i) => Some(i64::from(i.ival)),
        a_const::Val::Fval(f) => f.fval.parse::<i64>().ok(),
        _ => None,
    }
}

fn type_name_of(de: &DefElem) -> Option<String> {
    let arg = de.arg.as_ref()?.node.as_ref()?;
    if let NodeEnum::TypeName(tn) = arg {
        return shared::render_type_name_to_string(tn);
    }
    None
}

/// `OWNED BY <table>.<column>` is encoded as a list of String nodes.
fn parse_owned_by(
    de: &DefElem,
    default_schema: Option<&Identifier>,
    location: &SourceLocation,
) -> Result<Option<SequenceOwner>, ParseError> {
    let Some(arg) = de.arg.as_ref().and_then(|a| a.node.as_ref()) else {
        return Ok(None);
    };
    let NodeEnum::List(list) = arg else {
        return Ok(None);
    };
    let parts: Vec<&str> = list
        .items
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.as_str()),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        // `OWNED BY NONE` is a single "none" string.
        [first] if first.eq_ignore_ascii_case("none") => Ok(None),
        [table, column] => {
            let schema = default_schema
                .cloned()
                .ok_or_else(|| ParseError::UnqualifiedName {
                    location: location.clone(),
                })?;
            Ok(Some(SequenceOwner {
                table: crate::identifier::QualifiedName::new(
                    schema,
                    shared::ident(table, location)?,
                ),
                column: shared::ident(column, location)?,
            }))
        }
        [schema, table, column] => Ok(Some(SequenceOwner {
            table: crate::identifier::QualifiedName::new(
                shared::ident(schema, location)?,
                shared::ident(table, location)?,
            ),
            column: shared::ident(column, location)?,
        })),
        _ => Err(ParseError::Structural {
            location: location.clone(),
            message: "OWNED BY must reference table.column or schema.table.column".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc() -> SourceLocation {
        SourceLocation::new(PathBuf::from("test.sql"), 1, 1)
    }

    fn build(sql: &str) -> Sequence {
        let parsed = pg_query::parse(sql).expect("parses");
        let stmt = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw| raw.stmt)
            .and_then(|n| n.node)
            .expect("stmt");
        let NodeEnum::CreateSeqStmt(s) = stmt else {
            panic!()
        };
        build_sequence(&s, None, &loc()).expect("builds")
    }

    #[test]
    fn defaults_for_bare_create() {
        let s = build("CREATE SEQUENCE app.s1;");
        assert_eq!(s.qname.to_string(), "app.s1");
        assert_eq!(s.data_type, ColumnType::BigInt);
        assert_eq!(s.start, 1);
        assert_eq!(s.increment, 1);
        assert_eq!(s.cache, 1);
        assert!(!s.cycle);
        assert!(s.owned_by.is_none());
    }

    #[test]
    fn explicit_options_extracted() {
        let s = build(
            "CREATE SEQUENCE app.s1 AS integer INCREMENT BY 2 START WITH 10 MINVALUE 5 \
             MAXVALUE 1000 CACHE 50 CYCLE;",
        );
        assert_eq!(s.data_type, ColumnType::Integer);
        assert_eq!(s.start, 10);
        assert_eq!(s.increment, 2);
        assert_eq!(s.min_value, Some(5));
        assert_eq!(s.max_value, Some(1000));
        assert_eq!(s.cache, 50);
        assert!(s.cycle);
    }

    #[test]
    fn owned_by_extracted() {
        let s = build("CREATE SEQUENCE app.users_id_seq OWNED BY app.users.id;");
        let owner = s.owned_by.expect("owned_by present");
        assert_eq!(owner.table.to_string(), "app.users");
        assert_eq!(owner.column.as_str(), "id");
    }

    #[test]
    fn owned_by_none_yields_no_owner() {
        let s = build("CREATE SEQUENCE app.s OWNED BY NONE;");
        assert!(s.owned_by.is_none());
    }
}
