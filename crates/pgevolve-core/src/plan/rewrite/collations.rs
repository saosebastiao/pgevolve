//! SQL emitters for collation [`StepKind`](crate::plan::raw_step::StepKind)s.
//!
//! Each public function returns a complete SQL statement including the
//! trailing semicolon. `ReplaceCollation` has no single emitter — the
//! dispatcher in `plan::rewrite::mod` renders it as two steps
//! (`drop_collation` followed by `create_collation`) so the audit log
//! distinguishes the two halves and the destructive flag is set correctly.
//!
//! ## Locale-collapse rule
//!
//! The IR always stores `lc_collate` + `lc_ctype` separately even when the
//! user wrote the `locale = '…'` shorthand. The renderer collapses back to
//! `locale = '…'` whenever the two fields are byte-equal — both for clarity
//! and to match what `pg_dump` produces.

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::collation::Collation;

/// `CREATE COLLATION qname (provider = …, locale = '…', deterministic = false);`
///
/// When `lc_collate != lc_ctype`, both fields are rendered separately
/// instead of `locale = '…'`. `deterministic = true` is the PG default
/// and omitted from the rendered options.
#[must_use]
pub fn create_collation(c: &Collation) -> String {
    let mut opts: Vec<String> = Vec::with_capacity(4);
    opts.push(format!("provider = {}", c.provider.sql_keyword()));
    if c.lc_collate == c.lc_ctype {
        opts.push(format!(
            "locale = '{}'",
            super::sql::escape_sql_literal_body(&c.lc_collate)
        ));
    } else {
        opts.push(format!(
            "lc_collate = '{}'",
            super::sql::escape_sql_literal_body(&c.lc_collate)
        ));
        opts.push(format!(
            "lc_ctype = '{}'",
            super::sql::escape_sql_literal_body(&c.lc_ctype)
        ));
    }
    if !c.deterministic {
        opts.push("deterministic = false".into());
    }
    format!(
        "CREATE COLLATION {} ({});",
        c.qname.render_sql(),
        opts.join(", "),
    )
}

/// `DROP COLLATION qname;`
#[must_use]
pub fn drop_collation(qname: &QualifiedName) -> String {
    format!("DROP COLLATION {};", qname.render_sql())
}

/// `ALTER COLLATION qname RENAME TO new_name;`
#[must_use]
pub fn rename_collation(from: &QualifiedName, to: &Identifier) -> String {
    format!(
        "ALTER COLLATION {} RENAME TO {};",
        from.render_sql(),
        to.render_sql(),
    )
}

/// `COMMENT ON COLLATION qname IS 'text';` or `IS NULL` when `comment` is
/// `None`.
#[must_use]
pub fn comment_on_collation(qname: &QualifiedName, comment: Option<&str>) -> String {
    let body = comment.map_or_else(
        || "NULL".to_owned(),
        |c| format!("'{}'", super::sql::escape_sql_literal_body(c)),
    );
    format!("COMMENT ON COLLATION {} IS {body};", qname.render_sql())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::collation::CollationProvider;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn qn(schema: &str, name: &str) -> QualifiedName {
        QualifiedName::new(id(schema), id(name))
    }

    fn make(
        provider: CollationProvider,
        lc_collate: &str,
        lc_ctype: &str,
        deterministic: bool,
    ) -> Collation {
        Collation {
            qname: qn("app", "c"),
            provider,
            lc_collate: lc_collate.into(),
            lc_ctype: lc_ctype.into(),
            deterministic,
            version: None,
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn create_collation_collapses_to_locale_when_equal() {
        let c = make(CollationProvider::Libc, "en_US.utf8", "en_US.utf8", true);
        let sql = create_collation(&c);
        assert_eq!(
            sql,
            "CREATE COLLATION app.c (provider = libc, locale = 'en_US.utf8');"
        );
    }

    #[test]
    fn create_collation_renders_separate_fields_when_unequal() {
        let c = make(CollationProvider::Libc, "C", "en_US.utf8", true);
        let sql = create_collation(&c);
        assert_eq!(
            sql,
            "CREATE COLLATION app.c (provider = libc, lc_collate = 'C', lc_ctype = 'en_US.utf8');"
        );
    }

    #[test]
    fn create_collation_omits_deterministic_when_true() {
        let c = make(CollationProvider::Icu, "und", "und", true);
        let sql = create_collation(&c);
        assert!(
            !sql.contains("deterministic"),
            "unexpected `deterministic` clause: {sql}"
        );
    }

    #[test]
    fn create_collation_emits_deterministic_false() {
        let c = make(CollationProvider::Icu, "und", "und", false);
        let sql = create_collation(&c);
        assert_eq!(
            sql,
            "CREATE COLLATION app.c (provider = icu, locale = 'und', deterministic = false);"
        );
    }

    #[test]
    fn create_collation_escapes_single_quotes_in_locale() {
        let c = make(CollationProvider::Libc, "it's", "it's", true);
        let sql = create_collation(&c);
        assert!(
            sql.contains("locale = 'it''s'"),
            "expected escaped quote: {sql}",
        );
    }

    #[test]
    fn create_collation_renders_builtin_provider() {
        let c = make(CollationProvider::Builtin, "C.UTF-8", "C.UTF-8", true);
        let sql = create_collation(&c);
        assert!(sql.contains("provider = builtin"), "{sql}");
    }

    #[test]
    fn drop_collation_renders_correctly() {
        let sql = drop_collation(&qn("app", "legacy"));
        assert_eq!(sql, "DROP COLLATION app.legacy;");
    }

    #[test]
    fn rename_collation_renders_correctly() {
        let sql = rename_collation(&qn("app", "old"), &id("new"));
        assert_eq!(sql, "ALTER COLLATION app.old RENAME TO new;");
    }

    #[test]
    fn comment_on_collation_with_text() {
        let sql = comment_on_collation(&qn("app", "c"), Some("CI sort"));
        assert_eq!(sql, "COMMENT ON COLLATION app.c IS 'CI sort';");
    }

    #[test]
    fn comment_on_collation_null_clears() {
        let sql = comment_on_collation(&qn("app", "c"), None);
        assert_eq!(sql, "COMMENT ON COLLATION app.c IS NULL;");
    }

    #[test]
    fn comment_on_collation_escapes_single_quotes() {
        let sql = comment_on_collation(&qn("app", "c"), Some("it's a sort"));
        assert_eq!(sql, "COMMENT ON COLLATION app.c IS 'it''s a sort';");
    }
}
