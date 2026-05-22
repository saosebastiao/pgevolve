//! Dispatcher for `Change::Extension(ExtensionChange)`.

use std::sync::LazyLock;

use crate::diff::change::ExtensionChange;
use crate::identifier::{Identifier, QualifiedName};
use crate::plan::raw_step::{RawStep, StepKind, TransactionConstraint};
use crate::plan::rewrite::extensions as sql;

/// Synthetic schema component for extension targets.
///
/// Extensions live outside per-schema scope; this literal is a valid ASCII
/// identifier and is constructed exactly once at first use.
static PG_EXTENSION_SCHEMA: LazyLock<Identifier> = LazyLock::new(|| {
    Identifier::from_unquoted("pg_extension")
        .expect("'pg_extension' is a valid ASCII identifier — this is a compile-time constant")
});

pub fn emit(
    ec: ExtensionChange,
    destructive: bool,
    destructive_reason: Option<String>,
    out: &mut Vec<RawStep>,
) {
    match ec {
        ExtensionChange::Create(e) => {
            let name = e.name.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateExtension,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::create_extension(&e),
                transactional: TransactionConstraint::InTransaction,
            });
            if let Some(comment) = &e.comment {
                out.push(RawStep {
                    step_no: 0,
                    kind: StepKind::CommentOnExtension,
                    destructive: false,
                    destructive_reason: None,
                    intent_id: None,
                    targets: vec![extension_target(&name)],
                    sql: sql::comment_on_extension(&name, Some(comment)),
                    transactional: TransactionConstraint::InTransaction,
                });
            }
        }
        ExtensionChange::Drop(name) => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropExtension,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::drop_extension(&name),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        ExtensionChange::AlterUpdate { name, to_version } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::AlterExtensionUpdate,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::alter_extension_update(&name, &to_version),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        ExtensionChange::ReplaceWithCascade(e) => {
            let name = e.name.clone();
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::DropExtension,
                destructive,
                destructive_reason,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::drop_extension(&name),
                transactional: TransactionConstraint::InTransaction,
            });
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CreateExtension,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::create_extension(&e),
                transactional: TransactionConstraint::InTransaction,
            });
        }
        ExtensionChange::CommentOn { name, comment } => {
            out.push(RawStep {
                step_no: 0,
                kind: StepKind::CommentOnExtension,
                destructive: false,
                destructive_reason: None,
                intent_id: None,
                targets: vec![extension_target(&name)],
                sql: sql::comment_on_extension(&name, comment.as_deref()),
                transactional: TransactionConstraint::InTransaction,
            });
        }
    }
}

/// Extensions live outside per-schema scope; produce a synthetic
/// `pg_extension.<name>` target so the plan's `targets` field always
/// carries a `QualifiedName`. Matches the convention `schema_target`
/// uses for schemas (where the schema's name appears as its own
/// "schema").
fn extension_target(name: &Identifier) -> QualifiedName {
    QualifiedName::new(PG_EXTENSION_SCHEMA.clone(), name.clone())
}
