//! Focused diagnostic sanitizer regressions (UTF-8 / just parser markers).

use super::redact_command_text;

#[test]
fn preflight_just_parser_diagnostics_redact_secret_source_lines() {
    // Malformed just parser diagnostics can emit multi-byte markers such as ▶.
    // Redaction must stay on UTF-8 char boundaries (no panic) while still masking secrets.
    const REJECTED_SECRET: &str = "just-opaque-source-secret";
    let diagnostic = format!(
        "error: Unknown start of token '='\n  ——▶▶▶\n  justfile:1:10\n  --token={REJECTED_SECRET} --header=Authorization:Bearer {REJECTED_SECRET} OPENAI_API_KEY={REJECTED_SECRET}"
    );

    let redacted = redact_command_text(&diagnostic);
    assert!(
        redacted.contains("[REDACTED]"),
        "unicode diagnostic must still redact secrets: {redacted}"
    );
    assert!(
        !redacted.contains(REJECTED_SECRET),
        "unicode diagnostic leaked secret: {redacted}"
    );
    assert!(
        redacted.contains('▶'),
        "unicode marker must remain valid UTF-8 after redaction: {redacted}"
    );
}
