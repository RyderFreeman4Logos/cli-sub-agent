//! Soft fork: context summary injection for non-Claude-Code tools.
//!
//! When forking a session to a tool that cannot natively fork provider-level
//! sessions (codex, gemini-cli, opencode), we read the parent session's
//! result and structured output, produce a truncated context summary, and
//! inject it as an initial system prompt in the new session.
//!
//! ## Security boundary
//!
//! The context summary is passed through [`redact_text_content`] before
//! injection, stripping API keys, bearer tokens, passwords, and private
//! key blocks.  Fork sessions must call `build_session_meta()` fresh from
//! their own `SessionConfig` rather than copying the parent's ACP meta,
//! ensuring no MCP server credentials leak across the fork boundary.
//!
//! ## Quality gate (measurement approach)
//!
//! Soft-fork quality is defined as:
//!
//! > Forked session task success rate >= 80% of an equivalent cold-start session.
//!
//! This will be measured empirically by:
//! 1. Running the same prompt on N cold-start sessions (baseline).
//! 2. Running the same prompt on N soft-forked sessions.
//! 3. Comparing task-completion rate (exit code 0 + expected artifacts).
//!
//! Until empirical data is collected, the 80% threshold is a **design target**,
//! not a hard gate.  The token budget ([`SUMMARY_TOKEN_BUDGET`] = 2000) was
//! chosen as a balance between context fidelity and injection cost.

use std::path::Path;

use anyhow::{Context, Result};

use crate::output_parser::{estimate_tokens, load_output_index, read_section};
use crate::redact::redact_text_content;
use crate::result::{RESULT_FILE_NAME, SessionResult};

/// Maximum token budget for the context summary injected into forked sessions.
const SUMMARY_TOKEN_BUDGET: usize = 2000;

/// Context gathered from a parent session for soft-fork injection.
#[derive(Debug, Clone)]
pub struct SoftForkContext {
    /// Formatted context summary ready for injection as system/initial prompt.
    pub context_summary: String,
    /// The parent CSA session ID (ULID).
    pub parent_session_id: String,
}

/// Build a [`SoftForkContext`] by reading the parent session directory.
///
/// Reads:
/// 1. `result.toml` — status, tool, summary
/// 2. `output/index.toml` — section list
/// 3. `output/summary.md` — summary section content (if available)
///
/// The resulting `context_summary` is capped at [`SUMMARY_TOKEN_BUDGET`] tokens
/// (estimated via word-count heuristic).
pub fn soft_fork_session(
    parent_session_dir: &Path,
    parent_session_id: &str,
) -> Result<SoftForkContext> {
    let mut parts: Vec<String> = Vec::new();

    // 1. Read result.toml for status/artifacts
    let result_path = parent_session_dir.join(RESULT_FILE_NAME);
    if result_path.is_file() {
        let content = std::fs::read_to_string(&result_path)
            .with_context(|| format!("Failed to read {}", result_path.display()))?;
        if let Ok(result) = toml::from_str::<SessionResult>(&content) {
            parts.push(format!(
                "Parent session ran tool '{}', status: {}, exit code: {}.",
                result.tool, result.status, result.exit_code
            ));
            if !result.summary.is_empty() {
                parts.push(format!("Result summary: {}", result.summary));
            }
            if !result.artifacts.is_empty() {
                let artifact_list: Vec<&str> =
                    result.artifacts.iter().map(|a| a.path.as_str()).collect();
                parts.push(format!("Artifacts: {}", artifact_list.join(", ")));
            }
        }
    }

    // 2. Read output/index.toml for section list
    if let Ok(Some(index)) = load_output_index(parent_session_dir) {
        if !index.sections.is_empty() {
            let section_ids: Vec<&str> = index.sections.iter().map(|s| s.id.as_str()).collect();
            parts.push(format!(
                "Structured output sections: {} (total ~{} tokens).",
                section_ids.join(", "),
                index.total_tokens
            ));
        }
    }

    // 3. Read summary section content if available
    if let Ok(Some(summary_content)) = read_section(parent_session_dir, "summary") {
        if !summary_content.is_empty() {
            parts.push(format!("Summary from parent:\n{summary_content}"));
        }
    }

    // Assemble and truncate
    let raw_context = if parts.is_empty() {
        "No prior context available from parent session.".to_string()
    } else {
        parts.join("\n")
    };

    let truncated = truncate_to_token_budget(&raw_context, SUMMARY_TOKEN_BUDGET);

    // Redact secrets/API keys from the summary before injecting into child session.
    // This enforces the fork security boundary: child sessions must not inherit
    // parent credentials via the context summary.
    let redacted = redact_text_content(&truncated);

    let context_summary = format!(
        "You are continuing work from a previous session (ID: {parent_session_id}). \
         Key context:\n{redacted}"
    );

    Ok(SoftForkContext {
        context_summary,
        parent_session_id: parent_session_id.to_string(),
    })
}

/// Truncate text to fit within a token budget (estimated via word count * 4/3).
///
/// Removes words from the end until the estimate fits, then appends "[truncated]".
fn truncate_to_token_budget(text: &str, budget: usize) -> String {
    let estimated = estimate_tokens(text);
    if estimated <= budget {
        return text.to_string();
    }

    // Binary search-like approach: collect words and find cutoff
    let words: Vec<&str> = text.split_whitespace().collect();
    // Target word count: budget * 3/4 (inverse of estimate_tokens formula)
    let target_words = budget * 3 / 4;
    let cutoff = target_words.min(words.len());
    let truncated = words[..cutoff].join(" ");
    format!("{truncated}\n[truncated]")
}

#[cfg(test)]
#[path = "soft_fork_tests.rs"]
mod tests;
