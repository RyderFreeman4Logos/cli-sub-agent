use csa_core::types::ReviewDecision;
use csa_session::Severity;
use regex::Regex;
use serde::Deserialize;

use super::artifacts::{has_blocking_severity, severity_counts_are_zero};
use super::clean_detection::contains_clean_phrase;
use crate::review_cmd::prose_findings::structured_bracketed_finding_severity;

#[derive(Debug, Deserialize)]
struct TranscriptEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    item: Option<TranscriptItem>,
    #[serde(default)]
    result: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TranscriptItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    text: Option<String>,
}

pub(in crate::review_cmd) fn extract_review_text(raw_output: &str) -> Option<String> {
    let mut transcript_messages = Vec::new();
    let mut saw_json_line = false;

    for line in raw_output.lines().filter(|line| !line.trim().is_empty()) {
        let event = match serde_json::from_str::<TranscriptEvent>(line) {
            Ok(event) => {
                saw_json_line = true;
                event
            }
            Err(_) if !saw_json_line => continue,
            Err(_) => return Some(raw_output.to_string()),
        };
        if event.event_type == "result" {
            if let Some(text) = event.result.filter(|text| looks_like_review_message(text)) {
                transcript_messages.push(text);
            }
            continue;
        }

        let Some(item) = event.item else {
            continue;
        };
        if event.event_type == "item.completed"
            && item.item_type == "agent_message"
            && let Some(text) = item.text.filter(|text| looks_like_review_message(text))
        {
            transcript_messages.push(text);
        }
    }

    if transcript_messages.is_empty() {
        return if saw_json_line {
            None
        } else {
            Some(raw_output.to_string())
        };
    }

    transcript_messages.pop()
}

pub(in crate::review_cmd) fn terminal_tool_error_reason(raw_output: &str) -> Option<String> {
    let mut stream_segment_started = false;
    let mut terminal_error_reason = None;
    let mut in_code_fence = false;

    for line in raw_output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("```") {
            in_code_fence = !in_code_fence;
            stream_segment_started = false;
            continue;
        }
        if in_code_fence {
            continue;
        }
        if !(line.starts_with('{') && line.ends_with('}')) {
            stream_segment_started = false;
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            stream_segment_started = false;
            continue;
        };
        let Some(event_type) = value.get("type").and_then(serde_json::Value::as_str) else {
            stream_segment_started = false;
            continue;
        };
        if !STREAM_EVENT_TYPES.contains(&event_type) {
            stream_segment_started = false;
            continue;
        }
        if STREAM_START_EVENT_TYPES.contains(&event_type) {
            stream_segment_started = true;
            terminal_error_reason = None;
            continue;
        }

        match event_type {
            "result" if stream_segment_started && claude_result_is_error(&value) => {
                terminal_error_reason = Some(json_error_summary(&value));
            }
            "turn.failed" if stream_segment_started => {
                terminal_error_reason = Some(json_error_summary(&value));
            }
            "result" | "turn.completed" => {
                terminal_error_reason = None;
            }
            "turn.failed" => {}
            _ => {}
        }
    }

    terminal_error_reason
}

fn claude_result_is_error(value: &serde_json::Value) -> bool {
    value
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || value
            .get("subtype")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|subtype| subtype.starts_with("error"))
}

fn json_error_summary(value: &serde_json::Value) -> String {
    for pointer in [
        "/error/message",
        "/message",
        "/result",
        "/subtype",
        "/error",
    ] {
        let Some(field) = value.pointer(pointer) else {
            continue;
        };
        if let Some(text) = field.as_str()
            && !text.trim().is_empty()
        {
            return text.trim().chars().take(240).collect();
        }
        if field.is_object() || field.is_array() {
            return field.to_string().chars().take(240).collect();
        }
    }
    "stream terminal error".to_string()
}

/// Minimal stream-event view used to detect turn completion without depending on the shape
/// of unrelated event payloads. Reading only `type` (serde ignores all other fields) keeps
/// detection robust against schema drift in `result`/`item`/`usage` bodies.
#[derive(Debug, Deserialize)]
struct StreamEventType {
    #[serde(rename = "type")]
    event_type: String,
}

/// Terminal stream events marking a reviewer turn that finished normally. claude-code
/// (`--output-format stream-json`) emits a final `{"type":"result",...}`; codex emits
/// `{"type":"turn.completed",...}`. Their presence proves the turn ran to completion,
/// independent of the verdict it reached.
const STREAM_TERMINAL_EVENT_TYPES: &[&str] = &["result", "turn.completed"];

/// Events that prove a contiguous JSON object segment is a tool transport stream rather
/// than reviewer prose quoting a JSON fixture.
const STREAM_START_EVENT_TYPES: &[&str] = &["system", "thread.started", "turn.started"];

/// Event types that positively identify a JSON streaming transcript (claude-code / codex).
/// Used to keep [`stream_started_without_terminal_event`] from misclassifying non-streaming
/// output (plain text or rate-limit event blobs) as an incomplete stream.
const STREAM_EVENT_TYPES: &[&str] = &[
    // claude-code stream-json
    "system",
    "assistant",
    "user",
    "stream_event",
    "result",
    // codex event stream
    "thread.started",
    "turn.started",
    "turn.completed",
    "turn.failed",
    "item.started",
    "item.completed",
];

/// Whether a reviewer's streamed transcript began but never reached a terminal completion
/// event — the signature of a process killed/timed-out mid-turn (OOM, SIGKILL, no-progress
/// watchdog). Such a transcript's verdict tokens are unreliable: they are frequently scraped
/// from the *reviewed code* (which itself contains `CLEAN`/`HAS_ISSUES`/`UNAVAILABLE` literals)
/// rather than emitted as a deliberate verdict, so fail-closing them to `HAS_ISSUES` mislabels
/// an infrastructure failure as a blocking review (#1657). Callers combine this with a non-zero
/// exit code before reclassifying the reviewer as unavailable.
///
/// Conservative by construction: returns `false` unless the output is *recognized* as a
/// claude-code/codex JSON stream (so plain-text tool output is never flagged),
/// and `false` as soon as any terminal event is seen (so a completed reviewer that merely
/// exits non-zero is never flagged).
pub(in crate::review_cmd) fn stream_started_without_terminal_event(raw_output: &str) -> bool {
    let mut saw_stream_event = false;
    for line in raw_output.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(event) = serde_json::from_str::<StreamEventType>(line) else {
            continue;
        };
        if STREAM_TERMINAL_EVENT_TYPES.contains(&event.event_type.as_str()) {
            return false;
        }
        if STREAM_EVENT_TYPES.contains(&event.event_type.as_str()) {
            saw_stream_event = true;
        }
    }
    saw_stream_event
}

fn looks_like_review_message(text: &str) -> bool {
    super::has_structured_review_content(text)
        || contains_verdict_token(text, "PASS")
        || contains_verdict_token(text, "CLEAN")
        || contains_verdict_token(text, "FAIL")
        || contains_verdict_token(text, "HAS_ISSUES")
        || contains_verdict_token(text, "UNAVAILABLE")
        || contains_verdict_token(text, "UNCERTAIN")
        || contains_clean_phrase(text)
        || text.lines().any(|line| {
            is_findings_header(line) || line.to_ascii_lowercase().contains("overall risk")
        })
}

pub(super) fn zero_severity_counts() -> std::collections::BTreeMap<Severity, u32> {
    [
        (Severity::Critical, 0),
        (Severity::High, 0),
        (Severity::Medium, 0),
        (Severity::Low, 0),
    ]
    .into_iter()
    .collect()
}

pub(super) fn severity_counts_from_text(text: &str) -> std::collections::BTreeMap<Severity, u32> {
    let mut counts = zero_severity_counts();
    let mut in_findings_section = false;
    let mut in_code_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        if is_findings_header(line) {
            in_findings_section = true;
            continue;
        }
        if in_findings_section && trimmed.starts_with('#') {
            in_findings_section = false;
            continue;
        }

        if in_findings_section && let Some(severity) = inline_findings_line_severity(line) {
            *counts.entry(severity).or_insert(0) += 1;
            continue;
        }

        if let Some(severity) = structured_bracketed_finding_severity(line) {
            *counts.entry(severity).or_insert(0) += 1;
        }
    }

    counts
}

fn inline_findings_line_severity(line: &str) -> Option<Severity> {
    let trimmed = line.trim_start();
    let (index, rest) = trimmed.split_once('.')?;
    if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let (severity, _) = rest.trim_start().split_once(':')?;
    severity_from_label(severity.trim())
}

fn severity_from_label(level: &str) -> Option<Severity> {
    crate::review_cmd::prose_findings::severity_from_label(level)
}

pub(super) fn parse_overall_risk_from_text(text: &str) -> Option<String> {
    let risk_re = Regex::new(r"(?im)\boverall risk\b\s*:?\s*(critical|high|medium|low)\b")
        .expect("valid regex");
    risk_re
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|level| level.as_str().to_ascii_lowercase())
}

pub(super) fn contains_blocking_issue_signal(text: &str) -> bool {
    text.lines().any(|line| {
        !contains_clean_phrase(line)
            && crate::review_cmd::prose_findings::contains_blocking_review_signal(line)
    })
}

pub(super) fn derive_decision_from_text(
    text: &str,
    counts: &std::collections::BTreeMap<Severity, u32>,
    overall_risk: Option<&str>,
) -> ReviewDecision {
    if has_blocking_severity(counts) {
        return ReviewDecision::Fail;
    }
    if !severity_counts_are_zero(counts)
        && (contains_verdict_token(text, "FAIL") || contains_verdict_token(text, "HAS_ISSUES"))
    {
        return ReviewDecision::Fail;
    }
    if crate::review_consensus::contains_explicit_unavailable_verdict(text) {
        return ReviewDecision::Unavailable;
    }
    if contains_verdict_token(text, "SKIP") {
        return ReviewDecision::Skip;
    }
    if contains_verdict_token(text, "UNCERTAIN") {
        return ReviewDecision::Uncertain;
    }
    if (contains_verdict_token(text, "PASS")
        || contains_verdict_token(text, "CLEAN")
        || contains_clean_phrase(text))
        && overall_risk.is_none_or(|risk| risk.eq_ignore_ascii_case("low"))
    {
        return ReviewDecision::Pass;
    }
    if text.lines().any(is_findings_header)
        && contains_clean_phrase(text)
        && overall_risk.is_none_or(|risk| risk.eq_ignore_ascii_case("low"))
    {
        return ReviewDecision::Pass;
    }
    if overall_risk.is_some_and(|risk| !risk.eq_ignore_ascii_case("low")) {
        return ReviewDecision::Fail;
    }
    ReviewDecision::Uncertain
}

fn contains_verdict_token(haystack: &str, token: &str) -> bool {
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|part| part.eq_ignore_ascii_case(token))
}

fn is_findings_header(line: &str) -> bool {
    let trimmed = line.trim();
    let normalized = trimmed.trim_start_matches('#').trim();
    normalized.eq_ignore_ascii_case("findings")
        || normalized.eq_ignore_ascii_case("review findings")
}
