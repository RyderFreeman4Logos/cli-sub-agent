//! Human-readable session summary extraction for `csa session result` / `wait`
//! and the MCP hub tool response (#1682).
//!
//! `SessionResult.summary` holds the tool's raw terminal event for some tools
//! (notably codex, where it is a `turn.completed` JSON envelope kept for token
//! extraction). That envelope is NOT prose and must never be shown as the human
//! "Summary:" line. The agent's actual conclusion lives in `output/summary.md`;
//! prefer it, and otherwise suppress machine-event envelopes.

use std::path::Path;

use csa_session::Severity;

/// Headline title cap so an over-long finding description never blows out the
/// single-line `Summary:` field.
const REVIEW_HEADLINE_MAX_CHARS: usize = 160;

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

/// Enrich a bare failing review verdict with its highest-severity finding's
/// grade and title (#1852).
///
/// `csa session result` renders a review verdict as a single `Summary:` line.
/// For a blocking verdict the reviewer's persisted summary is frequently just
/// the bare token (`FAIL`), which tells the reader *that* the review failed but
/// not *why*. When `base` is such a bare failing verdict, surface the top
/// finding as `"FAIL — [HIGH] <title>"`. Any other summary (real agent prose, a
/// passing verdict, or a failing verdict with no legible finding) is returned
/// unchanged — passing verdicts are intentionally never enriched so a
/// converged-clean session cannot resurrect a stale earlier-round finding.
pub(crate) fn enrich_review_summary(session_dir: &Path, base: &str) -> String {
    if !is_bare_fail_verdict(base) {
        return base.to_string();
    }
    match top_review_finding_headline(session_dir) {
        Some(headline) => format!("{base} — {headline}"),
        None => base.to_string(),
    }
}

/// Whether `summary` is exactly a bare blocking review verdict token. Only the
/// whole-string match counts: a verdict followed by prose already explains
/// itself and must not be mangled.
fn is_bare_fail_verdict(summary: &str) -> bool {
    matches!(
        summary.trim().to_ascii_uppercase().as_str(),
        "FAIL" | "HAS_ISSUES"
    )
}

/// `[SEVERITY] title` for the highest-severity finding legible in the persisted
/// review sections, or `None` when none is present. Backtick-tolerant so a
/// markdown-wrapped `` `[HIGH]` `` tag is still recognized (#1852). Ties on
/// severity keep the first finding in section order.
fn top_review_finding_headline(session_dir: &Path) -> Option<String> {
    let sections = csa_session::read_all_sections(session_dir).ok()?;
    let mut best: Option<(Severity, String)> = None;
    for (_, content) in sections {
        let mut in_code_fence = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                in_code_fence = !in_code_fence;
                continue;
            }
            if in_code_fence {
                continue;
            }
            let Some((severity, title)) = graded_finding_headline(trimmed) else {
                continue;
            };
            let replace = match &best {
                Some((best_severity, _)) => &severity > best_severity,
                None => true,
            };
            if replace {
                best = Some((severity, title));
            }
        }
    }
    best.map(|(severity, title)| format!("[{}] {}", severity_label(&severity), title))
}

/// Parse a single finding line that leads with a bracketed severity tag,
/// returning its [`Severity`] and cleaned title. Tolerates a list marker
/// (`1. `, `- `), markdown inline-code backticks, and a leading category tag
/// (`[HIGH][correctness] ...`). Returns `None` for any line that does not lead
/// with a `[severity]` tag.
fn graded_finding_headline(line: &str) -> Option<(Severity, String)> {
    let without_marker = strip_list_marker(line.trim());
    let normalized = without_marker.replace('`', "");
    let body = normalized.trim_start();
    if !body.starts_with('[') {
        return None;
    }

    let mut severity: Option<Severity> = None;
    let mut cursor = body;
    while let Some(after_open) = cursor.strip_prefix('[') {
        let Some(close) = after_open.find(']') else {
            break;
        };
        let label = after_open.get(..close)?;
        if severity.is_none() {
            severity = severity_from_label(label);
        }
        cursor = after_open.get(close + 1..)?.trim_start();
    }
    let severity = severity?;

    let title = cursor
        .trim_start_matches([':', '-', '—'])
        .trim()
        .trim_end_matches('.')
        .trim();
    if title.is_empty() {
        return None;
    }
    Some((severity, clip(title, REVIEW_HEADLINE_MAX_CHARS)))
}

/// Strip a leading ordered/unordered list marker (`12. `, `3) `, `- `, `* `,
/// `+ `) so the finding body can be parsed uniformly.
fn strip_list_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    if let Some(rest) = strip_numeric_list_marker(trimmed) {
        return rest;
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    trimmed
}

fn strip_numeric_list_marker(line: &str) -> Option<&str> {
    let non_digit = line.find(|c: char| !c.is_ascii_digit())?;
    if non_digit == 0 {
        return None;
    }
    let after_digits = line.get(non_digit..)?;
    let rest = after_digits
        .strip_prefix(". ")
        .or_else(|| after_digits.strip_prefix(") "))?;
    Some(rest.trim_start())
}

fn severity_from_label(label: &str) -> Option<Severity> {
    match label.trim().to_ascii_lowercase().as_str() {
        "critical" | "p0" => Some(Severity::Critical),
        "high" | "p1" => Some(Severity::High),
        "medium" | "p2" => Some(Severity::Medium),
        "low" | "info" | "p3" | "p4" => Some(Severity::Low),
        _ => None,
    }
}

fn severity_label(severity: &Severity) -> &'static str {
    match severity {
        Severity::Critical => "CRITICAL",
        Severity::High => "HIGH",
        Severity::Medium => "MEDIUM",
        Severity::Low => "LOW",
    }
}

/// Truncate `text` to at most `max` characters, appending an ellipsis when cut,
/// counting by `char` so a multibyte boundary is never split.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", truncated.trim_end())
}

#[cfg(test)]
#[path = "session_summary_text_tests.rs"]
mod tests;
