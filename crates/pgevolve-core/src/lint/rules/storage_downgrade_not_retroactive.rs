//! `storage-downgrade-not-retroactive` lint rule.
//!
//! Warns when a SET STORAGE change reduces toastability. PG accepts the
//! change but existing rows keep their current placement until the next
//! UPDATE; authors expecting retroactive compaction usually want
//! VACUUM FULL or a table rewrite, neither of which pgevolve emits.

use crate::diff::change::Change;
use crate::diff::changeset::ChangeSet;
use crate::diff::table_op::TableOp;
use crate::ir::column::StorageKind;
use crate::lint::finding::{Finding, Severity};

/// Rule ID emitted on the finding; matches the file name.
pub const RULE_ID: &str = "storage-downgrade-not-retroactive";

pub fn check(cs: &ChangeSet) -> Vec<Finding> {
    let mut findings = Vec::new();
    for entry in cs.iter() {
        let Change::AlterTable { qname, ops } = &entry.change else {
            continue;
        };
        for op_entry in ops {
            let TableOp::SetColumnStorage { name, from, to } = &op_entry.op else {
                continue;
            };
            if is_downgrade(*from, *to) {
                findings.push(Finding {
                    severity: Severity::Warning,
                    rule: RULE_ID,
                    message: format!(
                        "column {qname}.{name} STORAGE {} → {} is not retroactive; \
                         existing TOASTed values remain in their current placement \
                         until rewritten by UPDATE or VACUUM FULL",
                        storage_name(*from),
                        storage_name(*to),
                    ),
                    location: None,
                });
            }
        }
    }
    findings
}

const fn is_downgrade(from: StorageKind, to: StorageKind) -> bool {
    const fn rank(s: StorageKind) -> u8 {
        match s {
            StorageKind::External => 3, // out-of-line, no compress
            StorageKind::Extended => 2, // out-of-line + compress
            StorageKind::Main => 1,     // compress inline, out-of-line as last resort
            StorageKind::Plain => 0,    // inline only
        }
    }
    rank(to) < rank(from)
}

const fn storage_name(s: StorageKind) -> &'static str {
    match s {
        StorageKind::Plain => "PLAIN",
        StorageKind::External => "EXTERNAL",
        StorageKind::Extended => "EXTENDED",
        StorageKind::Main => "MAIN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::changeset::ChangeSet;
    use crate::diff::destructiveness::Destructiveness;
    use crate::diff::table_op::TableOpEntry;
    use crate::identifier::{Identifier, QualifiedName};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn changeset_with_storage_change(from: StorageKind, to: StorageKind) -> ChangeSet {
        let mut cs = ChangeSet::new();
        cs.push(
            Change::AlterTable {
                qname: qn("app", "t"),
                ops: vec![TableOpEntry {
                    op: TableOp::SetColumnStorage {
                        name: id("body"),
                        from,
                        to,
                    },
                    destructiveness: Destructiveness::Safe,
                }],
            },
            Destructiveness::Safe,
        );
        cs
    }

    #[test]
    fn external_to_main_fires() {
        let cs = changeset_with_storage_change(StorageKind::External, StorageKind::Main);
        let findings = check(&cs);
        assert_eq!(findings.len(), 1, "expected one finding: {findings:?}");
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].message.contains("EXTERNAL"),
            "message should mention EXTERNAL: {}",
            findings[0].message
        );
        assert!(
            findings[0].message.contains("MAIN"),
            "message should mention MAIN: {}",
            findings[0].message
        );
    }

    #[test]
    fn plain_to_extended_does_not_fire() {
        // PLAIN → EXTENDED is an upgrade (higher rank), should be silent.
        let cs = changeset_with_storage_change(StorageKind::Plain, StorageKind::Extended);
        let findings = check(&cs);
        assert!(findings.is_empty(), "upgrade must not fire: {findings:?}");
    }

    #[test]
    fn no_storage_change_no_finding() {
        let cs = ChangeSet::new();
        let findings = check(&cs);
        assert!(
            findings.is_empty(),
            "empty changeset must yield no findings"
        );
    }

    #[test]
    fn extended_to_external_does_not_fire() {
        // EXTENDED (rank 2) → EXTERNAL (rank 3): external has higher rank,
        // so this is actually an upgrade. Should NOT fire.
        let cs = changeset_with_storage_change(StorageKind::Extended, StorageKind::External);
        let findings = check(&cs);
        assert!(
            findings.is_empty(),
            "EXTENDED→EXTERNAL is upgrade, must not fire: {findings:?}"
        );
    }

    #[test]
    fn extended_to_main_fires() {
        // EXTENDED (rank 2) → MAIN (rank 1): downgrade.
        let cs = changeset_with_storage_change(StorageKind::Extended, StorageKind::Main);
        let findings = check(&cs);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn same_storage_no_finding() {
        let cs = changeset_with_storage_change(StorageKind::External, StorageKind::External);
        let findings = check(&cs);
        assert!(
            findings.is_empty(),
            "same-storage no-op must not fire: {findings:?}"
        );
    }
}
