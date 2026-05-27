//! Errors when source subscription CONNECTION contains a `password=` value
//! that isn't a `${VAR}` env-var reference.
//!
//! Catching plaintext credentials at parse/lint time prevents accidental
//! secret commits. Sources must use `${ENV_VAR}` interpolation; the secret
//! is resolved at apply-time preflight and never persisted.
//!
//! Source-only rule; fires `Severity::Error` (not waivable — plaintext
//! passwords in source is a hard security violation, not a best-practice warn).

use crate::ir::catalog::Catalog;
use crate::lint::finding::{Finding, Severity};

pub const RULE_ID: &str = "subscription-password-in-source";

/// For each subscription, extract any `password=…` value from the CONNECTION
/// string and fire if it's not a `${VAR}` env-var reference.
pub fn check(source: &Catalog) -> Vec<Finding> {
    let mut findings = Vec::new();
    for s in &source.subscriptions {
        if let Some(value) = extract_password_value(&s.connection)
            && !is_env_var_ref(&value)
        {
            findings.push(Finding {
                rule: RULE_ID,
                severity: Severity::Error,
                message: format!(
                    "subscription {} CONNECTION contains plaintext password; \
                     use ${{ENV_VAR}} reference instead",
                    s.name,
                ),
                location: None,
            });
        }
    }
    findings
}

/// Find a `password=…` value in a libpq connstr. Returns `None` if absent.
///
/// Case-insensitive on the key; handles quoted (`'…'`) and unquoted values.
/// Quoted values support `\\` escape sequences and `\'` within the quoted span.
fn extract_password_value(connstr: &str) -> Option<String> {
    let mut chars = connstr.chars().peekable();
    loop {
        // Skip leading whitespace before next key.
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek().is_none() {
            break None;
        }
        // Consume key up to '='.
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                chars.next();
                break;
            }
            key.push(c);
            chars.next();
        }
        // Consume value (quoted or unquoted).
        let mut value = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next(); // consume opening quote
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(esc) = chars.next() {
                            value.push(esc);
                        }
                    }
                    '\'' => break, // closing quote
                    other => value.push(other),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        if key.trim().eq_ignore_ascii_case("password") {
            return Some(value);
        }
    }
}

/// Returns `true` if `value` is exactly `${NAME}` where `NAME` matches
/// `[A-Z_][A-Z0-9_]*`.
///
/// Partial concatenations like `prefix${VAR}suffix` return `false` — only a
/// pure env-var placeholder is considered safe.
fn is_env_var_ref(value: &str) -> bool {
    if !value.starts_with("${") || !value.ends_with('}') {
        return false;
    }
    let inner = &value[2..value.len() - 1];
    if inner.is_empty() {
        return false;
    }
    let mut chars = inner.chars();
    // SAFETY: inner is non-empty, so next() always succeeds.
    #[allow(clippy::unwrap_used)]
    let first = chars.next().unwrap();
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identifier::Identifier;
    use crate::ir::catalog::Catalog;
    use crate::ir::subscription::{Subscription, SubscriptionOptions};

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    fn make_sub(connection: &str) -> Subscription {
        Subscription {
            name: id("s"),
            connection: connection.into(),
            publications: vec![id("p")],
            options: SubscriptionOptions::default(),
            owner: None,
            comment: None,
        }
    }

    fn catalog_with(connection: &str) -> Catalog {
        let mut cat = Catalog::empty();
        cat.subscriptions.push(make_sub(connection));
        cat
    }

    // ── extract_password_value unit tests ───────────────────────────────────

    #[test]
    fn extracts_unquoted_password() {
        assert_eq!(
            extract_password_value("host=x password=secret"),
            Some("secret".into())
        );
    }

    #[test]
    fn extracts_quoted_password() {
        assert_eq!(
            extract_password_value("host=x password='hunter2'"),
            Some("hunter2".into())
        );
    }

    #[test]
    fn no_password_returns_none() {
        assert_eq!(extract_password_value("host=x dbname=app user=repl"), None);
    }

    #[test]
    fn password_key_case_insensitive() {
        assert_eq!(
            extract_password_value("host=x PASSWORD=plain"),
            Some("plain".into())
        );
    }

    // ── is_env_var_ref unit tests ───────────────────────────────────────────

    #[test]
    fn env_var_ref_valid() {
        assert!(is_env_var_ref("${REPL_PWD}"));
        assert!(is_env_var_ref("${A}"));
        assert!(is_env_var_ref("${MY_VAR_2}"));
        assert!(is_env_var_ref("${_UNDERSCORE}"));
    }

    #[test]
    fn env_var_ref_partial_concat_rejected() {
        assert!(!is_env_var_ref("prefix${VAR}suffix"));
        assert!(!is_env_var_ref("${VAR}suffix"));
        assert!(!is_env_var_ref("prefix${VAR}"));
    }

    #[test]
    fn env_var_ref_lowercase_rejected() {
        assert!(!is_env_var_ref("${lowercase}"));
    }

    #[test]
    fn env_var_ref_empty_inner_rejected() {
        assert!(!is_env_var_ref("${}"));
    }

    // ── check() integration tests ───────────────────────────────────────────

    #[test]
    fn plaintext_password_fires() {
        let source = catalog_with("host=x password=secret");
        let findings = check(&source);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, RULE_ID);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("plaintext password"));
    }

    #[test]
    fn env_var_password_silent() {
        let source = catalog_with("host=x password=${REPL_PWD}");
        assert!(check(&source).is_empty());
    }

    #[test]
    fn quoted_plaintext_fires() {
        let source = catalog_with("host=x password='hunter2'");
        let findings = check(&source);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
    }

    #[test]
    fn no_password_silent() {
        let source = catalog_with("host=x dbname=app user=repl");
        assert!(check(&source).is_empty());
    }

    #[test]
    fn case_insensitive_password_key_fires() {
        let source = catalog_with("host=x PASSWORD=plain");
        let findings = check(&source);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn env_var_ref_in_quoted_position_silent() {
        // PASSWORD='${REPL_PWD}' — the quoting is stripped; inner value is ${REPL_PWD}.
        let source = catalog_with("host=x password='${REPL_PWD}'");
        assert!(check(&source).is_empty());
    }
}
