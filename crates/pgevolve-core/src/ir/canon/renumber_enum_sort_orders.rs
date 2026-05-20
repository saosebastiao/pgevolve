//! Re-index every enum's `sort_order` values to `1.0, 2.0, 3.0, …` in
//! ascending order.
//!
//! PG stores enum sort orders as float4 and `ALTER TYPE … ADD VALUE`
//! can produce fractional or 0-indexed values. The source parser
//! assigns `1.0, 2.0, …` in declaration order. The IR-level
//! equivalence we care about is the value names AND their relative
//! order — the floats are storage detail. Re-numbering on both sides
//! makes byte-equality work without a custom `Eq` impl.

use crate::ir::catalog::Catalog;
use crate::ir::user_type::UserTypeKind;

/// Sort each enum's values by current `sort_order`, then renumber to
/// `1.0, 2.0, 3.0, …`.
pub fn run(cat: &mut Catalog) {
    for t in &mut cat.types {
        if let UserTypeKind::Enum { values } = &mut t.kind {
            // Preserve relative order (sort by current sort_order
            // ascending) then assign 1-indexed floats.
            values.sort_by(|a, b| {
                a.sort_order
                    .partial_cmp(&b.sort_order)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            #[allow(clippy::cast_precision_loss)]
            for (i, v) in values.iter_mut().enumerate() {
                v.sort_order = (i as f32) + 1.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::user_type::{EnumValue, UserType, UserTypeKind};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    #[test]
    fn renumbers_fractional_orders_to_sequential_floats() {
        let mut cat = Catalog::empty();
        cat.types.push(UserType {
            qname: QualifiedName::new(id("app"), id("status")),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue {
                        name: "open".into(),
                        sort_order: 0.5,
                    },
                    EnumValue {
                        name: "closed".into(),
                        sort_order: 1.7,
                    },
                    EnumValue {
                        name: "pending".into(),
                        sort_order: 0.1,
                    },
                ],
            },
            comment: None,
        });
        run(&mut cat);
        let kind = &cat.types[0].kind;
        let UserTypeKind::Enum { values } = kind else {
            panic!("expected Enum kind, got {kind:?}");
        };
        let orders: Vec<f32> = values.iter().map(|v| v.sort_order).collect();
        let names: Vec<&str> = values.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(orders, vec![1.0, 2.0, 3.0]);
        // Sorted by original sort_order: pending (0.1), open (0.5), closed (1.7).
        assert_eq!(names, vec!["pending", "open", "closed"]);
    }
}
