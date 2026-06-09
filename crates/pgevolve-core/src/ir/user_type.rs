//! User-defined types — enums, domains, composites.

use serde::{Deserialize, Serialize};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::column_type::ColumnType;
use crate::ir::default_expr::NormalizedExpr;
use crate::ir::difference::Difference;
use crate::ir::eq::Diff;

/// A user-defined type (enum, domain, or composite).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct UserType {
    /// Schema-qualified type name.
    pub qname: QualifiedName,
    /// The kind and kind-specific data for this type.
    pub kind: UserTypeKind,
    /// Optional `COMMENT ON TYPE` text.
    pub comment: Option<String>,
    /// Object owner. `None` = unmanaged (the differ ignores ownership).
    /// `Some(role)` = managed: diff emits `ALTER TYPE ... OWNER TO role`.
    pub owner: Option<Identifier>,
    /// Grants on this object. Empty = no grants. Canonicalized.
    pub grants: Vec<crate::ir::grant::Grant>,
}

/// The three user-defined type variants supported by pgevolve.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserTypeKind {
    /// `CREATE TYPE … AS ENUM (…)`.
    Enum {
        /// Ordered list of enum labels.
        values: Vec<EnumValue>,
    },
    /// `CREATE DOMAIN … AS …`.
    Domain {
        /// Underlying base type.
        base: ColumnType,
        /// Whether `NULL` values are accepted (`NOT NULL` constraint absent).
        nullable: bool,
        /// Optional `DEFAULT` expression.
        default: Option<NormalizedExpr>,
        /// `CHECK` constraints attached to this domain.
        check_constraints: Vec<DomainCheck>,
        /// Optional `COLLATE` clause.
        collation: Option<QualifiedName>,
    },
    /// `CREATE TYPE … AS (…)`.
    Composite {
        /// Ordered list of composite attributes.
        attributes: Vec<CompositeAttribute>,
    },
    /// `CREATE TYPE … AS RANGE (…)`.
    Range {
        /// Element type — `pg_range.rngsubtype`.
        subtype: QualifiedName,
        /// Optional opclass for the subtype's comparison.
        subtype_opclass: Option<QualifiedName>,
        /// Optional collation (only meaningful for collatable subtypes like text).
        collation: Option<QualifiedName>,
        /// Optional canonical function — `pg_range.rngcanonical`.
        canonical: Option<QualifiedName>,
        /// Optional `subtype_diff` function — `pg_range.rngsubdiff`.
        subtype_diff: Option<QualifiedName>,
        /// Custom multirange-type name (`None` → PG auto-names `<range>_multirange`).
        multirange_type_name: Option<Identifier>,
    },
}

/// A single label in a `CREATE TYPE … AS ENUM` declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnumValue {
    /// The enum label string.
    pub name: String,
    /// PG's `pg_enum.enumsortorder` is real4; we store the same float for
    /// byte-stable round-trip.
    pub sort_order: f32,
}

// f32 doesn't implement Eq or Hash. Provide manual impls that use the
// IEEE 754 bit pattern, so two EnumValues with the same sort_order
// compare equal and hash equal even though f32: !Eq in std.
impl Eq for EnumValue {}
impl std::hash::Hash for EnumValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.sort_order.to_bits().hash(state);
    }
}

/// A named `CHECK` constraint on a domain type.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DomainCheck {
    /// Constraint name.
    pub name: Identifier,
    /// Normalized check expression.
    pub expression: NormalizedExpr,
}

/// A single attribute (field) in a composite type.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CompositeAttribute {
    /// Attribute name.
    pub name: Identifier,
    /// Attribute data type.
    pub ty: ColumnType,
    /// Optional `COLLATE` clause on this attribute.
    pub collation: Option<QualifiedName>,
}

impl Diff for UserType {
    // The structural differ at the change level lives in `crate::diff::types`
    // and produces granular UserTypeChange variants. This `Diff` impl is the
    // debug/equivalence-rule hook used by `Catalog::diff` for reporting only;
    // a single top-level entry per changed type is intentional here.
    fn diff(&self, other: &Self) -> Vec<Difference> {
        use crate::ir::eq::diff_field;
        let mut out = Vec::new();
        if self.kind != other.kind {
            out.push(Difference::new(
                "",
                format!("{:?}", self.kind),
                format!("{:?}", other.kind),
            ));
        }
        out.extend(diff_field(
            "comment",
            &format!("{:?}", self.comment),
            &format!("{:?}", other.comment),
        ));
        out.extend(diff_field(
            "owner",
            &format!("{:?}", self.owner),
            &format!("{:?}", other.owner),
        ));
        out.extend(diff_field(
            "grants",
            &format!("{:?}", self.grants),
            &format!("{:?}", other.grants),
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::catalog::Catalog;
    use crate::ir::schema::Schema;

    fn ident(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }
    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(ident(schema), ident(name))
    }

    fn sample_enum() -> UserType {
        UserType {
            qname: qname("app", "order_status"),
            kind: UserTypeKind::Enum {
                values: vec![
                    EnumValue {
                        name: "pending".into(),
                        sort_order: 1.0,
                    },
                    EnumValue {
                        name: "shipped".into(),
                        sort_order: 2.0,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: Vec::new(),
        }
    }

    fn sample_domain() -> UserType {
        UserType {
            qname: qname("app", "positive_int"),
            kind: UserTypeKind::Domain {
                base: ColumnType::Integer,
                nullable: false,
                default: None,
                check_constraints: vec![],
                collation: None,
            },
            comment: None,
            owner: None,
            grants: Vec::new(),
        }
    }

    fn sample_range() -> UserType {
        UserType {
            qname: qname("app", "tsrange_co"),
            kind: UserTypeKind::Range {
                subtype: qname("pg_catalog", "timestamptz"),
                subtype_opclass: None,
                collation: None,
                canonical: None,
                subtype_diff: None,
                multirange_type_name: None,
            },
            comment: None,
            owner: None,
            grants: Vec::new(),
        }
    }

    fn sample_composite() -> UserType {
        UserType {
            qname: qname("app", "address"),
            kind: UserTypeKind::Composite {
                attributes: vec![
                    CompositeAttribute {
                        name: ident("street"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                    CompositeAttribute {
                        name: ident("zip"),
                        ty: ColumnType::Text,
                        collation: None,
                    },
                ],
            },
            comment: None,
            owner: None,
            grants: Vec::new(),
        }
    }

    #[test]
    fn user_types_round_trip_through_serde() {
        for ut in [
            sample_enum(),
            sample_domain(),
            sample_composite(),
            sample_range(),
        ] {
            let json = serde_json::to_string(&ut).unwrap();
            let back: UserType = serde_json::from_str(&json).unwrap();
            assert_eq!(ut, back);
        }
    }

    #[test]
    fn range_variant_with_all_fields_round_trip() {
        let r = UserType {
            qname: qname("app", "myrange"),
            kind: UserTypeKind::Range {
                subtype: qname("pg_catalog", "int4"),
                subtype_opclass: Some(qname("pg_catalog", "int4_ops")),
                collation: Some(qname("pg_catalog", "C")),
                canonical: Some(qname("app", "canon_fn")),
                subtype_diff: Some(qname("app", "diff_fn")),
                multirange_type_name: Some(ident("myrange_mr")),
            },
            comment: Some("a range".into()),
            owner: Some(ident("dba")),
            grants: Vec::new(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: UserType = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn catalog_holds_user_types_and_canonicalizes() {
        let mut c = Catalog::empty();
        c.schemas.push(Schema {
            name: ident("app"),
            comment: None,
            owner: None,
            grants: Vec::new(),
        });
        // Insert in non-canonical order.
        c.types.push(sample_composite());
        c.types.push(sample_enum());
        c.types.push(sample_domain());

        c = c.canonicalize().expect("must canonicalize");

        // After canonicalize, types are sorted by qname.
        let names: Vec<_> = c
            .types
            .iter()
            .map(|t| t.qname.name.as_str().to_string())
            .collect();
        assert_eq!(names, vec!["address", "order_status", "positive_int"]);
    }

    #[test]
    fn catalog_rejects_duplicate_user_type_qname() {
        use crate::ir::IrError;

        let mut c = Catalog::empty();
        c.schemas.push(Schema {
            name: ident("app"),
            comment: None,
            owner: None,
            grants: Vec::new(),
        });
        c.types.push(sample_enum());
        c.types.push(sample_enum()); // duplicate qname

        let result = c.canonicalize();
        assert!(
            matches!(result, Err(IrError::DuplicateObject { kind: "type", .. })),
            "expected IrError::DuplicateObject, got {result:?}",
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("order_status"), "should name the qname: {msg}");
    }

    #[test]
    fn enum_value_hash_uses_bit_pattern() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let a = EnumValue {
            name: "x".into(),
            sort_order: 1.0,
        };
        let b = EnumValue {
            name: "x".into(),
            sort_order: 1.0,
        };
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }

    #[test]
    fn owner_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = sample_enum();
        b.owner = Some(ident("new_owner"));
        assert!(sample_enum().diff(&b).iter().any(|x| x.path == "owner"));
    }

    #[test]
    fn grants_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = sample_enum();
        b.grants.push(crate::ir::grant::Grant {
            grantee: crate::ir::grant::GrantTarget::Public,
            privilege: crate::ir::grant::Privilege::Usage,
            with_grant_option: false,
            columns: None,
        });
        assert!(sample_enum().diff(&b).iter().any(|x| x.path == "grants"));
    }

    #[test]
    fn comment_change_diffs() {
        use crate::ir::eq::Diff;
        let mut b = sample_enum();
        b.comment = Some("A helpful comment".into());
        // Regression guard: Stage 2 rewrote Diff for UserType and accidentally
        // dropped comment from the diff; this test ensures a comment-only change
        // still produces a Difference entry with path "comment".
        assert!(
            sample_enum().diff(&b).iter().any(|x| x.path == "comment"),
            "expected a comment difference; got: {:?}",
            sample_enum().diff(&b),
        );
    }
}
