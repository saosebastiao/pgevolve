//! Dispatcher for `Change::UserType(UserTypeChange)`.

use super::super::types::{
    emit_alter_domain_add_check, emit_alter_domain_drop_check, emit_alter_domain_set_default,
    emit_alter_domain_set_not_null, emit_alter_type_add_attribute, emit_alter_type_add_value,
    emit_alter_type_alter_attribute_type, emit_alter_type_drop_attribute,
    emit_alter_type_rename_value, emit_comment_on_type, emit_create_type, emit_drop_type,
    emit_drop_type_cascade,
};
use crate::diff::change::UserTypeChange;
use crate::ir::user_type::UserTypeKind;
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};

#[allow(clippy::too_many_lines)] // one arm per `UserTypeChange` variant emitting its SQL; extraction would scatter the templates.
pub fn emit(
    utc: UserTypeChange,
    destructive: bool,
    destructive_reason: Option<String>,
    ctx: &super::super::Ctx<'_>,
    out: &mut Vec<RawStep>,
) {
    use UserTypeChange as U;

    match utc {
        U::Create(ut) => {
            let qname = ut.qname.clone();
            let kind = ut.kind.clone();
            let comment = ut.comment.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateType,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_type(&ut),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnType,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_type(&qname, &kind, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }

        U::Drop(qname) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropType,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_drop_type(&qname),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::ReplaceWithCascade { source, catalog } => {
            // Name every object the inherent `DROP TYPE … CASCADE` destroys, so
            // the destruction is auditable rather than hidden behind CASCADE.
            // The emitted SQL is unchanged — only the reason text grows.
            let deps =
                crate::plan::type_dependents::enumerate_type_dependents(&catalog.qname, ctx.target);
            let suffix =
                crate::plan::type_dependents::render_cascade_destruction(&catalog.qname, &deps);
            let enriched_reason = match &destructive_reason {
                Some(base) if !suffix.is_empty() => Some(format!("{base}{suffix}")),
                other => other.clone(),
            };
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropType,
                destructive,
                destructive_reason: enriched_reason.clone(),
                intent_id: None,
                targets: vec![catalog.qname.clone()],
                sql: emit_drop_type_cascade(&catalog.qname),
                transactional: TransactionConstraint::InTransaction,
            });
            let qname = source.qname.clone();
            let kind = source.kind.clone();
            let comment = source.comment.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateType,
                // The recreation half of a CASCADE replacement is itself
                // destructive: it executes only after DROP CASCADE has
                // removed dependent columns/views, so the intent gate must
                // also apply here.
                destructive,
                destructive_reason: enriched_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_create_type(&source),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(c) = &comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnType,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![qname.clone()],
                    sql: emit_comment_on_type(&qname, &kind, Some(c)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }

        U::EnumAddValue {
            qname,
            value,
            before,
            after,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTypeAddValue,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_type_add_value(&qname, &value, before.as_deref(), after.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::EnumRenameValue { qname, from, to } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTypeRenameValue,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_type_rename_value(&qname, &from, &to),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::DomainAddCheck { qname, constraint } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterDomainAddConstraint,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_domain_add_check(&qname, &constraint),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::DomainDropCheck { qname, name } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterDomainDropConstraint,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_domain_drop_check(&qname, &name),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::DomainSetDefault { qname, default } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterDomainSetDefault,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_domain_set_default(&qname, default.as_ref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::DomainSetNotNull { qname, not_null } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterDomainSetNotNull,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_domain_set_not_null(&qname, not_null),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::CompositeAddAttribute { qname, attribute } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTypeAddAttribute,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_type_add_attribute(&qname, &attribute),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::CompositeDropAttribute { qname, name } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTypeDropAttribute,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_type_drop_attribute(&qname, &name),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::CompositeAlterAttributeType {
            qname,
            attribute,
            new_type,
        } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterTypeAlterAttributeType,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![qname.clone()],
                sql: emit_alter_type_alter_attribute_type(&qname, &attribute, &new_type),
                transactional: TransactionConstraint::InTransaction,
            });
        }

        U::SetComment { qname, comment } => {
            // We need the type's kind to choose DOMAIN vs TYPE keyword.
            // Prefer source catalog (desired state); fall back to target (live state).
            let kind = ctx
                .source
                .types
                .iter()
                .find(|t| t.qname == qname)
                .or_else(|| ctx.target.types.iter().find(|t| t.qname == qname))
                .map(|t| &t.kind);
            let sql = if let Some(kind) = kind {
                emit_comment_on_type(&qname, kind, comment.as_deref())
            } else {
                // Fallback: use TYPE keyword (should not happen in practice).
                let fallback_kind = UserTypeKind::Enum { values: vec![] };
                emit_comment_on_type(&qname, &fallback_kind, comment.as_deref())
            };
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnType,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![qname.clone()],
                sql,
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}
