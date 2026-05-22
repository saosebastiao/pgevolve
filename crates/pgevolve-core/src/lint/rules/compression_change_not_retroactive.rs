//! `compression-change-not-retroactive` lint rule.
//!
//! Warns on any SET COMPRESSION change. Existing `TOASTed` values keep
//! their original codec; only new/updated rows get the new codec.

use crate::diff::change::Change;
use crate::diff::changeset::ChangeSet;
use crate::diff::table_op::TableOp;
use crate::lint::finding::{Finding, Severity};

/// Rule ID emitted on the finding; matches the file name.
pub const RULE_ID: &str = "compression-change-not-retroactive";

pub fn check(cs: &ChangeSet) -> Vec<Finding> {
    let mut findings = Vec::new();
    for entry in cs.iter() {
        let Change::AlterTable { qname, ops } = &entry.change else {
            continue;
        };
        for op_entry in ops {
            let TableOp::SetColumnCompression { name, .. } = &op_entry.op else {
                continue;
            };
            findings.push(Finding {
                severity: Severity::Warning,
                rule: RULE_ID,
                message: format!(
                    "column {qname}.{name} compression change is not retroactive; \
                     existing TOASTed values keep their original codec \
                     until rewritten by UPDATE or VACUUM FULL",
                ),
                location: None,
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::changeset::ChangeSet;
    use crate::diff::destructiveness::Destructiveness;
    use crate::diff::table_op::TableOpEntry;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::column::Compression;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn changeset_with_compression_change(compression: Option<Compression>) -> ChangeSet {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "t"),
                ops: vec![TableOpEntry {
                    op: TableOp::SetColumnCompression {
                        name: id("blob"),
                        compression,
                    },
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        cs
    }

    #[test]
    fn any_compression_change_fires() {
        // Pglz → Lz4 (represented as SetColumnCompression { compression: Some(Lz4) }).
        let cs = changeset_with_compression_change(Some(Compression::Lz4));
        let findings = check(&cs);
        assert_eq!(findings.len(), 1, "expected one finding: {findings:?}");
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn set_to_default_also_fires() {
        // Lz4 → None (cluster default) still emits a finding.
        let cs = changeset_with_compression_change(None);
        let findings = check(&cs);
        assert_eq!(findings.len(), 1, "expected one finding: {findings:?}");
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn no_change_no_finding() {
        let cs = ChangeSet::new();
        let findings = check(&cs);
        assert!(
            findings.is_empty(),
            "empty changeset must yield no findings"
        );
    }

    #[test]
    fn multiple_columns_each_fires() {
        // Two compression ops in one AlterTable → two findings.
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "t"),
                ops: vec![
                    TableOpEntry {
                        op: TableOp::SetColumnCompression {
                            name: id("blob"),
                            compression: Some(Compression::Lz4),
                        },
                        destructiveness: Destructiveness::Safe,
                    },
                    TableOpEntry {
                        op: TableOp::SetColumnCompression {
                            name: id("data"),
                            compression: Some(Compression::Pglz),
                        },
                        destructiveness: Destructiveness::Safe,
                    },
                ],
            },
            Destructiveness::Safe,
        );
        let findings = check(&cs);
        assert_eq!(findings.len(), 2, "expected two findings: {findings:?}");
    }
}
