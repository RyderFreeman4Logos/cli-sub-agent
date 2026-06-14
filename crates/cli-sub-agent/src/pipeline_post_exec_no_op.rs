//! No-op exit classification helpers.

use std::path::Path;

use csa_session::TokenUsage;

/// Sessions shorter than this threshold (in seconds) that exit 0 with zero
/// tool calls in sa-mode are classified as no-op exits.
pub(super) const ELAPSED_THRESHOLD_SECS: i64 = 60;
const MEANINGFUL_OUTPUT_TOKENS: u64 = 1000;
const SUMMARY_MAX_CHARS: usize = 500;
const TASK_LABEL_MAX_CHARS: usize = 72;
const REQUEST_EXCERPT_MAX_CHARS: usize = 150;
const ORIGINAL_SUMMARY_MAX_CHARS: usize = 80;

pub(super) fn has_meaningful_reasoning_output(
    token_usage: &Option<TokenUsage>,
    transport_output_tokens: Option<u64>,
) -> bool {
    [
        token_usage.as_ref().and_then(|usage| usage.output_tokens),
        transport_output_tokens,
    ]
    .into_iter()
    .flatten()
    .max()
    .is_some_and(|output_tokens| output_tokens > MEANINGFUL_OUTPUT_TOKENS)
}

pub(super) fn build_no_op_failure_summary(
    turn_count: u32,
    elapsed_secs: i64,
    tool_name: &str,
    session_description: Option<&str>,
    prompt: &str,
    original_summary: &str,
) -> String {
    let skill_name = infer_skill_name(session_description, prompt)
        .and_then(|skill| sanitize_compact_clip(&skill, TASK_LABEL_MAX_CHARS));
    let task_label = session_description
        .filter(|description| !description.trim_start().starts_with("skill:"))
        .and_then(|description| sanitize_compact_clip(description, TASK_LABEL_MAX_CHARS));
    let request_excerpt = extract_request_excerpt(prompt)
        .and_then(|excerpt| sanitize_compact_clip(&excerpt, REQUEST_EXCERPT_MAX_CHARS));
    let original_summary = sanitize_compact_clip(original_summary, ORIGINAL_SUMMARY_MAX_CHARS);

    let mut summary = format!(
        "no-op exit detected: turn_count={turn_count}, elapsed={elapsed_secs}s, no tool calls, tool={tool_name}. "
    );

    match skill_name.as_deref() {
        Some("commit") => summary.push_str(
            "commit skill requested task did not run; no local commit was created by this attempt. ",
        ),
        Some(skill) => summary.push_str(&format!("skill={skill} requested task did not run. ")),
        None => summary.push_str("requested task did not run. "),
    }

    if let Some(label) = task_label {
        summary.push_str(&format!("task=\"{label}\". "));
    }
    if let Some(excerpt) = request_excerpt {
        summary.push_str(&format!("request=\"{excerpt}\". "));
    }

    match skill_name.as_deref() {
        Some("commit") => summary.push_str(
            "recovery=rerun csa run --skill commit after checking auth/tier or choose another tool/model.",
        ),
        Some(skill) => summary.push_str(&format!(
            "recovery=rerun csa run --skill {skill} after checking auth/tier/tool logs."
        )),
        None => summary.push_str("recovery=rerun after checking auth/tier/tool logs."),
    }

    if let Some(original) = original_summary {
        summary.push_str(&format!(" original=\"{original}\"."));
    }

    clip_chars(&summary, SUMMARY_MAX_CHARS)
}

fn infer_skill_name(session_description: Option<&str>, prompt: &str) -> Option<String> {
    if let Some(description) = session_description
        .map(str::trim)
        .and_then(|description| description.strip_prefix("skill:"))
        .map(str::trim)
        .filter(|skill| !skill.is_empty())
    {
        return Some(description.to_string());
    }

    let marker = "<skill-source path=\"";
    let start = prompt.find(marker)? + marker.len();
    let path = prompt.get(start..)?.split('"').next()?.trim();
    let skill = Path::new(path).file_name()?.to_string_lossy();
    let skill = skill.trim();
    (!skill.is_empty()).then(|| skill.to_string())
}

fn extract_request_excerpt(prompt: &str) -> Option<String> {
    let candidate = prompt
        .rsplit_once("\n\n---\n\n")
        .map(|(_, request)| request)
        .unwrap_or(prompt);
    let compact = compact_whitespace(candidate);
    (!compact.is_empty()).then_some(compact)
}

fn sanitize_compact_clip(text: &str, max_chars: usize) -> Option<String> {
    let redacted = csa_session::redact_text_content(text);
    let compact = compact_whitespace(&redacted);
    if compact.is_empty() {
        None
    } else {
        Some(clip_chars(&compact, max_chars))
    }
}

fn compact_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let clipped: String = text.chars().take(max_chars - 3).collect();
    format!("{}...", clipped.trim_end())
}
