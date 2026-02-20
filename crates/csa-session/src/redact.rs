use regex::Regex;
use std::sync::OnceLock;

struct RedactionPatterns {
    api_key: Regex,
    token: Regex,
    secret_kv: Regex,
    private_key_block: Regex,
}

fn redaction_patterns() -> &'static RedactionPatterns {
    static PATTERNS: OnceLock<RedactionPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| RedactionPatterns {
        api_key: Regex::new(
            r#"(?ix)
                \b(?:sk|key)-[a-z0-9][a-z0-9_-]{7,}\b
                |
                \bAKIA[0-9A-Z]{16}\b
            "#,
        )
        .expect("api key regex must compile"),
        token: Regex::new(
            r#"(?ix)
                \bBearer\s+[A-Za-z0-9._~+/\-]+=*
                |
                \b[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b
                |
                \b(?:access_token|refresh_token|id_token)\b\s*[:=]\s*["']?[^"',\s}]+["']?
            "#,
        )
        .expect("token regex must compile"),
        secret_kv: Regex::new(
            r#"(?ix)
                \b(?:password|passwd|pwd|secret|client_secret|api_key)\b
                \s*[:=]\s*
                (?:
                    "(?:\\.|[^"])*"
                    |
                    '(?:\\.|[^'])*'
                    |
                    [^\s,}]+
                )
            "#,
        )
        .expect("secret regex must compile"),
        private_key_block: Regex::new(r#"(?s)-----BEGIN [^-]+ KEY-----.*?-----END [^-]+ KEY-----"#)
            .expect("private key regex must compile"),
    })
}

/// Redact sensitive material from serialized JSON event lines.
///
/// This function is intentionally string-based so it can run on fully
/// serialized event payloads without touching runtime event flow.
pub fn redact_event(serialized_json: &str) -> String {
    let patterns = redaction_patterns();
    let mut redacted = serialized_json.to_string();
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
}
