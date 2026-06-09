use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewVerdictArtifact, SessionArtifact, SessionResult};
use tracing::debug;

const SUMMARY_MAX_CHARS: usize = 200;
const REVIEW_SUMMARY_FAIL_TOKENS: &[&str] = &["FAIL", "HAS_ISSUES", "REJECT"];

pub(crate) fn refresh_and_repair_result(
    project_root: &Path,
    session_id: &str,
) -> Result<Option<SessionResult>> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)?;
    refresh_structured_output(&session_dir);

    let Some(mut result) = csa_session::load_result(project_root, session_id)? else {
        return Ok(None);
    };

    if enrich_result_from_session_dir(project_root, session_id, &session_dir, &mut result)? {
        csa_session::save_result(project_root, session_id, &result)?;
    }

    Ok(Some(result))
}

/// Like [`refresh_and_repair_result`] but operates directly on a known
/// `session_dir` without going through project-root-based path resolution.
///
/// Used for cross-project sessions where the session directory was resolved
/// via global ULID fallback and the current project_root would reject it.
pub(crate) fn refresh_and_repair_result_from_dir(
    session_dir: &Path,
) -> Result<Option<SessionResult>> {
    refresh_structured_output(session_dir);

    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    if !result_path.is_file() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&result_path)?;
    let mut result: SessionResult = toml::from_str(&contents)?;

    let mut changed = false;
    if let Some(summary) = derive_better_summary(session_dir, &result.summary)?
        && summary != result.summary
    {
        result.summary = summary;
        changed = true;
    }
    if let Some(events_count) = count_transcript_events(session_dir)?
        && events_count != result.events_count
    {
        result.events_count = events_count;
        changed = true;
    }
    if sync_review_verdict_exit_code(session_dir, &mut result)? {
        changed = true;
    }

    if changed {
        // Write repaired result back (best-effort for cross-project sessions).
        let _ = fs::write(&result_path, toml::to_string_pretty(&result)?);
    }

    Ok(Some(result))
}

pub(crate) fn enrich_result_from_session_dir(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    result: &mut SessionResult,
) -> Result<bool> {
    let mut changed = false;

    if let Some(summary) = derive_better_summary(session_dir, &result.summary)?
        && summary != result.summary
    {
        result.summary = summary;
        changed = true;
    }

    if let Some(events_count) = count_transcript_events(session_dir)?
        && events_count != result.events_count
    {
        result.events_count = events_count;
        changed = true;
    }

    if sync_review_verdict_exit_code(session_dir, result)? {
        changed = true;
    }

    let artifact_names = csa_session::list_artifacts(project_root, session_id)?;
    if merge_artifacts(&mut result.artifacts, artifact_names) {
        changed = true;
    }

    Ok(changed)
}

fn sync_review_verdict_exit_code(session_dir: &Path, result: &mut SessionResult) -> Result<bool> {
    let Some(exit_code) = read_review_verdict_exit_code(session_dir)? else {
        return Ok(sync_review_summary_exit_code(session_dir, result));
    };
    Ok(sync_result_exit_code(result, exit_code))
}

fn sync_review_summary_exit_code(session_dir: &Path, result: &mut SessionResult) -> bool {
    let summary_has_fail_verdict = review_summary_has_fail_verdict(&result.summary);
    let review_summary_has_blocking_outcome = session_dir.join("review_meta.json").is_file()
        && review_summary_has_blocking_outcome(&result.summary);
    if !(summary_has_fail_verdict || review_summary_has_blocking_outcome) {
        return false;
    }

    sync_result_exit_code(result, 1)
}

fn review_summary_has_fail_verdict(summary: &str) -> bool {
    summary.lines().map(str::trim).any(|line| {
        REVIEW_SUMMARY_FAIL_TOKENS
            .iter()
            .any(|token| summary_line_has_verdict_prefix(line, token))
    })
}

fn review_summary_has_blocking_outcome(summary: &str) -> bool {
    summary.lines().any(summary_line_has_blocking_outcome)
}

fn summary_line_has_blocking_outcome(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();
    summary_line_has_unnegated_high_severity(&normalized)
        || summary_line_has_unnegated_critical_severity(&normalized)
        || summary_line_has_unnegated_blocking_outcome(&normalized)
        || summary_line_has_unnegated_p1_outcome(&normalized)
}

fn summary_line_has_unnegated_high_severity(normalized: &str) -> bool {
    (normalized.contains("high-severity") || normalized.contains("high severity"))
        && !summary_line_negates_high_severity(normalized)
}

fn summary_line_negates_high_severity(normalized: &str) -> bool {
    normalized.contains("no high")
        || summary_line_has_zero_count(normalized, "0 high")
        || summary_line_has_zero_count_metric(normalized, &["high severity", "high-severity"])
}

fn summary_line_has_unnegated_critical_severity(normalized: &str) -> bool {
    (normalized.contains("critical-severity") || normalized.contains("critical severity"))
        && !summary_line_negates_critical_severity(normalized)
}

fn summary_line_negates_critical_severity(normalized: &str) -> bool {
    normalized.contains("no critical")
        || summary_line_has_zero_count(normalized, "0 critical")
        || summary_line_has_zero_count_metric(
            normalized,
            &["critical severity", "critical-severity"],
        )
}

fn summary_line_has_unnegated_blocking_outcome(normalized: &str) -> bool {
    (normalized.contains("blocking finding") || normalized.contains("blocking issue"))
        && !summary_line_negates_blocking_outcome(normalized)
}

fn summary_line_negates_blocking_outcome(normalized: &str) -> bool {
    normalized.contains("non-blocking")
        || normalized.contains("no blocking")
        || summary_line_has_zero_count(normalized, "0 blocking")
        || summary_line_has_zero_count_metric(normalized, &["blocking"])
        || normalized.contains("no correctness, regression, security, or blocking")
}

fn summary_line_has_unnegated_p1_outcome(normalized: &str) -> bool {
    (summary_line_has_metric_label(normalized, "p1")
        || normalized.contains("p1 finding")
        || normalized.contains("p1 issue")
        || normalized.contains("p1 correctness"))
        && !summary_line_negates_p1_outcome(normalized)
}

fn summary_line_negates_p1_outcome(normalized: &str) -> bool {
    normalized.contains("no p1")
        || summary_line_has_zero_count(normalized, "0 p1")
        || summary_line_has_zero_count_metric(normalized, &["p1"])
}

fn summary_line_has_zero_count(normalized: &str, prefix: &str) -> bool {
    normalized.starts_with(prefix) || normalized.contains(&format!(" {prefix}"))
}

fn summary_line_has_zero_count_metric(normalized: &str, labels: &[&str]) -> bool {
    const ZERO_COUNT_NOUNS: &[&str] = &[
        "bug",
        "bugs",
        "defect",
        "defects",
        "finding",
        "findings",
        "issue",
        "issues",
        "violation",
        "violations",
        "vulnerability",
        "vulnerabilities",
    ];

    labels.iter().any(|label| {
        summary_line_has_zero_metric(normalized, label)
            || ZERO_COUNT_NOUNS
                .iter()
                .any(|noun| summary_line_has_zero_metric(normalized, &format!("{label} {noun}")))
    })
}

fn summary_line_has_zero_metric(normalized: &str, label: &str) -> bool {
    summary_metric_label_variants(label).iter().any(|variant| {
        normalized.contains(&format!("{variant}: 0"))
            || normalized.contains(&format!("{variant} = 0"))
    })
}

fn summary_line_has_metric_label(normalized: &str, label: &str) -> bool {
    summary_metric_label_variants(label).iter().any(|variant| {
        normalized.starts_with(&format!("{variant}:"))
            || normalized.starts_with(&format!("{variant} ="))
            || normalized.contains(&format!(" {variant}:"))
            || normalized.contains(&format!(" {variant} ="))
    })
}

fn summary_metric_label_variants(label: &str) -> [String; 4] {
    [
        label.to_string(),
        format!("**{label}**"),
        format!("__{label}__"),
        format!("`{label}`"),
    ]
}

fn summary_line_has_verdict_prefix(line: &str, token: &str) -> bool {
    let stripped = line.trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, '*' | '_' | '`' | '#' | '-' | '>')
    });
    let Some(prefix) = stripped.get(..token.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(token) {
        return false;
    }

    summary_verdict_token_is_bounded(&stripped[token.len()..])
}

fn summary_verdict_token_is_bounded(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        None => true,
        Some(ch) if ch.is_ascii_alphanumeric() || ch == '_' => false,
        Some('-') | Some('/') => chars
            .next()
            .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_'),
        _ => true,
    }
}

fn sync_result_exit_code(result: &mut SessionResult, exit_code: i32) -> bool {
    let status = SessionResult::status_from_exit_code(exit_code);
    if result.exit_code == exit_code && result.status == status {
        return false;
    }

    result.exit_code = exit_code;
    result.status = status;
    true
}

fn read_review_verdict_exit_code(session_dir: &Path) -> Result<Option<i32>> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&verdict_path)?;
    let artifact: ReviewVerdictArtifact = serde_json::from_str(&raw)?;
    Ok(Some(
        crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision),
    ))
}

pub(crate) fn build_missing_result_diagnostic(
    session_id: &str,
    session_dir: &Path,
    phase_label: Option<&str>,
) -> String {
    let phase_suffix = phase_label
        .map(|phase| format!(" Phase: {phase}."))
        .unwrap_or_default();
    let available = describe_available_diagnostics(session_dir);
    if available.is_empty() {
        format!("No result found for session '{session_id}'.{phase_suffix}")
    } else {
        format!(
            "No result found for session '{session_id}'.{phase_suffix} Available diagnostics: {available}."
        )
    }
}

pub(crate) fn build_missing_logs_diagnostic(
    session_id: &str,
    session_dir: &Path,
    result: Option<&SessionResult>,
) -> String {
    let result_detail = format_result_detail(result);
    let available = describe_available_diagnostics(session_dir);
    if available.is_empty() {
        format!("No logs found for session {session_id}.{result_detail}")
    } else {
        format!(
            "No logs found for session {session_id}.{result_detail} Available diagnostics: {available}."
        )
    }
}

pub(crate) fn build_missing_events_diagnostic(
    session_id: &str,
    session_dir: &Path,
    result: Option<&SessionResult>,
) -> String {
    let result_detail = format_result_detail(result);
    let available = describe_available_diagnostics(session_dir);
    if available.is_empty() {
        format!(
            "No ACP events found for session {session_id}. Transcript capture may be disabled or incomplete.{result_detail}"
        )
    } else {
        format!(
            "No ACP events found for session {session_id}. Transcript capture may be disabled or incomplete.{result_detail} Available diagnostics: {available}."
        )
    }
}

pub(crate) fn refresh_structured_output(session_dir: &Path) {
    let output_log = session_dir.join("output.log");
    if !output_log.is_file() {
        return;
    }

    if let Err(err) = csa_session::persist_structured_output_from_file(session_dir, &output_log) {
        debug!(
            path = %output_log.display(),
            error = %err,
            "Failed to refresh structured output from output.log"
        );
    }
}

fn derive_better_summary(session_dir: &Path, current_summary: &str) -> Result<Option<String>> {
    if !is_low_signal_summary(current_summary) {
        return Ok(None);
    }

    if let Some(content) = csa_session::read_section(session_dir, "summary")?
        && let Some(summary) = select_summary_line(&content, false)
    {
        return Ok(Some(summary));
    }

    let output_log = session_dir.join("output.log");
    if output_log.is_file() {
        let output = fs::read_to_string(&output_log)?;
        if let Some(content) = extract_marked_section(&output, "summary")
            && let Some(summary) = select_summary_line(content, false)
        {
            return Ok(Some(summary));
        }

        if let Some(summary) = select_summary_line(&output, true) {
            return Ok(Some(summary));
        }
    }

    for path in ["stdout.log", "stderr.log"] {
        let log_path = session_dir.join(path);
        if !log_path.is_file() {
            continue;
        }

        let content = fs::read_to_string(&log_path)?;
        if let Some(summary) = select_summary_line(&content, true) {
            return Ok(Some(summary));
        }
    }

    Ok(None)
}

fn count_transcript_events(session_dir: &Path) -> Result<Option<u64>> {
    let transcript_path = session_dir.join("output").join("acp-events.jsonl");
    if !transcript_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(transcript_path)?;
    let count = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    Ok(Some(count))
}

fn merge_artifacts(artifacts: &mut Vec<SessionArtifact>, artifact_names: Vec<String>) -> bool {
    let mut changed = false;
    let mut seen: HashSet<String> = artifacts
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect();

    for name in artifact_names {
        let path = format!("output/{name}");
        if seen.insert(path.clone()) {
            artifacts.push(SessionArtifact::new(path));
            changed = true;
        }
    }

    if changed {
        artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    }

    changed
}

fn describe_available_diagnostics(session_dir: &Path) -> String {
    let mut available = Vec::new();

    let logs_dir = session_dir.join("logs");
    if logs_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&logs_dir)
    {
        let count = entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
            .count();
        if count > 0 {
            available.push(format!("logs/{count} file(s)"));
        }
    }

    for path in ["stdout.log", "stderr.log", "output.log"] {
        let full_path = session_dir.join(path);
        if full_path.is_file()
            && fs::metadata(&full_path)
                .map(|meta| meta.len() > 0)
                .unwrap_or(false)
        {
            available.push(path.to_string());
        }
    }

    let transcript_path = session_dir.join("output").join("acp-events.jsonl");
    if transcript_path.is_file() {
        available.push("output/acp-events.jsonl".to_string());
    }

    let output_index = session_dir.join("output").join("index.toml");
    if output_index.is_file() {
        available.push("structured output".to_string());
    }

    available.join(", ")
}

fn format_result_detail(result: Option<&SessionResult>) -> String {
    let Some(result) = result else {
        return String::new();
    };

    format!(
        " Result: {} (exit {}). Summary: {}.",
        result.status,
        result.exit_code,
        truncate_summary(&result.summary)
    )
}

fn extract_marked_section<'a>(text: &'a str, section_id: &str) -> Option<&'a str> {
    let start_marker = format!("<!-- CSA:SECTION:{section_id} -->");
    let end_marker = format!("<!-- CSA:SECTION:{section_id}:END -->");
    let start = text.find(&start_marker)?;
    let remaining = &text[start + start_marker.len()..];
    let end = remaining.find(&end_marker)?;
    Some(remaining[..end].trim())
}

fn select_summary_line(text: &str, prefer_tail: bool) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !is_noise_line(line))
        .collect();

    if lines.is_empty() {
        return None;
    }

    if prefer_tail {
        if let Some(line) = lines.iter().rev().find(|line| looks_like_sentence(line)) {
            return Some(truncate_summary(line));
        }
        return lines.last().map(|line| truncate_summary(line));
    }

    if let Some(line) = lines.iter().find(|line| looks_like_sentence(line)) {
        return Some(truncate_summary(line));
    }

    lines.first().map(|line| truncate_summary(line))
}

fn is_low_signal_summary(summary: &str) -> bool {
    let trimmed = summary.trim();
    trimmed.is_empty()
        || is_noise_line(trimmed)
        || !trimmed
            .chars()
            .any(|ch| ch.is_ascii_alphanumeric() || is_cjk(ch))
}

fn is_noise_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty()
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
        || trimmed.starts_with("<!-- CSA:SECTION:")
        || trimmed.starts_with("[tool:")
        || trimmed.starts_with("[plan]")
        || trimmed.starts_with("[thought]")
        || trimmed.starts_with("[stdout]")
        || trimmed.starts_with("[csa-heartbeat]")
        || trimmed.starts_with("[CSA:TRUNCATED")
}

fn looks_like_sentence(line: &&str) -> bool {
    let trimmed = line.trim();
    if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("```")
        || trimmed.starts_with('[')
    {
        return false;
    }

    trimmed
        .chars()
        .any(|ch| ch.is_ascii_alphanumeric() || is_cjk(ch))
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
    )
}

fn truncate_summary(line: &str) -> String {
    line.chars().take(SUMMARY_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_summary_line_prefers_sentence_over_code_fence() {
        let text = "```\nactual summary line\n```";
        let summary = select_summary_line(text, true).expect("summary");
        assert_eq!(summary, "actual summary line");
    }

    #[test]
    fn extract_marked_section_returns_summary_payload() {
        let text = "before\n<!-- CSA:SECTION:summary -->\nSummary body\n<!-- CSA:SECTION:summary:END -->\nafter\n";
        let section = extract_marked_section(text, "summary").expect("section");
        assert_eq!(section, "Summary body");
    }

    #[test]
    fn is_low_signal_summary_flags_markdown_fence() {
        assert!(is_low_signal_summary("```"));
        assert!(is_low_signal_summary("~~~json"));
        assert!(!is_low_signal_summary("Task complete"));
    }

    #[test]
    fn derive_better_summary_repairs_csa_marker_summary_from_output_log() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let session_dir = temp.path();
        fs::write(
            session_dir.join("output.log"),
            "<!-- CSA:SECTION:summary -->\nRecovered summary line\n<!-- CSA:SECTION:summary:END -->\n",
        )
        .expect("write output log");

        let repaired = derive_better_summary(session_dir, "<!-- CSA:SECTION:summary:END -->")
            .expect("derive summary");

        assert_eq!(repaired.as_deref(), Some("Recovered summary line"));
    }
}
