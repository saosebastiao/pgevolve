//! SQL rendering for SUBSCRIPTION operations.
//!
//! Every public function corresponds to one DML kind on a Postgres SUBSCRIPTION.
//! All helpers return a complete SQL statement including the trailing semicolon.
//!
//! **Important split**: `render_options_body_for_create` includes CREATE-only
//! options (`create_slot`, `copy_data`); `render_options_body_for_alter` omits
//! them. PG rejects `ALTER SUBSCRIPTION s SET (create_slot = …)` — these
//! options only exist at CREATE time. The alter helpers call
//! `render_options_body_for_alter` as a defense-in-depth filter (the differ's
//! `options_delta` also strips them, but the renderer is the last line of defense).

use crate::identifier::Identifier;
use crate::ir::subscription::{OriginMode, StreamingMode, Subscription, SubscriptionOptions};

/// `CREATE SUBSCRIPTION s CONNECTION '...' PUBLICATION ... WITH (...);`
#[must_use]
pub fn create_subscription(s: &Subscription) -> String {
    let mut out = format!("CREATE SUBSCRIPTION {} ", s.name.render_sql());
    out.push_str(&format!(
        "CONNECTION '{}' ",
        escape_sql_literal(&s.connection)
    ));
    out.push_str("PUBLICATION ");
    let pubs: Vec<String> = s.publications.iter().map(Identifier::render_sql).collect();
    out.push_str(&pubs.join(", "));
    let with = render_with_options(&s.options);
    if !with.is_empty() {
        out.push(' ');
        out.push_str(&with);
    }
    out.push(';');
    out
}

/// `DROP SUBSCRIPTION s;`
#[must_use]
pub fn drop_subscription(name: &Identifier) -> String {
    format!("DROP SUBSCRIPTION {};", name.render_sql())
}

/// `ALTER SUBSCRIPTION s CONNECTION '...';`
#[must_use]
pub fn alter_subscription_connection(name: &Identifier, new_connection: &str) -> String {
    format!(
        "ALTER SUBSCRIPTION {} CONNECTION '{}';",
        name.render_sql(),
        escape_sql_literal(new_connection),
    )
}

/// `ALTER SUBSCRIPTION s ADD PUBLICATION p;`
#[must_use]
pub fn alter_subscription_add_publication(name: &Identifier, publication: &Identifier) -> String {
    format!(
        "ALTER SUBSCRIPTION {} ADD PUBLICATION {};",
        name.render_sql(),
        publication.render_sql(),
    )
}

/// `ALTER SUBSCRIPTION s DROP PUBLICATION p;`
#[must_use]
pub fn alter_subscription_drop_publication(name: &Identifier, publication: &Identifier) -> String {
    format!(
        "ALTER SUBSCRIPTION {} DROP PUBLICATION {};",
        name.render_sql(),
        publication.render_sql(),
    )
}

/// `ALTER SUBSCRIPTION s SET PUBLICATION p, ...;`
///
/// Note: the differ emits granular ADD/DROP; this helper exists for
/// `StepKind::AlterSubscriptionSetPublication` round-trips only.
#[must_use]
pub fn alter_subscription_set_publication(
    name: &Identifier,
    publications: &[Identifier],
) -> String {
    let pubs: Vec<String> = publications.iter().map(Identifier::render_sql).collect();
    format!(
        "ALTER SUBSCRIPTION {} SET PUBLICATION {};",
        name.render_sql(),
        pubs.join(", "),
    )
}

/// `ALTER SUBSCRIPTION s SET (option = value, ...);`
///
/// Uses `render_options_body_for_alter` which OMITS `create_slot` and
/// `copy_data` — those are CREATE-only PG options. The differ's
/// `options_delta` also strips them, but this is a defense-in-depth filter.
#[must_use]
pub fn alter_subscription_set_options(name: &Identifier, opts: &SubscriptionOptions) -> String {
    let body = render_options_body_for_alter(opts);
    format!("ALTER SUBSCRIPTION {} SET ({body});", name.render_sql())
}

/// `COMMENT ON SUBSCRIPTION s IS '...' | NULL;`
#[must_use]
pub fn comment_on_subscription(name: &Identifier, comment: Option<&str>) -> String {
    let body = comment.map_or_else(
        || "NULL".to_string(),
        |c| format!("'{}'", c.replace('\'', "''")),
    );
    format!("COMMENT ON SUBSCRIPTION {} IS {body};", name.render_sql())
}

// ---- private helpers ----

/// Wrap `render_options_body_for_create` in `WITH (…)` if non-empty.
fn render_with_options(opts: &SubscriptionOptions) -> String {
    let body = render_options_body_for_create(opts);
    if body.is_empty() {
        String::new()
    } else {
        format!("WITH ({body})")
    }
}

/// Render all WITH options including CREATE-only `create_slot` + `copy_data`.
/// Used only by `create_subscription`.
fn render_options_body_for_create(opts: &SubscriptionOptions) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = opts.enabled {
        parts.push(format!("enabled = {v}"));
    }
    if let Some(ref v) = opts.slot_name {
        parts.push(format!("slot_name = {}", v.render_sql()));
    }
    if let Some(v) = opts.create_slot {
        parts.push(format!("create_slot = {v}"));
    }
    if let Some(v) = opts.copy_data {
        parts.push(format!("copy_data = {v}"));
    }
    push_alterable_options(opts, &mut parts);
    parts.join(", ")
}

/// Render only the ALTER-able WITH options. Omits `create_slot` and `copy_data`.
/// Used by `alter_subscription_set_options`.
fn render_options_body_for_alter(opts: &SubscriptionOptions) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = opts.enabled {
        parts.push(format!("enabled = {v}"));
    }
    if let Some(ref v) = opts.slot_name {
        parts.push(format!("slot_name = {}", v.render_sql()));
    }
    // create_slot and copy_data intentionally omitted — PG rejects them in ALTER.
    push_alterable_options(opts, &mut parts);
    parts.join(", ")
}

/// Shared rendering for the post-slot_name options (all ALTER-able).
fn push_alterable_options(opts: &SubscriptionOptions, parts: &mut Vec<String>) {
    if let Some(ref v) = opts.synchronous_commit {
        parts.push(format!("synchronous_commit = '{}'", v.replace('\'', "''")));
    }
    if let Some(v) = opts.binary {
        parts.push(format!("binary = {v}"));
    }
    if let Some(v) = opts.streaming {
        parts.push(format!("streaming = {}", streaming_keyword(v)));
    }
    if let Some(v) = opts.two_phase {
        parts.push(format!("two_phase = {v}"));
    }
    if let Some(v) = opts.disable_on_error {
        parts.push(format!("disable_on_error = {v}"));
    }
    if let Some(v) = opts.password_required {
        parts.push(format!("password_required = {v}"));
    }
    if let Some(v) = opts.run_as_owner {
        parts.push(format!("run_as_owner = {v}"));
    }
    if let Some(v) = opts.origin {
        parts.push(format!("origin = {}", origin_keyword(v)));
    }
    if let Some(v) = opts.failover {
        parts.push(format!("failover = {v}"));
    }
}

const fn streaming_keyword(m: StreamingMode) -> &'static str {
    match m {
        StreamingMode::Off => "off",
        StreamingMode::On => "on",
        StreamingMode::Parallel => "parallel",
    }
}

const fn origin_keyword(m: OriginMode) -> &'static str {
    match m {
        OriginMode::Any => "any",
        OriginMode::None => "none",
    }
}

fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::subscription::{OriginMode, StreamingMode, Subscription, SubscriptionOptions};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn minimal_sub() -> Subscription {
        Subscription {
            name: id("mysub"),
            connection: "host=db.example.com dbname=app".to_string(),
            publications: vec![id("mypub")],
            options: SubscriptionOptions::default(),
            owner: None,
            comment: None,
        }
    }

    #[test]
    fn create_subscription_minimal_no_with() {
        let s = minimal_sub();
        let sql = create_subscription(&s);
        assert_eq!(
            sql,
            "CREATE SUBSCRIPTION mysub CONNECTION 'host=db.example.com dbname=app' PUBLICATION mypub;"
        );
    }

    #[test]
    fn create_subscription_multi_publication() {
        let mut s = minimal_sub();
        s.publications = vec![id("p1"), id("p2")];
        let sql = create_subscription(&s);
        assert!(sql.contains("PUBLICATION p1, p2"));
    }

    #[test]
    fn create_subscription_with_create_slot_false() {
        let mut s = minimal_sub();
        s.options.create_slot = Some(false);
        s.options.copy_data = Some(false);
        s.options.enabled = Some(false);
        let sql = create_subscription(&s);
        assert!(sql.contains("WITH ("));
        assert!(sql.contains("enabled = false"));
        assert!(sql.contains("create_slot = false"));
        assert!(sql.contains("copy_data = false"));
    }

    #[test]
    fn create_subscription_with_var_in_connection_stored_verbatim() {
        let mut s = minimal_sub();
        s.connection = "host=db.example.com password=${REPL_PASSWORD}".to_string();
        let sql = create_subscription(&s);
        // The ${VAR} form must appear literally in the output — resolution is
        // apply-time only.
        assert!(sql.contains("${REPL_PASSWORD}"));
    }

    #[test]
    fn create_subscription_connection_single_quotes_escaped() {
        let mut s = minimal_sub();
        s.connection = "host=it's.db".to_string();
        let sql = create_subscription(&s);
        assert!(sql.contains("host=it''s.db"));
    }

    #[test]
    fn drop_subscription_renders_correctly() {
        let sql = drop_subscription(&id("mysub"));
        assert_eq!(sql, "DROP SUBSCRIPTION mysub;");
    }

    #[test]
    fn alter_subscription_connection_renders_correctly() {
        let sql = alter_subscription_connection(&id("mysub"), "host=new.db");
        assert_eq!(sql, "ALTER SUBSCRIPTION mysub CONNECTION 'host=new.db';");
    }

    #[test]
    fn alter_subscription_add_publication_renders_correctly() {
        let sql = alter_subscription_add_publication(&id("mysub"), &id("newpub"));
        assert_eq!(sql, "ALTER SUBSCRIPTION mysub ADD PUBLICATION newpub;");
    }

    #[test]
    fn alter_subscription_drop_publication_renders_correctly() {
        let sql = alter_subscription_drop_publication(&id("mysub"), &id("oldpub"));
        assert_eq!(sql, "ALTER SUBSCRIPTION mysub DROP PUBLICATION oldpub;");
    }

    #[test]
    fn alter_subscription_set_publication_renders_correctly() {
        let pubs = vec![id("p1"), id("p2")];
        let sql = alter_subscription_set_publication(&id("mysub"), &pubs);
        assert_eq!(sql, "ALTER SUBSCRIPTION mysub SET PUBLICATION p1, p2;");
    }

    #[test]
    fn alter_subscription_set_options_single_field() {
        let opts = SubscriptionOptions {
            binary: Some(true),
            ..Default::default()
        };
        let sql = alter_subscription_set_options(&id("mysub"), &opts);
        assert_eq!(sql, "ALTER SUBSCRIPTION mysub SET (binary = true);");
    }

    #[test]
    fn alter_subscription_set_options_does_not_include_create_slot() {
        // Defense-in-depth: even if create_slot is set in opts, the ALTER
        // helper must NOT emit it (PG rejects it).
        let opts = SubscriptionOptions {
            create_slot: Some(true),
            copy_data: Some(true),
            binary: Some(false),
            ..Default::default()
        };
        let sql = alter_subscription_set_options(&id("mysub"), &opts);
        assert!(
            !sql.contains("create_slot"),
            "create_slot must not appear in ALTER SET"
        );
        assert!(
            !sql.contains("copy_data"),
            "copy_data must not appear in ALTER SET"
        );
        assert!(sql.contains("binary = false"));
    }

    #[test]
    fn alter_subscription_set_options_streaming_mode() {
        let opts = SubscriptionOptions {
            streaming: Some(StreamingMode::Parallel),
            ..Default::default()
        };
        let sql = alter_subscription_set_options(&id("mysub"), &opts);
        assert!(sql.contains("streaming = parallel"));
    }

    #[test]
    fn alter_subscription_set_options_origin_none() {
        let opts = SubscriptionOptions {
            origin: Some(OriginMode::None),
            ..Default::default()
        };
        let sql = alter_subscription_set_options(&id("mysub"), &opts);
        assert!(sql.contains("origin = none"));
    }

    #[test]
    fn comment_on_subscription_with_text() {
        let sql = comment_on_subscription(&id("mysub"), Some("my comment"));
        assert_eq!(sql, "COMMENT ON SUBSCRIPTION mysub IS 'my comment';");
    }

    #[test]
    fn comment_on_subscription_null_clears() {
        let sql = comment_on_subscription(&id("mysub"), None);
        assert_eq!(sql, "COMMENT ON SUBSCRIPTION mysub IS NULL;");
    }

    #[test]
    fn streaming_keyword_round_trip() {
        assert_eq!(streaming_keyword(StreamingMode::Off), "off");
        assert_eq!(streaming_keyword(StreamingMode::On), "on");
        assert_eq!(streaming_keyword(StreamingMode::Parallel), "parallel");
    }

    #[test]
    fn origin_keyword_round_trip() {
        assert_eq!(origin_keyword(OriginMode::Any), "any");
        assert_eq!(origin_keyword(OriginMode::None), "none");
    }

    #[test]
    fn all_alterable_options_rendered_by_set_options() {
        let opts = SubscriptionOptions {
            enabled: Some(true),
            synchronous_commit: Some("off".to_string()),
            binary: Some(true),
            streaming: Some(StreamingMode::On),
            two_phase: Some(false),
            disable_on_error: Some(true),
            password_required: Some(false),
            run_as_owner: Some(true),
            origin: Some(OriginMode::Any),
            failover: Some(false),
            ..Default::default()
        };
        let sql = alter_subscription_set_options(&id("s"), &opts);
        assert!(sql.contains("enabled = true"));
        assert!(sql.contains("synchronous_commit = 'off'"));
        assert!(sql.contains("binary = true"));
        assert!(sql.contains("streaming = on"));
        assert!(sql.contains("two_phase = false"));
        assert!(sql.contains("disable_on_error = true"));
        assert!(sql.contains("password_required = false"));
        assert!(sql.contains("run_as_owner = true"));
        assert!(sql.contains("origin = any"));
        assert!(sql.contains("failover = false"));
    }
}
