use csa_core::types::ToolName;
use csa_executor::{
    contains_gemini_oauth_prompt, normalize_gemini_prompt_text, strip_ansi_escape_sequences,
};

use super::clean_detection::strip_prompt_guards;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::review_cmd) enum ToolReviewFailureKind {
    GeminiAuthPromptDetected,
}

impl ToolReviewFailureKind {
    pub(in crate::review_cmd) fn status_reason(self) -> &'static str {
        match self {
            Self::GeminiAuthPromptDetected => super::GEMINI_AUTH_PROMPT_STATUS_REASON,
        }
    }

    pub(in crate::review_cmd) fn summary_note(self) -> &'static str {
        match self {
            Self::GeminiAuthPromptDetected => {
                "gemini-cli auth failure: OAuth browser prompt detected; no review verdict produced"
            }
        }
    }
}

pub(in crate::review_cmd) fn detect_tool_review_failure(
    tool: ToolName,
    stdout: &str,
    stderr: &str,
) -> Option<ToolReviewFailureKind> {
    if tool != ToolName::GeminiCli {
        return None;
    }
    let normalized_stdout =
        normalize_gemini_prompt_text(&strip_ansi_escape_sequences(&strip_prompt_guards(stdout)));
    let normalized_stderr =
        normalize_gemini_prompt_text(&strip_ansi_escape_sequences(&strip_prompt_guards(stderr)));
    let combined = if normalized_stderr.is_empty() {
        normalized_stdout.clone()
    } else if normalized_stdout.is_empty() {
        normalized_stderr.clone()
    } else {
        format!("{normalized_stdout}\n{normalized_stderr}")
    };

    if !contains_gemini_oauth_prompt(&combined) {
        return None;
    }

    let saw_turn_completed = combined.lines().any(|line| {
        line.contains("\"type\":\"turn.completed\"")
            || line.contains("\"type\": \"turn.completed\"")
            || line.trim() == "turn.completed"
    });
    if saw_turn_completed {
        return None;
    }

    let output_tokens = crate::run_helpers::parse_token_usage(&combined)
        .and_then(|usage| usage.output_tokens)
        .unwrap_or(0);
    if output_tokens != 0 {
        return None;
    }
    Some(ToolReviewFailureKind::GeminiAuthPromptDetected)
}
