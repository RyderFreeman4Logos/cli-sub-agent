//! Human-readable session summary extraction for `csa session result` / `wait`
//! and the MCP hub tool response (#1682).
//!
//! `SessionResult.summary` holds the tool's raw terminal event for some tools
//! (notably codex, where it is a `turn.completed` JSON envelope kept for token
//! extraction). That envelope is NOT prose and must never be shown as the human
//! "Summary:" line. The agent's actual conclusion lives in `output/summary.md`;
//! prefer it, and otherwise suppress machine-event envelopes.

use std::path::Path;

/// Resolve the human-readable summary for display.
///
/// Preference order:
/// 1. `output/summary.md` (agent-/review-authored prose), when present and non-empty.
/// 2. The raw `summary`, unless it is a JSON event envelope (see
///    [`is_json_event_envelope`]), in which case `None` is returned so the caller
///    omits the Summary line entirely rather than printing machine JSON.
pub(crate) fn human_session_summary(session_dir: &Path, raw_summary: &str) -> Option<String> {
    if let Some(markdown) = read_summary_markdown(session_dir) {
        return Some(markdown);
    }
    if is_json_event_envelope(raw_summary) {
        return None;
    }
    non_empty_trimmed(raw_summary)
}

fn read_summary_markdown(session_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(session_dir.join("output").join("summary.md")).ok()?;
    non_empty_trimmed(&raw)
}

/// Whether `summary` is a machine event envelope rather than human prose: it
/// parses as JSON and carries a string `type` field (e.g. codex
/// `{"type":"turn.completed",...}`). Such envelopes are intentionally kept on
/// `SessionResult.summary` for token extraction but are never human summaries.
/// Kept `pub(crate)` as a reusable predicate for callers that have no session
/// directory to consult `output/summary.md`.
pub(crate) fn is_json_event_envelope(summary: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(summary.trim())
        .ok()
        .is_some_and(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .is_some()
        })
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
#[path = "session_summary_text_tests.rs"]
mod tests;
