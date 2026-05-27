//! Apply-time `${VAR}` env-var interpolation.
//!
//! Source IR stores literal `${NAME}` references in SUBSCRIPTION CONNECTION
//! strings. Resolution happens at apply-time preflight: every step's SQL is
//! scanned, every reference is looked up in process env (or a test override),
//! and missing references cause an `ApplyError::MissingEnvVar` before any
//! DB connection is attempted. plan.sql on disk always contains the
//! unresolved form — secrets are never written to disk.
//!
//! Syntax: `${NAME}`. Only matches `[A-Z_][A-Z0-9_]*` between the braces;
//! anything else is left literal (so legitimate `$1`, `${foo}` from a
//! function body etc. don't accidentally trigger).

use std::fmt;

/// Resolve `${VAR}` references in `template` against `env`. Returns the
/// resolved string, or `MissingEnvVar(name)` if any referenced variable
/// is absent.
///
/// `env` is a closure so tests can inject a controlled environment without
/// touching the process's actual env vars.
///
/// # Errors
///
/// Returns `MissingEnvVar(name)` for the first `${VAR}` reference whose
/// variable name isn't present in `env`.
pub fn resolve<F>(template: &str, env: F) -> Result<String, MissingEnvVar>
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            // Find the closing '}'.
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                let name_start = i + 2;
                let name_end = name_start + end;
                let name = &template[name_start..name_end];
                if is_valid_var_name(name) {
                    match env(name) {
                        Some(v) => {
                            out.push_str(&v);
                            i = name_end + 1;
                            continue;
                        }
                        None => return Err(MissingEnvVar(name.to_string())),
                    }
                }
                // Invalid name shape → leave literal, advance one byte.
            }
        }
        // Default: copy one byte.
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

/// Returned when `resolve` encounters a `${VAR}` whose name isn't in the env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingEnvVar(pub String);

impl fmt::Display for MissingEnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "missing environment variable: {}", self.0)
    }
}

impl std::error::Error for MissingEnvVar {}

fn is_valid_var_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Collect every `${VAR}` reference in `template` without resolving. Useful
/// for preflight summary / error messages.
#[must_use]
pub fn references(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'{'
            && let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'}')
        {
            let name = &template[i + 2..i + 2 + end];
            if is_valid_var_name(name) {
                out.push(name.to_string());
                i = i + 2 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<_, _> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn resolves_single_var() {
        let r = resolve("host=x password=${PW}", env_from(&[("PW", "secret")])).unwrap();
        assert_eq!(r, "host=x password=secret");
    }

    #[test]
    fn resolves_multiple_vars() {
        let r = resolve(
            "host=${H} user=${U} password=${P}",
            env_from(&[("H", "db.example.com"), ("U", "repl"), ("P", "secret")]),
        )
        .unwrap();
        assert_eq!(r, "host=db.example.com user=repl password=secret");
    }

    #[test]
    fn fails_on_missing_var() {
        let err = resolve("password=${MISSING}", env_from(&[])).unwrap_err();
        assert_eq!(err.0, "MISSING");
    }

    #[test]
    fn template_without_vars_is_identity() {
        let r = resolve("host=x dbname=app", env_from(&[])).unwrap();
        assert_eq!(r, "host=x dbname=app");
    }

    #[test]
    fn invalid_var_shapes_are_literal() {
        let r = resolve("foo ${lowercase} bar", env_from(&[])).unwrap();
        assert_eq!(r, "foo ${lowercase} bar");
    }

    #[test]
    fn unclosed_brace_is_literal() {
        let r = resolve("password=${UNCLOSED no end", env_from(&[("UNCLOSED", "x")])).unwrap();
        assert_eq!(r, "password=${UNCLOSED no end");
    }

    #[test]
    fn references_lists_all_vars_in_order() {
        let r = references("host=${H} user=${U} password=${P}");
        assert_eq!(r, vec!["H", "U", "P"]);
    }

    #[test]
    fn references_skips_invalid_names() {
        let r = references("foo ${bad} ${GOOD} ${bad2}");
        assert_eq!(r, vec!["GOOD"]);
    }

    #[test]
    fn underscores_allowed_in_var_names() {
        let r = resolve("password=${MY_VAR_2}", env_from(&[("MY_VAR_2", "x")])).unwrap();
        assert_eq!(r, "password=x");
    }
}
