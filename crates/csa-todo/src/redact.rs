//! Multi-layer redaction pipeline for TODO plan content.
//!
//! Layer 1: Known pattern matching — deterministic regex-based redaction of API keys,
//! tokens, and other secrets.
//!
//! Layer 2: High-entropy flagging — informational detection of strings that look
//! like secrets based on Shannon entropy, without replacement.

use regex::Regex;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Known-pattern regexes (compiled once)
// ---------------------------------------------------------------------------

static KNOWN_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // OpenAI keys
        r"sk-[a-zA-Z0-9_-]{20,}",
        // Google AI keys
        r"AIza[a-zA-Z0-9_-]{35,}",
        // Groq keys
        r"gsk_[a-zA-Z0-9_-]{20,}",
        // GitHub tokens (ghp_, gho_, ghu_, ghs_, ghr_)
        r"gh[pousr]_[a-zA-Z0-9]{36,}",
        // Bearer tokens
        r"Bearer\s+[a-zA-Z0-9._-]{20,}",
        // Generic API key in key=value context (quoted strings ≥32 chars near key-like names)
        r#"(?i)(?:api[_-]?key|secret|token|password)\s*[:=]\s*['"][A-Za-z0-9_\-/.+]{32,}['"]"#,
    ]
    .iter()
    .map(|pat| Regex::new(pat).expect("built-in redaction regex must compile"))
    .collect()
});

const REDACTED: &str = "[REDACTED]";

// ---------------------------------------------------------------------------
// Layer 1: Known-pattern redaction
// ---------------------------------------------------------------------------

/// Replace known secret patterns with `[REDACTED]`.
///
/// Returns the redacted text and the number of replacements made.
pub fn redact_known_patterns(text: &str) -> (String, usize) {
    let mut result = text.to_string();
    let mut total_count = 0usize;

    for pattern in KNOWN_PATTERNS.iter() {
        let mut count = 0usize;
        result = pattern
            .replace_all(&result, |_caps: &regex::Captures<'_>| {
                count += 1;
                REDACTED.to_string()
            })
            .into_owned();
        total_count += count;
    }

    (result, total_count)
}

// ---------------------------------------------------------------------------
// Layer 2: High-entropy flagging
// ---------------------------------------------------------------------------

/// Calculate Shannon entropy of a byte string (log2 bits per character).
fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }

    let mut freq = [0u32; 256];
    for &b in s.as_bytes() {
        freq[b as usize] += 1;
    }

    let len = s.len() as f64;
    freq.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

const ENTROPY_THRESHOLD: f64 = 4.5;
const MIN_TOKEN_LEN: usize = 20;

/// Flag contiguous non-whitespace strings with high Shannon entropy.
///
/// Returns `(line_number, flagged_string)` pairs (1-indexed line numbers).
/// This is informational — flagged strings are NOT replaced.
pub fn flag_high_entropy(text: &str) -> Vec<(usize, String)> {
    let mut flagged = Vec::new();

    for (line_idx, line) in text.lines().enumerate() {
        for token in line.split_whitespace() {
            if token.len() > MIN_TOKEN_LEN && shannon_entropy(token) > ENTROPY_THRESHOLD {
                flagged.push((line_idx + 1, token.to_string()));
            }
        }
    }

    flagged
}

// ---------------------------------------------------------------------------
// Combined pipeline
// ---------------------------------------------------------------------------

/// Result of the full redaction pipeline.
#[derive(Debug, Clone)]
pub struct RedactionResult {
    /// Content after known-pattern redaction.
    pub content: String,
    /// Number of known patterns that were redacted.
    pub patterns_redacted: usize,
    /// High-entropy strings flagged (line_number, string) after redaction.
    pub high_entropy_flagged: Vec<(usize, String)>,
}

/// Run the full redaction pipeline: known-pattern replacement, then entropy flagging.
pub fn redact_all(text: &str) -> RedactionResult {
    let (content, patterns_redacted) = redact_known_patterns(text);
    let high_entropy_flagged = flag_high_entropy(&content);

    RedactionResult {
        content,
        patterns_redacted,
        high_entropy_flagged,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Layer 1: known-pattern tests --------------------------------------

    #[test]
    fn test_redact_openai_key() {
        let input = "key: sk-proj-abcdefghijklmnopqrstuvwxyz123456";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "key: [REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_redact_google_ai_key() {
        let input = "AIzaSyA1234567890abcdefghijklmnopqrstuvwx";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "[REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_redact_groq_key() {
        let input = "export GROQ_KEY=gsk_abcdefghijklmnopqrstuvwxyz";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "export GROQ_KEY=[REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_redact_github_token_ghp() {
        let input = "token: ghp_1234567890abcdefghijklmnopqrstuvwxyz";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "token: [REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_redact_github_token_gho() {
        let input = "gho_abcdefghijklmnopqrstuvwxyz1234567890";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "[REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_redact_github_token_ghs() {
        let input = "ghs_abcdefghijklmnopqrstuvwxyz1234567890ab";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "[REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_redact_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.xxx";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "Authorization: [REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_redact_generic_api_key_value() {
        let input = r#"api_key = "abcdefghijklmnopqrstuvwxyz1234567890""#;
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, "[REDACTED]");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_normal_text_not_redacted() {
        let input = "Hello world, this is a normal TODO plan with no secrets.";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, input);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_short_strings_not_redacted() {
        let input = "sk-short";
        let (redacted, count) = redact_known_patterns(input);
        assert_eq!(redacted, input);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_multiple_secrets_in_one_text() {
        let input = "key1=sk-abcdefghijklmnopqrstuvwxyz and key2=gsk_abcdefghijklmnopqrstuvwxyz";
        let (redacted, count) = redact_known_patterns(input);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("sk-abcdefghijklmnopqrstuvwxyz"));
        assert!(!redacted.contains("gsk_abcdefghijklmnopqrstuvwxyz"));
        assert_eq!(count, 2);
    }

    #[test]
    fn test_redaction_count_accuracy() {
        let input = concat!(
            "first: sk-aaaabbbbccccddddeeeeffffgggg\n",
            "second: sk-1111222233334444555566667777\n",
            "third: normal text\n",
        );
        let (_, count) = redact_known_patterns(input);
        assert_eq!(count, 2);
    }

    // -- Layer 2: entropy flagging tests -----------------------------------

    #[test]
    fn test_flag_high_entropy_base64() {
        // Use a string with diverse characters to guarantee entropy > 4.5
        let input = "config: aB3xZ9pQ2mR7kL5nW8cY4jT6fG1hV0dS";
        let flagged = flag_high_entropy(input);
        assert!(
            !flagged.is_empty(),
            "Should flag base64-like high-entropy string"
        );
        assert_eq!(flagged[0].0, 1);
    }

    #[test]
    fn test_flag_high_entropy_skips_low_entropy() {
        let input = "aaaaaaaaaaaaaaaaaaaaaaaaa";
        let flagged = flag_high_entropy(input);
        assert!(
            flagged.is_empty(),
            "Low-entropy repeated chars should not be flagged"
        );
    }

    #[test]
    fn test_flag_high_entropy_skips_short_tokens() {
        let input = "abc123 short";
        let flagged = flag_high_entropy(input);
        assert!(flagged.is_empty(), "Short tokens should not be flagged");
    }

    #[test]
    fn test_flag_high_entropy_line_numbers() {
        let input =
            "normal line\nstill normal\nhigh_entropy_value: x9Kp2mR7vLqW3nYs8bTfJ4gUdA6hCeZ1";
        let flagged = flag_high_entropy(input);
        if !flagged.is_empty() {
            assert_eq!(flagged[0].0, 3, "Should be on line 3");
        }
    }

    // -- Shannon entropy unit tests ----------------------------------------

    #[test]
    fn test_shannon_entropy_empty() {
        assert_eq!(shannon_entropy(""), 0.0);
    }

    #[test]
    fn test_shannon_entropy_single_char() {
        assert!(shannon_entropy("aaaa") < 0.1, "Repeated char = ~0 entropy");
    }

    #[test]
    fn test_shannon_entropy_mixed() {
        let entropy = shannon_entropy("aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrR");
        assert!(
            entropy > 4.0,
            "Mixed-case alphabet should have high entropy, got {entropy}"
        );
    }

    // -- Combined pipeline tests -------------------------------------------

    #[test]
    fn test_redact_all_combines_both_layers() {
        let input = concat!(
            "api_key: sk-proj-abcdefghijklmnopqrstuvwxyz123456\n",
            "also: x9Kp2mR7vLqW3nYs8bTfJ4gUdA6hCeZ1\n",
            "normal text here\n",
        );

        let result = redact_all(input);

        assert!(
            result.patterns_redacted >= 1,
            "Should redact the OpenAI key"
        );
        assert!(
            result.content.contains("[REDACTED]"),
            "Redacted content should contain placeholder"
        );
        assert!(
            !result.content.contains("sk-proj-"),
            "Original key should be gone"
        );
    }

    #[test]
    fn test_redact_all_clean_text() {
        let input = "This is a perfectly normal TODO plan.\nNo secrets here.";
        let result = redact_all(input);
        assert_eq!(result.patterns_redacted, 0);
        assert_eq!(result.content, input);
    }

    #[test]
    fn test_redact_all_entropy_runs_on_redacted_content() {
        // After redaction, [REDACTED] placeholder should NOT be entropy-flagged
        let input = "secret: sk-proj-abcdefghijklmnopqrstuvwxyz123456";
        let result = redact_all(input);
        assert_eq!(result.patterns_redacted, 1);

        // [REDACTED] is low-entropy, should not be flagged
        let redacted_flagged: Vec<_> = result
            .high_entropy_flagged
            .iter()
            .filter(|(_, s)| s == "[REDACTED]")
            .collect();
        assert!(
            redacted_flagged.is_empty(),
            "[REDACTED] placeholder should not be entropy-flagged"
        );
    }
}
