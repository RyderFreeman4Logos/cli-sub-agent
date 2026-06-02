use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use csa_session::SessionArtifact;
use serde::{Deserialize, Serialize};

use crate::debate_cmd::DebateMode;

const DEBATE_VERDICT_REL_PATH: &str = "output/debate-verdict.json";
const DEBATE_TRANSCRIPT_REL_PATH: &str = "output/debate-transcript.md";

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct DebateVerdict {
    pub(crate) verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decision: Option<String>,
    confidence: String,
    summary: String,
    key_points: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<String>,
    timestamp: String,
    /// Debate execution mode annotation.
    ///
    /// - `"heterogeneous"`: different model families (full diversity)
    /// - `"same-model adversarial"`: same tool, independent contexts (degraded)
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DebateSummary {
    pub(crate) verdict: String,
    pub(crate) decision: Option<String>,
    pub(crate) confidence: String,
    pub(crate) summary: String,
    pub(crate) key_points: Vec<String>,
    pub(crate) failure_reason: Option<String>,
    /// Debate execution mode for output annotation.
    pub(crate) mode: DebateMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DebateOutputHeader {
    pub(crate) prompt_bytes: usize,
}

pub(crate) fn extract_debate_summary(
    tool_output: &str,
    fallback_summary: &str,
    mode: DebateMode,
) -> DebateSummary {
    // Prefer the final assistant message(s): when the tool emitted a codex-style
    // JSON event transcript, summary/verdict must come from the assistant text,
    // not from protocol JSON / hook events / tool_result noise (#161). Fall back
    // to the raw output only when no assistant text could be extracted.
    let assistant_text = extract_final_assistant_text(tool_output);
    let source = assistant_text.as_deref().unwrap_or(tool_output);
    let summary = extract_one_line_summary(source, fallback_summary);
    let key_points = extract_key_points(source, summary.as_str());
    DebateSummary {
        verdict: extract_verdict(source).to_string(),
        decision: None,
        confidence: extract_confidence(source).to_string(),
        summary,
        key_points,
        failure_reason: None,
        mode,
    }
}

/// Extract the assistant-authored text from a tool output, dropping protocol
/// JSON, hook events, and tool_result envelopes. Returns `Some(text)` only when
/// the output was a codex JSON event transcript that yielded non-empty assistant
/// content; otherwise `None` so the caller keeps the raw (already-prose) output.
fn extract_final_assistant_text(tool_output: &str) -> Option<String> {
    if !crate::codex_transcript_filter::first_non_empty_line_is_thread_started(tool_output) {
        return None;
    }
    crate::codex_transcript_filter::extract_codex_json_event_text(tool_output)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub(crate) fn persist_debate_output_artifacts(
    session_dir: &Path,
    summary: &DebateSummary,
    transcript: &str,
) -> Result<Vec<SessionArtifact>> {
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "Failed to create debate output directory: {}",
            output_dir.display()
        )
    })?;

    let mode_annotation = match summary.mode {
        DebateMode::Heterogeneous => None,
        DebateMode::SameModelAdversarial => {
            Some("same-model adversarial, not heterogeneous".to_string())
        }
    };
    let verdict = DebateVerdict {
        verdict: summary.verdict.clone(),
        decision: summary.decision.clone(),
        confidence: summary.confidence.clone(),
        summary: summary.summary.clone(),
        key_points: summary.key_points.clone(),
        failure_reason: summary.failure_reason.clone(),
        timestamp: Utc::now().to_rfc3339(),
        mode: mode_annotation,
    };
    let verdict_path = output_dir.join("debate-verdict.json");
    let verdict_json = serde_json::to_string_pretty(&verdict)
        .context("Failed to serialize debate verdict JSON")?;
    fs::write(&verdict_path, verdict_json)
        .with_context(|| format!("Failed to write debate verdict: {}", verdict_path.display()))?;

    let transcript_path = output_dir.join("debate-transcript.md");
    fs::write(&transcript_path, transcript).with_context(|| {
        format!(
            "Failed to write debate transcript: {}",
            transcript_path.display()
        )
    })?;

    Ok(vec![
        SessionArtifact::new(DEBATE_VERDICT_REL_PATH),
        SessionArtifact::new(DEBATE_TRANSCRIPT_REL_PATH),
    ])
}

pub(crate) fn append_debate_artifacts_to_result(
    project_root: &Path,
    session_id: &str,
    debate_artifacts: &[SessionArtifact],
    summary: &DebateSummary,
) -> Result<()> {
    let mut result = csa_session::load_result(project_root, session_id)?
        .ok_or_else(|| anyhow::anyhow!("Missing result.toml for debate session {session_id}"))?;

    for artifact in debate_artifacts {
        if !result
            .artifacts
            .iter()
            .any(|existing| existing.path == artifact.path)
        {
            result.artifacts.push(artifact.clone());
        }
    }

    result.summary = summary.summary.clone();

    csa_session::save_result(project_root, session_id, &result)
        .with_context(|| format!("Failed to update result.toml for debate session {session_id}"))?;
    // Best-effort cooldown marker
    csa_session::write_cooldown_marker_for_project(project_root, session_id, result.completed_at);
    Ok(())
}

pub(crate) fn format_debate_stdout_summary(summary: &DebateSummary) -> String {
    let mode_suffix = match summary.mode {
        DebateMode::Heterogeneous => String::new(),
        DebateMode::SameModelAdversarial => {
            " [DEGRADED: same-model adversarial, not heterogeneous]".to_string()
        }
    };
    format!(
        "Debate verdict: {} (confidence: {}) - {}{}",
        summary.verdict, summary.confidence, summary.summary, mode_suffix
    )
}

pub(crate) fn format_debate_stdout_text(
    summary: &DebateSummary,
    transcript: &str,
    header: Option<DebateOutputHeader>,
) -> String {
    let mut rendered = String::new();
    if let Some(header) = header {
        rendered.push_str(&format!("Debate prompt bytes: {}\n", header.prompt_bytes));
    }
    rendered.push_str(&format_debate_stdout_summary(summary));
    rendered.push('\n');

    if !transcript.is_empty() {
        rendered.push('\n');
        rendered.push_str(transcript);
        if !transcript.ends_with('\n') {
            rendered.push('\n');
        }
    }

    rendered
}

#[derive(Debug, Serialize)]
struct DebateJsonOutput<'a> {
    verdict: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<&'a str>,
    confidence: &'a str,
    summary: &'a str,
    key_points: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<&'a str>,
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_bytes: Option<usize>,
    transcript: &'a str,
    meta_session_id: &'a str,
}

pub(crate) fn render_debate_stdout_json(
    summary: &DebateSummary,
    transcript: &str,
    meta_session_id: &str,
    header: Option<DebateOutputHeader>,
) -> Result<String> {
    let payload = DebateJsonOutput {
        verdict: summary.verdict.as_str(),
        decision: summary.decision.as_deref(),
        confidence: summary.confidence.as_str(),
        summary: summary.summary.as_str(),
        key_points: &summary.key_points,
        failure_reason: summary.failure_reason.as_deref(),
        mode: match summary.mode {
            DebateMode::Heterogeneous => "heterogeneous",
            DebateMode::SameModelAdversarial => "same-model-adversarial",
        },
        prompt_bytes: header.map(|h| h.prompt_bytes),
        transcript,
        meta_session_id,
    };

    serde_json::to_string_pretty(&payload).context("Failed to serialize debate JSON output")
}

pub(crate) fn render_debate_output(
    tool_output: &str,
    meta_session_id: &str,
    provider_session_id: Option<&str>,
) -> String {
    let mut output = match provider_session_id {
        Some(provider_id) => tool_output.replace(provider_id, meta_session_id),
        None => tool_output.to_string(),
    };

    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }

    output.push_str(&format!("CSA Meta Session ID: {meta_session_id}\n"));
    output
}

pub(crate) fn extract_explicit_verdict(output: &str) -> Option<&'static str> {
    let mut matched = None;
    for line in output.lines() {
        let normalized = line.trim().to_ascii_uppercase();
        if normalized.is_empty() {
            continue;
        }

        // `CSA_VERDICT: CONFIRMED` is the structured success marker emitted by
        // debate participants; treat it as a success verdict (#161). It carries
        // through to `exit_code_from_debate_verdict`, where `CONFIRMED` maps to 0.
        if normalized.contains("CSA_VERDICT") && normalized.contains("CONFIRMED") {
            matched = Some("CONFIRMED");
            continue;
        }

        let has_verdict_hint = normalized.contains("VERDICT")
            || normalized.contains("FINAL DECISION")
            || normalized.contains("DECISION")
            || normalized.contains("CONCLUSION");

        if has_verdict_hint {
            if normalized.contains("APPROVE") {
                matched = Some("APPROVE");
            } else if normalized.contains("CONFIRMED") {
                matched = Some("CONFIRMED");
            } else if normalized.contains("REJECT") {
                matched = Some("REJECT");
            } else if normalized.contains("REVISE") {
                matched = Some("REVISE");
            }
            if matched.is_some() {
                continue;
            }
        }

        match normalized.as_str() {
            "APPROVE" => matched = Some("APPROVE"),
            "CONFIRMED" => matched = Some("CONFIRMED"),
            "REVISE" => matched = Some("REVISE"),
            "REJECT" => matched = Some("REJECT"),
            _ => {}
        }
    }

    matched
}

pub(crate) fn extract_verdict(output: &str) -> &'static str {
    extract_explicit_verdict(output).unwrap_or("REVISE")
}

pub(crate) fn extract_confidence(output: &str) -> &'static str {
    for line in output.lines() {
        let normalized = line.trim().to_ascii_lowercase();
        if !normalized.contains("confidence") {
            continue;
        }
        if normalized.contains("high") {
            return "high";
        }
        if normalized.contains("low") {
            return "low";
        }
        if normalized.contains("medium") {
            return "medium";
        }
    }

    let whole = output.to_ascii_lowercase();
    if whole.contains("high confidence") {
        "high"
    } else if whole.contains("low confidence") {
        "low"
    } else {
        "medium"
    }
}

pub(crate) fn extract_one_line_summary(output: &str, fallback_summary: &str) -> String {
    if let Some(summary) = extract_synthesis_summary(output) {
        return summary;
    }
    extract_first_prose_summary(output, fallback_summary)
}

fn extract_synthesis_summary(output: &str) -> Option<String> {
    extract_labeled_block_summary(output).or_else(|| extract_labeled_line_summary(output))
}

fn extract_labeled_line_summary(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("summary:") || lower.starts_with("conclusion:") {
            let value = trimmed
                .split_once(':')
                .map(|(_, rhs)| rhs)
                .unwrap_or(trimmed);
            let cleaned = normalize_whitespace(value);
            if !cleaned.is_empty() {
                return Some(truncate_chars(cleaned.as_str(), 200));
            }
        }
    }

    None
}

fn extract_labeled_block_summary(output: &str) -> Option<String> {
    let mut capture = false;
    let mut lines = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if !capture {
            let Some(rest) = strip_synthesis_label(trimmed) else {
                continue;
            };
            let inline = normalize_whitespace(rest);
            if !inline.is_empty() {
                return Some(truncate_chars(inline.as_str(), 200));
            }
            capture = true;
            continue;
        }

        if trimmed.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        if is_labeled_block_boundary(trimmed) {
            break;
        }
        if is_non_summary_line(trimmed) {
            continue;
        }
        lines.push(trimmed);
    }

    if lines.is_empty() {
        None
    } else {
        let cleaned = normalize_whitespace(&lines.join(" "));
        if cleaned.is_empty() {
            None
        } else {
            Some(truncate_chars(cleaned.as_str(), 200))
        }
    }
}

fn extract_fallback_summary(fallback_summary: &str) -> Option<String> {
    let fallback = normalize_whitespace(fallback_summary);
    if fallback.is_empty()
        || is_non_summary_line(fallback.as_str())
        || crate::session_summary_text::is_json_event_envelope(fallback.as_str())
    {
        None
    } else {
        Some(truncate_chars(fallback.as_str(), 200))
    }
}

fn extract_first_prose_summary(output: &str, fallback_summary: &str) -> String {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_non_summary_line(trimmed) {
            continue;
        }

        let cleaned = normalize_whitespace(trimmed);
        if !cleaned.is_empty() {
            return truncate_chars(cleaned.as_str(), 200);
        }
    }

    extract_fallback_summary(fallback_summary).unwrap_or_else(|| "No summary provided.".to_string())
}

fn strip_synthesis_label(line: &str) -> Option<&str> {
    let (label, rest) = line.split_once(':')?;
    match label.trim().to_ascii_lowercase().as_str() {
        "overall_assessment" | "overall assessment" | "final synthesis" | "synthesis" => {
            Some(rest.trim())
        }
        _ => None,
    }
}

fn is_labeled_block_boundary(line: &str) -> bool {
    if line.starts_with("<!-- CSA:SECTION:") {
        return true;
    }
    let Some((label, _)) = line.split_once(':') else {
        return false;
    };
    let normalized = label.trim();
    !normalized.is_empty()
        && normalized
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch == '_' || ch == ' ')
}

pub(crate) fn extract_key_points(output: &str, fallback_summary: &str) -> Vec<String> {
    let mut points = Vec::new();
    let mut dedupe = HashSet::new();

    for line in output.lines() {
        let trimmed = line.trim();
        let candidate = if let Some(item) = trimmed.strip_prefix("- ") {
            Some(item)
        } else if let Some(item) = trimmed.strip_prefix("* ") {
            Some(item)
        } else if let Some((prefix, rest)) = trimmed.split_once(". ") {
            if prefix.chars().all(|ch| ch.is_ascii_digit()) {
                Some(rest)
            } else {
                None
            }
        } else if let Some((prefix, rest)) = trimmed.split_once(") ") {
            if prefix.chars().all(|ch| ch.is_ascii_digit()) {
                Some(rest)
            } else {
                None
            }
        } else {
            None
        };

        let Some(candidate) = candidate else {
            continue;
        };
        let cleaned = truncate_chars(normalize_whitespace(candidate).as_str(), 240);
        if cleaned.is_empty() {
            continue;
        }
        if dedupe.insert(cleaned.to_ascii_lowercase()) {
            points.push(cleaned);
        }
        if points.len() >= 5 {
            break;
        }
    }

    if points.is_empty() {
        let fallback = normalize_whitespace(fallback_summary);
        if !fallback.is_empty() {
            points.push(truncate_chars(fallback.as_str(), 240));
        }
    }

    points
}

fn is_non_summary_line(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("```")
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("<!-- CSA:SECTION:")
        || line.starts_with("[thought-fallback]")
        || line.starts_with("CSA Meta Session ID:")
        || line.starts_with("Position:")
        || line.starts_with("Key Arguments:")
        || line.starts_with("Implementation:")
        || line.starts_with("Anticipated Counterarguments:")
        || is_protocol_or_hook_envelope(line)
}

/// Whether `line` is a machine protocol/hook event envelope rather than prose:
/// a raw JSON object carrying a `type` field (e.g. codex `thread.started` /
/// `turn.completed`, claude-code stream events) or such an envelope wrapped in a
/// CSA `[other] {...}` event label (e.g. `[other] {"type":"hook_started"}`).
/// These must never be selected as a debate summary line (#161).
fn is_protocol_or_hook_envelope(line: &str) -> bool {
    let candidate = line
        .trim()
        .strip_prefix("[other]")
        .map(str::trim)
        .unwrap_or_else(|| line.trim());
    crate::session_summary_text::is_json_event_envelope(candidate)
}

fn normalize_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }

    let keep = max_chars.saturating_sub(3);
    let mut out: String = input.chars().take(keep).collect();
    out.push_str("...");
    out
}
