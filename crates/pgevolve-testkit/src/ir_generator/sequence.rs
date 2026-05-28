//! Sequence construction helper used when seeding one sequence per schema
//! inside [`super::arbitrary_catalog`].

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::column_type::ColumnType;
use pgevolve_core::ir::sequence::Sequence;

pub(super) fn stand_alone_sequence(schema: &Identifier) -> Sequence {
    Sequence {
        qname: QualifiedName::new(
            schema.clone(),
            Identifier::from_unquoted(&format!("{schema}_seq")).unwrap(),
        ),
        data_type: ColumnType::BigInt,
        start: 1,
        increment: 1,
        min_value: None,
        max_value: None,
        cache: 1,
        cycle: false,
        owned_by: None,
        comment: None,
        owner: None,
        grants: vec![],
    }
}
