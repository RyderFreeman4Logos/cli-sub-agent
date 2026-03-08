//! Token estimation for reference files.
//!
//! Two-tier strategy:
//! - Small files (<32KB): chars / 3 heuristic (Chinese-friendly, fast)
//! - Large files (>=32KB): precise count via tokuin's OpenAI tokenizer

use anyhow::{Context, Result};
use std::path::Path;
use tokuin::tokenizers::{OpenAITokenizer, Tokenizer};

/// File size threshold for switching from heuristic to precise tokenization.
const PRECISE_THRESHOLD: u64 = 32_768;

/// Estimate token count for a file on disk.
///
/// - Files smaller than 32KB use a chars/3 heuristic (fast, Chinese-friendly).
/// - Files 32KB or larger use tokuin's OpenAI tokenizer for a precise count.
pub fn estimate_tokens(path: &Path) -> Result<usize> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat file: {}", path.display()))?;
    let size = metadata.len();

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    if size < PRECISE_THRESHOLD {
        Ok(estimate_tokens_heuristic(&content))
    } else {
        estimate_tokens_precise(&content)
    }
}

/// Heuristic token estimate: character count divided by 3.
///
/// Works reasonably well for mixed English/Chinese text because CJK characters
/// are each one char but typically encode to 1-2 tokens, while English words
/// average ~4 chars per token. The /3 ratio sits between both.
pub fn estimate_tokens_heuristic(content: &str) -> usize {
    // Use char count (not byte count) so CJK characters count as 1 each.
    let char_count = content.chars().count();
    char_count / 3
}

/// Precise token estimate using tokuin's OpenAI BPE tokenizer.
fn estimate_tokens_precise(content: &str) -> Result<usize> {
    let tokenizer = OpenAITokenizer::new("gpt-4")
        .map_err(|e| anyhow::anyhow!("Failed to create tokenizer: {e}"))?;
    let count = tokenizer
        .count_tokens(content)
        .map_err(|e| anyhow::anyhow!("Failed to count tokens: {e}"))?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_heuristic_english() {
        // "Hello world" = 11 chars -> 11/3 = 3 tokens (integer division)
        assert_eq!(estimate_tokens_heuristic("Hello world"), 3);
    }

    #[test]
    fn test_heuristic_short_text() {
        // "abcd" = 4 chars -> 4/3 = 1 token
        assert_eq!(estimate_tokens_heuristic("abcd"), 1);
    }

    #[test]
    fn test_heuristic_empty() {
        assert_eq!(estimate_tokens_heuristic(""), 0);
    }

    #[test]
    fn test_estimate_tokens_small_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        // "Hello world, this is a test file." = 33 chars -> 33/3 = 11
        write!(tmp, "Hello world, this is a test file.").unwrap();

        let result = estimate_tokens(tmp.path()).unwrap();
        assert_eq!(result, 11);
    }

    #[test]
    fn test_estimate_tokens_file_not_found() {
        let result = estimate_tokens(Path::new("/nonexistent/file.md"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to stat"));
    }

    #[test]
    fn test_estimate_tokens_known_content() {
        let mut tmp = NamedTempFile::new().unwrap();
        // 300 'x' chars -> 300/3 = 100 tokens via heuristic
        let content = "x".repeat(300);
        write!(tmp, "{content}").unwrap();

        let result = estimate_tokens(tmp.path()).unwrap();
        assert_eq!(result, 100);
    }

    #[test]
    fn test_precise_tokenizer_works() {
        // Verify the precise path doesn't panic/error on valid input.
        let result = estimate_tokens_precise("Hello, world!");
        assert!(result.is_ok());
        assert!(result.unwrap() > 0);
    }
}
