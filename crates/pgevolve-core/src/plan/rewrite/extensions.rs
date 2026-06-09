//! SQL emission for extension planner steps.
//!
//! Each helper produces a single canonical SQL statement string ending
//! with `;`, deterministic for byte-stable plan output.

use crate::identifier::Identifier;
use crate::ir::extension::Extension;

/// `CREATE EXTENSION IF NOT EXISTS "name" [WITH SCHEMA "schema"] [VERSION 'v'];`
#[allow(dead_code)]
pub(crate) fn create_extension(e: &Extension) -> String {
    let mut sql = format!("CREATE EXTENSION IF NOT EXISTS {}", e.name.render_sql());
    if let Some(schema) = &e.schema {
        sql.push_str(&format!(" WITH SCHEMA {}", schema.render_sql()));
    }
    if let Some(version) = &e.version {
        sql.push_str(&format!(
            " VERSION '{}'",
            super::sql::escape_sql_literal_body(version)
        ));
    }
    sql.push(';');
    sql
}

/// `DROP EXTENSION "name" CASCADE;`
#[allow(dead_code)]
pub(crate) fn drop_extension(name: &Identifier) -> String {
    format!("DROP EXTENSION {} CASCADE;", name.render_sql())
}

/// `ALTER EXTENSION "name" UPDATE TO 'v';`
#[allow(dead_code)]
pub(crate) fn alter_extension_update(name: &Identifier, to_version: &str) -> String {
    format!(
        "ALTER EXTENSION {} UPDATE TO '{}';",
        name.render_sql(),
        super::sql::escape_sql_literal_body(to_version),
    )
}

/// `COMMENT ON EXTENSION "name" IS '...';` or `IS NULL;`
#[allow(dead_code)]
pub(crate) fn comment_on_extension(name: &Identifier, comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(
            "COMMENT ON EXTENSION {} IS '{}';",
            name.render_sql(),
            super::sql::escape_sql_literal_body(c),
        ),
        None => format!("COMMENT ON EXTENSION {} IS NULL;", name.render_sql()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn ext_with(name: &str, schema: Option<&str>, version: Option<&str>) -> Extension {
        Extension {
            name: id(name),
            schema: schema.map(id),
            version: version.map(str::to_string),
            comment: None,
        }
    }

    #[test]
    fn create_bare() {
        assert_eq!(
            create_extension(&ext_with("pgcrypto", None, None)),
            "CREATE EXTENSION IF NOT EXISTS pgcrypto;"
        );
    }

    #[test]
    fn create_with_schema_and_version() {
        assert_eq!(
            create_extension(&ext_with("pg_trgm", Some("app"), Some("1.6"))),
            "CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA app VERSION '1.6';"
        );
    }

    #[test]
    fn drop_renders_cascade() {
        assert_eq!(
            drop_extension(&id("pgcrypto")),
            "DROP EXTENSION pgcrypto CASCADE;"
        );
    }

    #[test]
    fn alter_update_to_version() {
        assert_eq!(
            alter_extension_update(&id("pgcrypto"), "1.4"),
            "ALTER EXTENSION pgcrypto UPDATE TO '1.4';"
        );
    }

    #[test]
    fn comment_set_and_clear() {
        assert_eq!(
            comment_on_extension(&id("pgcrypto"), Some("crypto helpers")),
            "COMMENT ON EXTENSION pgcrypto IS 'crypto helpers';"
        );
        assert_eq!(
            comment_on_extension(&id("pgcrypto"), None),
            "COMMENT ON EXTENSION pgcrypto IS NULL;"
        );
    }

    #[test]
    fn escape_single_quote() {
        assert_eq!(
            comment_on_extension(&id("pgcrypto"), Some("it's fine")),
            "COMMENT ON EXTENSION pgcrypto IS 'it''s fine';"
        );
    }
}
