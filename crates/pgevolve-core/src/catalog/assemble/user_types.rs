//! User-defined type assembly (enums, domains, composites) from catalog rows.
//!
//! Called from [`super::assemble`] to build [`crate::ir::user_type::UserType`]
//! IR entries.

use std::collections::HashMap;

use crate::catalog::error::CatalogError;
use crate::catalog::filter::CatalogFilter;
use crate::catalog::rows::Row;
use crate::ir::column_type::ColumnType;
use crate::ir::user_type::{CompositeAttribute, DomainCheck, EnumValue, UserType, UserTypeKind};

use super::{ident_required, qname_from, reparse_expression_text, strip_check_wrapper};

/// Build user-defined types (enums, domains, composites) from raw catalog rows.
#[allow(clippy::too_many_lines)]
pub(super) fn build_user_types(
    type_rows: &[Row],
    enum_value_rows: &[Row],
    domain_detail_rows: &[Row],
    domain_check_rows: &[Row],
    comp_attr_rows: &[Row],
    filter: &CatalogFilter,
) -> Result<Vec<UserType>, CatalogError> {
    use crate::catalog::CatalogQuery as Q;

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
            owner: None,
            grants: vec![],
        });
    }

    // canonicalize sorts by qname — no pre-sort needed here, but do it for
    // consistent ordering before the catalog-level canonicalize call.
    out.sort_by(|a, b| a.qname.cmp(&b.qname));
    Ok(out)
}
