//! Canon for `Catalog::ts_dictionaries` and `Catalog::ts_configurations`.
//!
//! - Both collections are sorted by `qname.render_sql()`.
//! - Within each configuration, `mappings` are sorted by `token_type`.
//! - Within each dictionary, `options` are sorted by key (the `.0` field).
//! - Duplicate `qname` identity within each kind raises the corresponding
//!   [`IrError`] variant.

use crate::ir::IrError;
use crate::ir::catalog::Catalog;

/// Canonicalize all text-search objects in `cat`.
///
/// - `ts_dictionaries` sorted by `qname.render_sql()`. Duplicate `qname`
///   raises [`IrError::DuplicateTsDictionary`].
/// - `ts_configurations` sorted by `qname.render_sql()`. Duplicate `qname`
///   raises [`IrError::DuplicateTsConfiguration`].
/// - Each dictionary's `options` sorted by key.
/// - Each configuration's `mappings` sorted by `token_type`.
pub fn run(cat: &mut Catalog) -> Result<(), IrError> {
    // Sort and dedupe dictionaries.
    for dict in &mut cat.ts_dictionaries {
        dict.options.sort_by(|a, b| a.0.cmp(&b.0));
    }
    cat.ts_dictionaries.sort_by_key(|d| d.qname.render_sql());
    for w in cat.ts_dictionaries.windows(2) {
        if w[0].qname == w[1].qname {
            return Err(IrError::DuplicateTsDictionary(w[0].qname.clone()));
        }
    }

    // Sort and dedupe configurations.
    for cfg in &mut cat.ts_configurations {
        cfg.mappings.sort_by(|a, b| a.token_type.cmp(&b.token_type));
    }
    cat.ts_configurations.sort_by_key(|c| c.qname.render_sql());
    for w in cat.ts_configurations.windows(2) {
        if w[0].qname == w[1].qname {
            return Err(IrError::DuplicateTsConfiguration(w[0].qname.clone()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::{Identifier, QualifiedName};
    use crate::ir::text_search::{TsConfiguration, TsDictionary, TsMapping};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qname(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn dict(schema: &str, name: &str) -> TsDictionary {
        TsDictionary {
            qname: qname(schema, name),
            template: qname("pg_catalog", "snowball"),
            options: vec![],
            owner: None,
            comment: None,
        }
    }

    fn cfg(schema: &str, name: &str) -> TsConfiguration {
        TsConfiguration {
            qname: qname(schema, name),
            parser: qname("pg_catalog", "default"),
            mappings: vec![],
            owner: None,
            comment: None,
        }
    }

    // ---- dictionary sort determinism ----

    #[test]
    fn dictionaries_sorted_by_qname() {
        let mut cat = Catalog::empty();
        cat.ts_dictionaries.push(dict("public", "zzz_dict"));
        cat.ts_dictionaries.push(dict("public", "aaa_dict"));
        run(&mut cat).unwrap();
        assert_eq!(cat.ts_dictionaries[0].qname.name.as_str(), "aaa_dict");
        assert_eq!(cat.ts_dictionaries[1].qname.name.as_str(), "zzz_dict");
    }

    #[test]
    fn dictionaries_sorted_across_schemas() {
        let mut cat = Catalog::empty();
        cat.ts_dictionaries.push(dict("zns", "alpha"));
        cat.ts_dictionaries.push(dict("ans", "alpha"));
        run(&mut cat).unwrap();
        assert_eq!(cat.ts_dictionaries[0].qname.schema.as_str(), "ans");
        assert_eq!(cat.ts_dictionaries[1].qname.schema.as_str(), "zns");
    }

    // ---- dictionary dup rejection ----

    #[test]
    fn rejects_duplicate_dictionary() {
        let mut cat = Catalog::empty();
        cat.ts_dictionaries.push(dict("public", "english_stem"));
        cat.ts_dictionaries.push(dict("public", "english_stem"));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateTsDictionary(_)
        ));
    }

    // ---- dictionary options sorted by key ----

    #[test]
    fn dictionary_options_sorted_by_key() {
        let mut cat = Catalog::empty();
        let mut d = dict("public", "my_dict");
        d.options = vec![
            ("stopwords".to_string(), "english".to_string()),
            ("language".to_string(), "english".to_string()),
        ];
        cat.ts_dictionaries.push(d);
        run(&mut cat).unwrap();
        assert_eq!(cat.ts_dictionaries[0].options[0].0, "language");
        assert_eq!(cat.ts_dictionaries[0].options[1].0, "stopwords");
    }

    // ---- configuration sort determinism ----

    #[test]
    fn configurations_sorted_by_qname() {
        let mut cat = Catalog::empty();
        cat.ts_configurations.push(cfg("public", "zzz_config"));
        cat.ts_configurations.push(cfg("public", "aaa_config"));
        run(&mut cat).unwrap();
        assert_eq!(cat.ts_configurations[0].qname.name.as_str(), "aaa_config");
        assert_eq!(cat.ts_configurations[1].qname.name.as_str(), "zzz_config");
    }

    // ---- configuration dup rejection ----

    #[test]
    fn rejects_duplicate_configuration() {
        let mut cat = Catalog::empty();
        cat.ts_configurations.push(cfg("public", "english_config"));
        cat.ts_configurations.push(cfg("public", "english_config"));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateTsConfiguration(_)
        ));
    }

    // ---- configuration mappings sorted by token_type ----

    #[test]
    fn configuration_mappings_sorted_by_token_type() {
        let mut cat = Catalog::empty();
        let mut c = cfg("public", "english_config");
        c.mappings = vec![
            TsMapping {
                token_type: "word".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
            TsMapping {
                token_type: "asciiword".to_string(),
                dictionaries: vec![qname("public", "english_stem")],
            },
            TsMapping {
                token_type: "numword".to_string(),
                dictionaries: vec![qname("pg_catalog", "simple")],
            },
        ];
        cat.ts_configurations.push(c);
        run(&mut cat).unwrap();
        let mappings = &cat.ts_configurations[0].mappings;
        assert_eq!(mappings[0].token_type, "asciiword");
        assert_eq!(mappings[1].token_type, "numword");
        assert_eq!(mappings[2].token_type, "word");
    }

    // ---- both kinds independently clean ----

    #[test]
    fn both_kinds_pass_when_clean() {
        let mut cat = Catalog::empty();
        cat.ts_dictionaries.push(dict("public", "dict_a"));
        cat.ts_dictionaries.push(dict("public", "dict_b"));
        cat.ts_configurations.push(cfg("public", "cfg_a"));
        cat.ts_configurations.push(cfg("public", "cfg_b"));
        run(&mut cat).unwrap();
        assert_eq!(cat.ts_dictionaries.len(), 2);
        assert_eq!(cat.ts_configurations.len(), 2);
    }

    // ---- dictionary dup does not mask configuration dup (and vice versa) ----

    #[test]
    fn dictionary_dup_does_not_affect_configurations() {
        let mut cat = Catalog::empty();
        cat.ts_dictionaries.push(dict("public", "dup_dict"));
        cat.ts_dictionaries.push(dict("public", "dup_dict"));
        // Even though configurations are clean, dictionary dup fires first.
        cat.ts_configurations.push(cfg("public", "cfg_a"));
        assert!(matches!(
            run(&mut cat).unwrap_err(),
            IrError::DuplicateTsDictionary(_)
        ));
    }
}
