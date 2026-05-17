//! JSONL conversation prefix extraction for CSA-lite fork (issue #1432).
//!
//! Reads a Claude Code JSONL session file and extracts a token-budgeted
//! conversation prefix suitable for context injection into a forked
//! session. The companion [`crate::detect_caller_session`]-style flow
//! (in `csa-session::caller_detect`) provides the JSONL path; this
//! module focuses purely on the read + filter + budget step.
//!
//! Filtering rules when [`PrefixConfig::skip_tool_results`] is `true`:
//! - Top-level `type` must be `"user"` or `"assistant"`; anything else
//!   (progress events, system notes, API errors) is skipped.
//! - Within `message.content` arrays, blocks with `type == "tool_use"`
//!   or `type == "tool_result"` are dropped before the message is
//!   serialized to plain text.
//! - String-form `content` whose role is `"tool"` is dropped (defensive).
//!
//! Malformed JSON lines are skipped with a `tracing::debug!` log rather
//! than failing the whole extraction.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;
use tracing::debug;

/// Default token budget when the caller does not override it.
pub const DEFAULT_PREFIX_BUDGET_TOKENS: usize = 32_768;

/// Configuration for prefix extraction.
#[derive(Debug, Clone)]
pub struct PrefixConfig {
    /// Maximum number of tokens to extract before truncating.
    pub budget_tokens: usize,
    /// If `true`, skip tool-result and tool-use content blocks.
    pub skip_tool_results: bool,
}

impl Default for PrefixConfig {
    fn default() -> Self {
        Self {
            budget_tokens: DEFAULT_PREFIX_BUDGET_TOKENS,
            skip_tool_results: true,
        }
    }
}

/// Result of a [`PrefixExtractor::extract_prefix`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedPrefix {
    /// Conversation text formatted for context injection.
    pub content: String,
    /// Estimated tokens consumed by [`Self::content`].
    pub token_count: usize,
    /// Number of messages included in the prefix.
    pub message_count: usize,
    /// `true` if the budget was reached before the file was fully consumed.
    pub truncated: bool,
}

/// Reads a Claude Code JSONL file and produces a budget-limited prefix.
pub struct PrefixExtractor {
    config: PrefixConfig,
}

impl PrefixExtractor {
    pub fn new(config: PrefixConfig) -> Self {
        Self { config }
    }

    /// Extract the conversation prefix from `jsonl_path`.
    pub fn extract_prefix(&self, jsonl_path: &Path) -> Result<ExtractedPrefix> {
        let file = File::open(jsonl_path)
            .with_context(|| format!("failed to open JSONL file: {}", jsonl_path.display()))?;
        let reader = BufReader::new(file);
        debug!(path = %jsonl_path.display(), budget = self.config.budget_tokens, "extracting prefix");
        self.extract_from_reader(reader)
    }

    fn extract_from_reader<R: BufRead>(&self, reader: R) -> Result<ExtractedPrefix> {
        let mut parts: Vec<String> = Vec::new();
        let mut token_count: usize = 0;
        let mut message_count: usize = 0;
        let mut truncated = false;

        for (line_no, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(err) => {
                    debug!(line_no, error = %err, "skipping unreadable JSONL line");
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }

            let value: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(err) => {
                    debug!(line_no, error = %err, "skipping malformed JSONL line");
                    continue;
                }
            };

            let Some(message) = extract_message(&self.config, &value) else {
                continue;
            };

            let next_tokens = estimate_tokens(&message.text);
            if token_count.saturating_add(next_tokens) > self.config.budget_tokens {
                truncated = true;
                debug!(
                    line_no,
                    token_count,
                    next_tokens,
                    budget = self.config.budget_tokens,
                    "budget exceeded; truncating prefix"
                );
                break;
            }

            parts.push(format!("[{}]\n{}", message.role, message.text.trim()));
            token_count += next_tokens;
            message_count += 1;
        }

        let content = parts.join("\n\n");
        Ok(ExtractedPrefix {
            content,
            token_count,
            message_count,
            truncated,
        })
    }
}

struct Message {
    role: String,
    text: String,
}

fn extract_message(config: &PrefixConfig, value: &Value) -> Option<Message> {
    let outer_type = value.get("type").and_then(Value::as_str)?;
    if outer_type != "user" && outer_type != "assistant" {
        return None;
    }

    let message = value.get("message")?;
    let role = message.get("role").and_then(Value::as_str)?.to_string();
    let content = message.get("content")?;

    let text = match content {
        Value::String(s) => {
            if config.skip_tool_results && role == "tool" {
                return None;
            }
            s.clone()
        }
        Value::Array(blocks) => {
            let mut buf = String::new();
            for block in blocks {
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                if config.skip_tool_results
                    && (block_type == "tool_result" || block_type == "tool_use")
                {
                    continue;
                }
                let snippet = block
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| block.get("content").and_then(Value::as_str));
                if let Some(s) = snippet {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(s);
                }
            }
            if buf.is_empty() {
                return None;
            }
            buf
        }
        _ => return None,
    };

    Some(Message { role, text })
}

/// Estimate token count using a simple word-based heuristic
/// (~4 chars per token, approximated as `words * 4 / 3`).
///
/// Mirrors `csa-session::output_parser::estimate_tokens` rather than
/// pulling csa-session into the L3 transport crate.
fn estimate_tokens(content: &str) -> usize {
    content.split_whitespace().count() * 4 / 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn extract(config: PrefixConfig, jsonl: &str) -> ExtractedPrefix {
        let extractor = PrefixExtractor::new(config);
        extractor
            .extract_from_reader(Cursor::new(jsonl.as_bytes()))
            .expect("extraction should not fail on in-memory reader")
    }

    #[test]
    fn empty_file_returns_empty_prefix() {
        let result = extract(PrefixConfig::default(), "");
        assert_eq!(result.content, "");
        assert_eq!(result.token_count, 0);
        assert_eq!(result.message_count, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn blank_lines_are_ignored() {
        let result = extract(PrefixConfig::default(), "\n\n   \n\n");
        assert_eq!(result.message_count, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn under_budget_includes_all_messages_and_keeps_order() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":"hello world"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]}}"#,
        );

        let result = extract(PrefixConfig::default(), jsonl);
        assert_eq!(result.message_count, 2);
        assert!(!result.truncated);
        let user_idx = result.content.find("hello world").expect("user text");
        let asst_idx = result.content.find("hi there").expect("assistant text");
        assert!(user_idx < asst_idx, "messages should appear in input order");
        assert!(result.content.contains("[user]"));
        assert!(result.content.contains("[assistant]"));
    }

    #[test]
    fn over_budget_truncates_after_first_fitting_message() {
        // estimate_tokens("alpha beta gamma") = 3 * 4 / 3 = 4
        // budget = 4 ⇒ first fits exactly, second exceeds
        let config = PrefixConfig {
            budget_tokens: 4,
            skip_tool_results: true,
        };
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":"alpha beta gamma"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"second message text"}}"#,
        );

        let result = extract(config, jsonl);
        assert!(result.truncated);
        assert_eq!(result.message_count, 1);
        assert!(result.content.contains("alpha beta gamma"));
        assert!(!result.content.contains("second message text"));
        assert_eq!(result.token_count, 4);
    }

    #[test]
    fn skip_tool_results_filters_tool_blocks() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":["#,
            r#"{"type":"tool_result","content":"file contents secret"},"#,
            r#"{"type":"text","text":"plain text"}"#,
            r#"]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":["#,
            r#"{"type":"tool_use","name":"Bash","input":{"command":"rm -rf /"}},"#,
            r#"{"type":"text","text":"running command"}"#,
            r#"]}}"#,
        );

        let result = extract(PrefixConfig::default(), jsonl);
        assert!(result.content.contains("plain text"));
        assert!(result.content.contains("running command"));
        assert!(!result.content.contains("file contents secret"));
        assert!(!result.content.contains("rm -rf"));
        assert_eq!(result.message_count, 2);
    }

    #[test]
    fn keep_tool_results_when_disabled() {
        let config = PrefixConfig {
            budget_tokens: DEFAULT_PREFIX_BUDGET_TOKENS,
            skip_tool_results: false,
        };
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":["#,
            r#"{"type":"tool_result","content":"file contents"},"#,
            r#"{"type":"text","text":"plain"}"#,
            r#"]}}"#,
        );

        let result = extract(config, jsonl);
        assert!(result.content.contains("file contents"));
        assert!(result.content.contains("plain"));
    }

    #[test]
    fn malformed_json_lines_are_skipped() {
        let jsonl = concat!(
            "this is not json\n",
            r#"{"type":"user","message":{"role":"user","content":"good message"}}"#,
            "\n",
            r#"{"oops":"missing brace"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"another message"}}"#,
        );

        let result = extract(PrefixConfig::default(), jsonl);
        assert_eq!(result.message_count, 2);
        assert!(result.content.contains("good message"));
        assert!(result.content.contains("another message"));
        assert!(!result.truncated);
    }

    #[test]
    fn non_user_assistant_entries_are_skipped() {
        let jsonl = concat!(
            r#"{"type":"progress","data":{"x":1}}"#,
            "\n",
            r#"{"type":"system","subtype":"api_error"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"only this"}}"#,
        );

        let result = extract(PrefixConfig::default(), jsonl);
        assert_eq!(result.message_count, 1);
        assert!(result.content.contains("only this"));
    }

    #[test]
    fn role_tool_string_content_is_skipped_when_filtering() {
        let jsonl =
            r#"{"type":"user","message":{"role":"tool","content":"raw tool dump"}}"#.to_string();
        let result = extract(PrefixConfig::default(), &jsonl);
        assert_eq!(result.message_count, 0);
    }

    #[test]
    fn extract_prefix_reads_from_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"disk hello"}}"#,
                "\n",
            ),
        )
        .expect("write fixture");

        let extractor = PrefixExtractor::new(PrefixConfig::default());
        let result = extractor
            .extract_prefix(&path)
            .expect("extract should succeed");
        assert_eq!(result.message_count, 1);
        assert!(result.content.contains("disk hello"));
    }

    #[test]
    fn extract_prefix_missing_file_errors() {
        let extractor = PrefixExtractor::new(PrefixConfig::default());
        let err = extractor
            .extract_prefix(Path::new("/nonexistent/csa-test-prefix.jsonl"))
            .expect_err("missing file should error");
        let msg = format!("{err:#}");
        assert!(msg.contains("failed to open JSONL file"), "got: {msg}");
    }
}
