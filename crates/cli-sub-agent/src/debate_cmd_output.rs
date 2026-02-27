use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use csa_session::SessionArtifact;
use serde::Serialize;

use crate::debate_cmd::DebateMode;

const DEBATE_VERDICT_REL_PATH: &str = "output/debate-verdict.json";
const DEBATE_TRANSCRIPT_REL_PATH: &str = "output/debate-transcript.md";

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct DebateVerdict {
    verdict: String,
    confidence: String,
    summary: String,
    key_points: Vec<String>,
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
    pub(crate) confidence: String,
    pub(crate) summary: String,
    pub(crate) key_points: Vec<String>,
    /// Debate execution mode for output annotation.
    pub(crate) mode: DebateMode,
}

pub(crate) fn extract_debate_summary(
    tool_output: &str,
    fallback_summary: &str,
    mode: DebateMode,
) -> DebateSummary {
    let summary = extract_one_line_summary(tool_output, fallback_summary);
    let key_points = extract_key_points(tool_output, summary.as_str());
    DebateSummary {
        verdict: extract_verdict(tool_output).to_string(),
        confidence: extract_confidence(tool_output).to_string(),
        summary,
        key_points,
        mode,
    }
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
        confidence: summary.confidence.clone(),
        summary: summary.summary.clone(),
        key_points: summary.key_points.clone(),
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

    csa_session::save_result(project_root, session_id, &result)
        .with_context(|| format!("Failed to update result.toml for debate session {session_id}"))?;
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

pub(crate) fn extract_verdict(output: &str) -> &'static str {
    let mut matched = None;
    for line in output.lines() {
        let normalized = line.trim().to_ascii_uppercase();
        if normalized.is_empty() {
            continue;
        }

        let has_verdict_hint = normalized.contains("VERDICT")
            || normalized.contains("FINAL DECISION")
            || normalized.contains("DECISION")
            || normalized.contains("CONCLUSION");

        if has_verdict_hint {
            if normalized.contains("APPROVE") {
                matched = Some("APPROVE");
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
            "REVISE" => matched = Some("REVISE"),
            "REJECT" => matched = Some("REJECT"),
            _ => {}
        }
    }

    matched.unwrap_or("REVISE")
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
                return truncate_chars(cleaned.as_str(), 200);
            }
        }
    }

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

    let fallback = normalize_whitespace(fallback_summary);
    if fallback.is_empty() {
        "No summary provided.".to_string()
    } else {
        truncate_chars(fallback.as_str(), 200)
    }
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
        || line.starts_with("Position:")
        || line.starts_with("Key Arguments:")
        || line.starts_with("Implementation:")
        || line.starts_with("Anticipated Counterarguments:")
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
