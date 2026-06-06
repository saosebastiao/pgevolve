//! Advisory: a tablespace exists in both source and live with a different
//! LOCATION. Postgres cannot relocate a tablespace, so pgevolve never
//! auto-changes it — the user must recreate it manually if intended.

use crate::ir::cluster::catalog::ClusterCatalog;
use crate::lint::finding::{Finding, Severity};

/// Rule ID emitted on the finding; matches the file name.
pub const RULE_ID: &str = "tablespace-location-drift";

/// Compares tablespace locations between `source` (desired) and `target` (live).
///
/// Fires an advisory [`Severity::Warning`] for every tablespace that appears in
/// both catalogs but has a differing `location`. Postgres does not allow
/// relocating a tablespace after creation; pgevolve never emits an ALTER for
/// it, so the finding is purely advisory — the operator must recreate the
/// tablespace manually if the change was intentional.
pub fn check(source: &ClusterCatalog, target: &ClusterCatalog) -> Vec<Finding> {
    let mut out = Vec::new();
    for s in &source.tablespaces {
        if let Some(t) = target.tablespaces.iter().find(|t| t.name == s.name)
            && t.location != s.location
        {
            out.push(Finding {
                severity: Severity::Warning,
                rule: RULE_ID,
                message: format!(
                    "tablespace {} location differs: live={}, source={} \
                     — pgevolve does not relocate tablespaces; recreate manually if intended",
                    s.name, t.location, s.location,
                ),
                location: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::cluster::catalog::ClusterCatalog;
    use crate::ir::cluster::tablespace::Tablespace;
    use std::collections::BTreeMap;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn ts(name: &str, location: &str) -> Tablespace {
        Tablespace {
            name: id(name),
            location: location.to_string(),
            owner: None,
            options: BTreeMap::new(),
            comment: None,
        }
    }

    fn catalog_with(tablespaces: Vec<Tablespace>) -> ClusterCatalog {
        ClusterCatalog {
            roles: vec![],
            tablespaces,
        }
    }

    #[test]
    fn different_location_fires_one_finding() {
        let source = catalog_with(vec![ts("fast_ssd", "/mnt/nvme")]);
        let target = catalog_with(vec![ts("fast_ssd", "/mnt/ssd")]);
        let findings = check(&source, &target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn same_location_no_finding() {
        let source = catalog_with(vec![ts("fast_ssd", "/mnt/ssd")]);
        let target = catalog_with(vec![ts("fast_ssd", "/mnt/ssd")]);
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn tablespace_only_in_source_no_finding() {
        let source = catalog_with(vec![ts("fast_ssd", "/mnt/ssd")]);
        let target = catalog_with(vec![]);
        assert!(check(&source, &target).is_empty());
    }

    #[test]
    fn tablespace_only_in_target_no_finding() {
        let source = catalog_with(vec![]);
        let target = catalog_with(vec![ts("fast_ssd", "/mnt/ssd")]);
        assert!(check(&source, &target).is_empty());
    }
}
