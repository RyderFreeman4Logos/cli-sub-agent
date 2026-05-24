use csa_core::types::ReviewDecision;
use csa_session::Severity;
use regex::Regex;
use serde::Deserialize;

use super::artifacts::{has_blocking_severity, severity_counts_are_zero};
use super::clean_detection::contains_clean_phrase;

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
    let marker_re =
        Regex::new(r"(?i)\[(critical|high|medium|low|info|p[0-4])\]").expect("valid regex");
    let mut counts = zero_severity_counts();

    for captures in marker_re.captures_iter(text) {
        let severity = match captures.get(1).map(|m| m.as_str().to_ascii_lowercase()) {
            Some(level) if level == "critical" => Severity::Critical,
            Some(level) if level == "high" => Severity::High,
            Some(level) if level == "medium" => Severity::Medium,
            Some(level) if level == "low" => Severity::Low,
            Some(level) if level == "info" => Severity::Low,
            Some(level) if level == "p0" => Severity::Critical,
            Some(level) if level == "p1" => Severity::High,
            Some(level) if level == "p2" => Severity::Medium,
            Some(level) if level == "p3" || level == "p4" => Severity::Low,
            _ => continue,
        };
        *counts.entry(severity).or_insert(0) += 1;
    }

    counts
}

pub(super) fn parse_overall_risk_from_text(text: &str) -> Option<String> {
    let risk_re = Regex::new(r"(?im)\boverall risk\b\s*:?\s*(critical|high|medium|low)\b")
        .expect("valid regex");
    risk_re
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|level| level.as_str().to_ascii_lowercase())
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
    if contains_verdict_token(text, "UNAVAILABLE") {
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
