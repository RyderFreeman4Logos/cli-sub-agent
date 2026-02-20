use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

struct RedactionPatterns {
    api_key: Regex,
    token: Regex,
    secret_kv: Regex,
    private_key_block: Regex,
}

fn build_redaction_patterns() -> Option<RedactionPatterns> {
    Some(RedactionPatterns {
        api_key: Regex::new(
            r#"(?ix)
                \b(?:sk|key)-[a-z0-9][a-z0-9_-]{7,}\b
                |
                \bAKIA[0-9A-Z]{16}\b
            "#,
        )
        .ok()?,
        token: Regex::new(
            r#"(?ix)
                \bBearer\s+[A-Za-z0-9._~+/\-]+=*
                |
                \b[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b
                |
                \b(?:access_token|refresh_token|id_token)\b\s*[:=]\s*["']?[^"',\s}]+["']?
            "#,
        )
        .ok()?,
        secret_kv: Regex::new(
            r#"(?ix)
                (?:
                    \b(?:password|passwd|pwd|secret|client_secret|api_key|token|access_token|refresh_token|id_token)\b
                    \s*[:=]\s*
                    (?:
                        "(?:\\.|[^"])*"
                        |
                        '(?:\\.|[^'])*'
                        |
                        [^\s,}]+
                    )
                    |
                    (?:\\?")(?:password|passwd|pwd|secret|client_secret|api_key|token|access_token|refresh_token|id_token)(?:\\?")
                    \s*:\s*
                    (?:\\?")(?:\\.|[^"\\])*(?:\\?")
                )
            "#,
        )
        .ok()?,
        private_key_block: Regex::new(r#"(?s)-----BEGIN [^-]+ KEY-----.*?-----END [^-]+ KEY-----"#)
            .ok()?,
    })
}

fn redaction_patterns() -> Option<&'static RedactionPatterns> {
    static PATTERNS: OnceLock<Option<RedactionPatterns>> = OnceLock::new();
    PATTERNS.get_or_init(build_redaction_patterns).as_ref()
}

fn redact_text(input: &str, patterns: &RedactionPatterns) -> String {
    let mut redacted = input.to_string();
    for pattern in [
        &patterns.private_key_block,
        &patterns.api_key,
        &patterns.token,
        &patterns.secret_kv,
    ] {
        redacted = pattern.replace_all(&redacted, "[REDACTED]").into_owned();
    }
    redacted
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "password"
            | "passwd"
            | "pwd"
            | "secret"
            | "clientsecret"
            | "apikey"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
    )
}

fn redact_nested_json_string(input: &str, patterns: &RedactionPatterns) -> Option<String> {
    let mut nested = serde_json::from_str::<Value>(input).ok()?;
    redact_json_value(&mut nested, None, patterns);
    serde_json::to_string(&nested).ok()
}

fn redact_json_value(value: &mut Value, key: Option<&str>, patterns: &RedactionPatterns) {
    let key_is_sensitive = key.is_some_and(is_sensitive_key);
    match value {
        Value::Object(map) => {
            for (child_key, child_value) in map {
                redact_json_value(child_value, Some(child_key), patterns);
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item, None, patterns);
            }
        }
        Value::String(text) => {
            if key_is_sensitive {
                *text = "[REDACTED]".to_string();
                return;
            }
            if let Some(redacted_nested) = redact_nested_json_string(text, patterns) {
                *text = redacted_nested;
                return;
            }
            *text = redact_text(text, patterns);
        }
        _ => {
            if key_is_sensitive {
                *value = Value::String("[REDACTED]".to_string());
            }
        }
    }
}

/// Redact sensitive material from serialized JSON event lines.
///
/// This function is intentionally string-based so it can run on fully
/// serialized event payloads without touching runtime event flow.
pub fn redact_event(serialized_json: &str) -> String {
    let Some(patterns) = redaction_patterns() else {
        return serialized_json.to_string();
    };

    if let Ok(mut structured) = serde_json::from_str::<Value>(serialized_json) {
        redact_json_value(&mut structured, None, patterns);
        if let Ok(redacted) = serde_json::to_string(&structured) {
            return redacted;
        }
    }

    redact_text(serialized_json, patterns)
}

#[cfg(test)]
mod tests {
    use super::redact_event;

    #[test]
    fn test_redact_event_masks_api_keys() {
        let line = r#"{"type":"message","data":"use sk-test_123456789 and key-prod_987654321 and AKIA1234567890ABCDEF"}"#;
        let out = redact_event(line);
        assert!(!out.contains("sk-test_123456789"));
        assert!(!out.contains("key-prod_987654321"));
        assert!(!out.contains("AKIA1234567890ABCDEF"));
        assert_eq!(out.matches("[REDACTED]").count(), 3);
    }

    #[test]
    fn test_redact_event_masks_bearer_jwt_and_oauth_tokens() {
        let line = r#"{"data":"Authorization: Bearer abcDEF123._-token access_token=oauth-secret jwt=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signaturetoken"}"#;
        let out = redact_event(line);
        assert!(!out.contains("Bearer abcDEF123._-token"));
        assert!(!out.contains("access_token=oauth-secret"));
        assert!(!out.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_event_masks_password_and_secret_pairs() {
        let line =
            r#"{"data":"password=hunter2 secret=\"top-secret\" client_secret:'ultra-secret'"}"#;
        let out = redact_event(line);
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("top-secret"));
        assert!(!out.contains("ultra-secret"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_event_masks_private_key_blocks() {
        let line = r#"{"data":"-----BEGIN PRIVATE KEY-----\nabc123\n-----END PRIVATE KEY-----"}"#;
        let out = redact_event(line);
        assert!(!out.contains("BEGIN PRIVATE KEY"));
        assert!(!out.contains("abc123"));
        assert!(!out.contains("END PRIVATE KEY"));
        assert_eq!(out, r#"{"data":"[REDACTED]"}"#);
    }

    #[test]
    fn test_redact_event_masks_structured_json_sensitive_fields() {
        let line = r#"{"v":1,"seq":0,"ts":"2026-02-20T00:00:00Z","type":"tool_call","data":{"password":"hunter2","api_key":"sk-abc123","nested":{"secret":"my-secret"}}}"#;
        let out = redact_event(line);
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("my-secret"));
        assert!(!out.contains("sk-abc123"));
        assert!(out.contains(r#""password":"[REDACTED]""#));
        assert!(out.contains(r#""api_key":"[REDACTED]""#));
        assert!(out.contains(r#""secret":"[REDACTED]""#));
    }

    #[test]
    fn test_redact_event_masks_json_escaped_secret_payloads() {
        let line = r#"{"v":1,"seq":0,"ts":"2026-02-20T00:00:00Z","type":"tool_call","data":"{\"password\":\"hunter2\",\"secret\":\"my-secret\",\"api_key\":\"sk-abc123\"}"}"#;
        let out = redact_event(line);
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("my-secret"));
        assert!(!out.contains("sk-abc123"));
        assert!(out.contains(r#"\"password\":\"[REDACTED]\""#));
        assert!(out.contains(r#"\"secret\":\"[REDACTED]\""#));
        assert!(out.contains(r#"\"api_key\":\"[REDACTED]\""#));
    }
}
