//! Host-side environment expansion for plugin settings.
//!
//! Expansion is a granted capability: it only runs over a plugin's opaque
//! `settings` value when `permissions.env_expansion` is true. Plugins never
//! see an environment enumeration or a `getenv` interface — only resolved
//! strings. The supported syntax is a deliberately limited shell subset:
//!
//! ```text
//! $VAR             empty string when unset
//! ${VAR}           empty string when unset
//! ${VAR:-default}  default when unset or empty
//! ${VAR-default}   default only when unset
//! ${VAR:?message}  error when unset or empty
//! ${VAR?message}   error only when unset
//! $$               literal dollar sign
//! ```
//!
//! Command substitution, backticks, assignments, and arithmetic are never
//! supported. A `$` not followed by `$`, `{`, or a variable start stays
//! literal.

use anyhow::{bail, Result};

/// Recursively expands every string in a settings value.
pub fn expand_settings(
    value: &serde_json::Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<serde_json::Value> {
    Ok(match value {
        serde_json::Value::String(text) => serde_json::Value::String(expand_str(text, lookup)?),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| expand_settings(item, lookup))
                .collect::<Result<_>>()?,
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, item)| Ok((key.clone(), expand_settings(item, lookup)?)))
                .collect::<Result<_>>()?,
        ),
        other => other.clone(),
    })
}

/// Expands one string using the syntax documented on this module.
pub fn expand_str(input: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().map(|(_, next)| *next) {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('{') => {
                chars.next();
                let rest = &input[index..];
                let Some(close) = rest.find('}') else {
                    bail!("unterminated ${{...}} expression in {input:?}");
                };
                let expression = &rest[2..close];
                out.push_str(&expand_braced(expression, input, lookup)?);
                // Skip everything up to and including the closing brace.
                while chars
                    .peek()
                    .is_some_and(|(position, _)| *position <= index + close)
                {
                    chars.next();
                }
            }
            Some(next) if next == '_' || next.is_ascii_alphabetic() => {
                let mut name = String::new();
                while let Some((_, next)) = chars.peek() {
                    if *next == '_' || next.is_ascii_alphanumeric() {
                        name.push(*next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str(&lookup(&name).unwrap_or_default());
            }
            _ => out.push('$'),
        }
    }
    Ok(out)
}

fn expand_braced(
    expression: &str,
    input: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String> {
    let operator_start = expression
        .char_indices()
        .find(|(index, c)| *index > 0 && matches!(c, ':' | '-' | '?'))
        .map(|(index, _)| index);
    let (name, operator) = match operator_start {
        Some(index) => (&expression[..index], &expression[index..]),
        None => (expression, ""),
    };
    if name.is_empty()
        || !name.chars().enumerate().all(|(index, c)| {
            c == '_' || c.is_ascii_alphabetic() || (index > 0 && c.is_ascii_digit())
        })
    {
        bail!("invalid variable name {name:?} in {input:?}");
    }

    let value = lookup(name);
    let (unset_only, operator) = match operator.strip_prefix(':') {
        Some(rest) => (false, rest),
        None => (true, operator),
    };
    let missing = match (&value, unset_only) {
        (None, _) => true,
        (Some(value), false) => value.is_empty(),
        (Some(_), true) => false,
    };

    if let Some(default) = operator.strip_prefix('-') {
        return Ok(if missing {
            default.to_string()
        } else {
            value.unwrap_or_default()
        });
    }
    if let Some(message) = operator.strip_prefix('?') {
        if missing {
            let message = if message.is_empty() {
                format!("required environment variable {name} is not set")
            } else {
                message.to_string()
            };
            bail!("{message}");
        }
        return Ok(value.unwrap_or_default());
    }
    if !operator.is_empty() {
        bail!("unsupported expansion operator in ${{{expression}}}");
    }
    Ok(value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "TOKEN" => Some("secret".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    #[test]
    fn expands_bare_and_braced_variables() {
        assert_eq!(expand_str("x $TOKEN y", &env).unwrap(), "x secret y");
        assert_eq!(expand_str("x ${TOKEN} y", &env).unwrap(), "x secret y");
        assert_eq!(expand_str("$MISSING", &env).unwrap(), "");
        assert_eq!(expand_str("${MISSING}", &env).unwrap(), "");
    }

    #[test]
    fn defaults_distinguish_unset_from_empty() {
        assert_eq!(expand_str("${MISSING:-d}", &env).unwrap(), "d");
        assert_eq!(expand_str("${EMPTY:-d}", &env).unwrap(), "d");
        assert_eq!(expand_str("${MISSING-d}", &env).unwrap(), "d");
        assert_eq!(expand_str("${EMPTY-d}", &env).unwrap(), "");
        assert_eq!(expand_str("${TOKEN:-d}", &env).unwrap(), "secret");
    }

    #[test]
    fn required_variables_error_with_the_message() {
        let error = expand_str("${MISSING:?token is required}", &env)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "token is required");
        assert!(expand_str("${EMPTY:?msg}", &env).is_err());
        assert_eq!(expand_str("${EMPTY?msg}", &env).unwrap(), "");
        assert_eq!(expand_str("${TOKEN?msg}", &env).unwrap(), "secret");
        let error = expand_str("${MISSING?}", &env).unwrap_err().to_string();
        assert!(error.contains("MISSING"), "{error}");
    }

    #[test]
    fn double_dollar_escapes_a_literal_dollar() {
        assert_eq!(
            expand_str("cost: $$5 $$TOKEN", &env).unwrap(),
            "cost: $5 $TOKEN"
        );
    }

    #[test]
    fn lone_dollar_stays_literal() {
        assert_eq!(expand_str("100$ and $ x", &env).unwrap(), "100$ and $ x");
        assert_eq!(expand_str("$1", &env).unwrap(), "$1");
    }

    #[test]
    fn unterminated_and_invalid_expressions_error() {
        assert!(expand_str("${TOKEN", &env).is_err());
        assert!(expand_str("${1BAD}", &env).is_err());
        assert!(expand_str("${TOKEN+x}", &env).is_err());
    }

    #[test]
    fn expands_nested_settings_values() {
        let settings = serde_json::json!({
            "token": "${TOKEN}",
            "list": ["$TOKEN", 5, true],
            "nested": {"url": "https://x/$TOKEN"},
        });
        let expanded = expand_settings(&settings, &env).unwrap();
        assert_eq!(
            expanded,
            serde_json::json!({
                "token": "secret",
                "list": ["secret", 5, true],
                "nested": {"url": "https://x/secret"},
            })
        );
    }
}
