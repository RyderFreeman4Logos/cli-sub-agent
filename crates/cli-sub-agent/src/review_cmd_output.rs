use std::fs;
use std::path::Path;
use std::str::FromStr;

use csa_core::types::{ReviewDecision, ToolName};
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{
    Finding, ReviewVerdictArtifact, Severity, SeveritySummary, write_review_verdict,
};
use csa_session::{output_parser::parse_sections, output_section::OutputSection};
use regex::Regex;
use serde::Deserialize;
use tracing::{debug, warn};

const REVIEW_RESULT_SUMMARY_MAX_CHARS: usize = 200;
const EDIT_RESTRICTION_SUMMARY_PREFIX: &str = "Edit restriction violated:";

#[derive(Debug, Clone)]
pub(super) struct ReviewerOutcome {
    pub reviewer_index: usize,
    pub tool: ToolName,
    pub session_id: String,
    pub output: String,
    pub exit_code: i32,
    pub verdict: &'static str,
    /// Tool-level diagnostic when the review failed due to tool issues (e.g. MCP).
    pub diagnostic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PersistedReviewArtifact {
    #[serde(default)]
    findings: Vec<Finding>,
    #[serde(default)]
    severity_summary: SeveritySummary,
    #[serde(default)]
    overall_risk: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TranscriptEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    item: Option<TranscriptItem>,
}

#[derive(Debug, Deserialize)]
struct TranscriptItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// Prefer structured review sections (summary/details) when available to avoid
/// leaking unrelated provider noise into caller-facing review output.
pub(super) fn sanitize_review_output(output: &str) -> String {
    let sections = parse_sections(output);
    if sections.is_empty() {
        return output.to_string();
    }

    let summary = last_non_empty_section_content(output, &sections, "summary");
    let details = last_non_empty_section_content(output, &sections, "details");
    if summary.is_none() && details.is_none() {
        return output.to_string();
    }

    let mut rendered = String::new();
    if let Some(content) = summary {
        rendered.push_str("<!-- CSA:SECTION:summary -->\n");
        rendered.push_str(&content);
        if !content.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:summary:END -->\n");
    }
    if let Some(content) = details {
        if !rendered.is_empty() && !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:details -->\n");
        rendered.push_str(&content);
        if !content.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:details:END -->\n");
    }
    rendered
}

pub(super) fn has_structured_review_content(output: &str) -> bool {
    let sanitized = sanitize_review_output(output);
    let sections = parse_sections(&sanitized);
    ["summary", "details"].into_iter().any(|section_id| {
        last_non_empty_section_content(&sanitized, &sections, section_id).is_some()
    })
}

pub(super) fn derive_review_result_summary(output: &str) -> Option<String> {
    let sanitized = sanitize_review_output(output);
    let sections = parse_sections(&sanitized);
    let content = last_non_empty_section_content(&sanitized, &sections, "summary")
        .or_else(|| last_non_empty_section_content(&sanitized, &sections, "details"))?;

    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(truncate_review_result_summary)
}

pub(super) fn is_edit_restriction_summary(summary: &str) -> bool {
    summary.starts_with(EDIT_RESTRICTION_SUMMARY_PREFIX)
}

fn last_non_empty_section_content(
    output: &str,
    sections: &[OutputSection],
    section_id: &str,
) -> Option<String> {
    sections
        .iter()
        .rev()
        .filter(|section| section.id == section_id)
        .find_map(|section| {
            let content = extract_section_content(output, section);
            if content.trim().is_empty() {
                None
            } else {
                Some(content)
            }
        })
}

fn extract_section_content(output: &str, section: &OutputSection) -> String {
    if section.line_start == 0 || section.line_end < section.line_start {
        return String::new();
    }

    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() || section.line_start > lines.len() {
        return String::new();
    }

    let start = section.line_start - 1;
    let end_exclusive = section.line_end.min(lines.len());
    lines[start..end_exclusive].join("\n")
}

/// Persist a [`ReviewSessionMeta`] to `{session_dir}/review_meta.json`.
///
/// Best-effort: failures are logged as warnings but do not fail the review.
pub(super) fn persist_review_meta(project_root: &Path, meta: &ReviewSessionMeta) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            if let Err(e) = write_review_meta(&session_dir, meta) {
                warn!(session_id = %meta.session_id, error = %e, "Failed to write review_meta.json");
            } else {
                debug!(session_id = %meta.session_id, "Wrote review_meta.json");
            }
        }
        Err(e) => {
            warn!(session_id = %meta.session_id, error = %e, "Cannot resolve session dir for review meta");
        }
    }
}

/// Persist a [`ReviewVerdictArtifact`] to `{session_dir}/output/review-verdict.json`.
///
/// Best-effort: failures are logged as warnings but do not fail the review.
pub(super) fn persist_review_verdict(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
    prior_round_refs: Vec<String>,
) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            let verdict_path = session_dir.join("output").join("review-verdict.json");
            if verdict_path.exists() {
                debug!(
                    session_id = %meta.session_id,
                    path = %verdict_path.display(),
                    "Skipping output/review-verdict.json persistence because AI artifact already exists"
                );
                return;
            }
            let artifact = match derive_review_verdict_artifact(&session_dir, meta, findings) {
                Ok(mut artifact) => {
                    artifact.prior_round_refs = prior_round_refs.clone();
                    artifact
                }
                Err(error) => {
                    debug!(
                        session_id = %meta.session_id,
                        error = %error,
                        "Failed to derive review-verdict artifact; falling back to review_meta defaults"
                    );
                    ReviewVerdictArtifact::from_parts(
                        meta.session_id.clone(),
                        ReviewDecision::from_str(&meta.decision)
                            .unwrap_or(ReviewDecision::Uncertain),
                        meta.verdict.clone(),
                        findings,
                        prior_round_refs.clone(),
                    )
                }
            };
            if let Err(e) = write_review_verdict(&session_dir, &artifact) {
                warn!(
                    session_id = %meta.session_id,
                    error = %e,
                    "Failed to write output/review-verdict.json"
                );
            } else {
                debug!(session_id = %meta.session_id, "Wrote output/review-verdict.json");
            }
        }
        Err(e) => {
            warn!(
                session_id = %meta.session_id,
                error = %e,
                "Cannot resolve session dir for review verdict"
            );
        }
    }
}

fn derive_review_verdict_artifact(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
) -> Result<ReviewVerdictArtifact, anyhow::Error> {
    if let Some(artifact) = load_review_artifact_from_output(session_dir)? {
        let decision = derive_decision_from_findings(
            artifact.findings.is_empty(),
            artifact.overall_risk.as_deref(),
            ReviewDecision::from_str(&meta.decision).ok(),
        );
        return Ok(build_review_verdict_artifact(
            meta.session_id.clone(),
            decision,
            legacy_verdict_for_decision(decision, &meta.verdict),
            severity_counts_from_summary(&artifact.severity_summary),
            Vec::new(),
        ));
    }

    if let Some(artifact) = infer_review_verdict_from_full_output(session_dir, meta)? {
        return Ok(artifact);
    }

    Ok(ReviewVerdictArtifact::from_parts(
        meta.session_id.clone(),
        ReviewDecision::from_str(&meta.decision).unwrap_or(ReviewDecision::Uncertain),
        meta.verdict.clone(),
        findings,
        Vec::new(),
    ))
}

fn load_review_artifact_from_output(
    session_dir: &Path,
) -> Result<Option<PersistedReviewArtifact>, anyhow::Error> {
    let findings_path = session_dir.join("review-findings.json");
    if !findings_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&findings_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", findings_path.display()))?;
    let artifact = serde_json::from_str::<PersistedReviewArtifact>(&contents)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", findings_path.display()))?;
    Ok(Some(artifact))
}

fn infer_review_verdict_from_full_output(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
) -> Result<Option<ReviewVerdictArtifact>, anyhow::Error> {
    let full_output_path = session_dir.join("output").join("full.md");
    if !full_output_path.exists() {
        return Ok(None);
    }

    let raw_output = fs::read_to_string(&full_output_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", full_output_path.display()))?;
    let Some(review_text) = extract_review_text(&raw_output) else {
        return Ok(None);
    };

    if !has_structured_review_content(&review_text) {
        return Ok(None);
    }

    let counts = severity_counts_from_text(&review_text);
    let overall_risk = parse_overall_risk_from_text(&review_text);
    let decision = derive_decision_from_text(&review_text, &counts, overall_risk.as_deref());
    Ok(Some(build_review_verdict_artifact(
        meta.session_id.clone(),
        decision,
        legacy_verdict_for_decision(decision, &meta.verdict),
        counts,
        Vec::new(),
    )))
}

fn extract_review_text(raw_output: &str) -> Option<String> {
    let mut transcript_messages = Vec::new();
    let mut saw_json_line = false;

    for line in raw_output.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<TranscriptEvent>(line) else {
            return Some(raw_output.to_string());
        };
        saw_json_line = true;
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
    has_structured_review_content(text)
        || contains_verdict_token(text, "PASS")
        || contains_verdict_token(text, "CLEAN")
        || contains_verdict_token(text, "FAIL")
        || contains_verdict_token(text, "HAS_ISSUES")
        || contains_verdict_token(text, "UNCERTAIN")
        || contains_clean_phrase(text)
        || text.lines().any(|line| {
            is_findings_header(line) || line.to_ascii_lowercase().contains("overall risk")
        })
}

fn severity_counts_from_summary(
    summary: &SeveritySummary,
) -> std::collections::BTreeMap<Severity, u32> {
    [
        (Severity::Critical, summary.critical),
        (Severity::High, summary.high),
        (Severity::Medium, summary.medium),
        (Severity::Low, summary.low),
        (Severity::Info, summary.info),
    ]
    .into_iter()
    .collect()
}

fn zero_severity_counts() -> std::collections::BTreeMap<Severity, u32> {
    severity_counts_from_summary(&SeveritySummary::default())
}

fn severity_counts_from_text(text: &str) -> std::collections::BTreeMap<Severity, u32> {
    let marker_re =
        Regex::new(r"(?i)\[(critical|high|medium|low|info|p[0-4])\]").expect("valid regex");
    let mut counts = zero_severity_counts();

    for captures in marker_re.captures_iter(text) {
        let severity = match captures.get(1).map(|m| m.as_str().to_ascii_lowercase()) {
            Some(level) if level == "critical" => Severity::Critical,
            Some(level) if level == "high" => Severity::High,
            Some(level) if level == "medium" => Severity::Medium,
            Some(level) if level == "low" => Severity::Low,
            Some(level) if level == "info" => Severity::Info,
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

fn parse_overall_risk_from_text(text: &str) -> Option<String> {
    let risk_re = Regex::new(r"(?im)\boverall risk\b\s*:?\s*(critical|high|medium|low)\b")
        .expect("valid regex");
    risk_re
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|level| level.as_str().to_ascii_lowercase())
}

fn derive_decision_from_findings(
    findings_empty: bool,
    overall_risk: Option<&str>,
    meta_decision: Option<ReviewDecision>,
) -> ReviewDecision {
    if findings_empty {
        match meta_decision {
            Some(
                meta_decision @ (ReviewDecision::Skip
                | ReviewDecision::Uncertain
                | ReviewDecision::Fail),
            ) => {
                return meta_decision;
            }
            Some(ReviewDecision::Pass)
                if overall_risk.is_none_or(|risk| risk.eq_ignore_ascii_case("low")) =>
            {
                return ReviewDecision::Pass;
            }
            Some(ReviewDecision::Pass) => return ReviewDecision::Fail,
            None if overall_risk.is_none_or(|risk| risk.eq_ignore_ascii_case("low")) => {
                return ReviewDecision::Uncertain;
            }
            None => return ReviewDecision::Fail,
        }
    }

    ReviewDecision::Fail
}

fn derive_decision_from_text(
    text: &str,
    counts: &std::collections::BTreeMap<Severity, u32>,
    overall_risk: Option<&str>,
) -> ReviewDecision {
    if counts.values().any(|count| *count > 0) {
        return ReviewDecision::Fail;
    }
    if contains_verdict_token(text, "FAIL") || contains_verdict_token(text, "HAS_ISSUES") {
        return ReviewDecision::Fail;
    }
    if contains_verdict_token(text, "SKIP") {
        return ReviewDecision::Skip;
    }
    if contains_verdict_token(text, "UNCERTAIN") {
        return ReviewDecision::Uncertain;
    }
    if counts.values().any(|count| *count > 0) {
        return ReviewDecision::Fail;
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
    ReviewDecision::Uncertain
}

fn build_review_verdict_artifact(
    session_id: String,
    decision: ReviewDecision,
    verdict_legacy: String,
    severity_counts: std::collections::BTreeMap<Severity, u32>,
    prior_round_refs: Vec<String>,
) -> ReviewVerdictArtifact {
    ReviewVerdictArtifact {
        schema_version: csa_session::review_artifact::REVIEW_VERDICT_SCHEMA_VERSION,
        session_id,
        timestamp: chrono::Utc::now(),
        decision,
        verdict_legacy,
        severity_counts,
        prior_round_refs,
    }
}

fn legacy_verdict_for_decision(decision: ReviewDecision, fallback: &str) -> String {
    match decision {
        ReviewDecision::Pass => "CLEAN".to_string(),
        ReviewDecision::Fail => "HAS_ISSUES".to_string(),
        ReviewDecision::Skip | ReviewDecision::Uncertain => fallback.to_string(),
    }
}

/// Detect whether `project_root` resides inside a git worktree submodule.
///
/// A worktree submodule's `.git` is a file (not directory) containing a
/// `gitdir:` reference that traverses both `worktrees/` and `modules/`
/// path segments — the hallmark of the nested worktree-submodule layout.
pub(super) fn is_worktree_submodule(project_root: &Path) -> bool {
    let git_marker = project_root.join(".git");
    if !git_marker.is_file() {
        return false;
    }
    let Ok(marker) = std::fs::read_to_string(&git_marker) else {
        return false;
    };
    let Some(gitdir_raw) = marker.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let gitdir = gitdir_raw.trim();
    gitdir.contains("/worktrees/") && gitdir.contains("/modules/")
}

/// Detect known tool-level diagnostic messages that indicate the review tool
/// failed to actually perform a review (e.g., gemini-cli MCP connectivity issues).
///
/// Checks both stdout and stderr for known failure patterns.
/// Returns a human-readable diagnostic summary when a known pattern is found.
pub(super) fn detect_tool_diagnostic(stdout: &str, stderr: &str) -> Option<String> {
    let has_mcp_issue =
        |text: &str| text.contains("MCP issues detected") || text.contains("Run /mcp list");

    if has_mcp_issue(stdout) || has_mcp_issue(stderr) {
        return Some(
            "gemini-cli encountered MCP server connectivity issues. \
             Run `gemini /mcp list` to diagnose. \
             Consider using `--tool claude-code` as a fallback."
                .to_string(),
        );
    }

    None
}

/// Print per-reviewer output and diagnostics for multi-reviewer mode.
pub(super) fn print_reviewer_outcomes(outcomes: &[ReviewerOutcome]) {
    for o in outcomes {
        let r = o.reviewer_index + 1;
        println!(
            "===== Reviewer {r} ({}) | verdict={} | exit_code={} =====",
            o.tool, o.verdict, o.exit_code
        );
        if let Some(ref d) = o.diagnostic {
            eprintln!("[csa-review] Reviewer {r} tool failure: {d}");
        }
        print!("{}", o.output);
        if !o.output.ends_with('\n') {
            println!();
        }
    }
}

/// Check whether review output contains substantive content beyond prompt guards.
///
/// Returns `true` when the raw output is empty or contains only CSA prompt
/// injection markers / hook output and whitespace — indicating the review tool
/// produced no actual findings.
pub(super) fn is_review_output_empty(raw_output: &str) -> bool {
    strip_prompt_guards(raw_output).trim().is_empty()
}

/// Remove non-review content: prompt injection blocks, hook markers, and section wrappers.
fn strip_prompt_guards(text: &str) -> String {
    let mut result = String::new();
    let mut in_guard = false;
    for line in text.lines() {
        if line.contains("<csa-caller-prompt-injection") {
            in_guard = true;
            continue;
        }
        if line.contains("</csa-caller-prompt-injection>") {
            in_guard = false;
            continue;
        }
        if in_guard {
            continue;
        }
        if line.trim_start().starts_with("[csa-hook]") {
            continue;
        }
        if line.trim_start().starts_with("[csa-heartbeat]") {
            continue;
        }
        // Strip CSA section markers (empty wrappers are not substantive content)
        if line.trim_start().starts_with("<!-- CSA:SECTION:") {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

fn truncate_review_result_summary(line: &str) -> String {
    line.chars().take(REVIEW_RESULT_SUMMARY_MAX_CHARS).collect()
}

fn contains_verdict_token(haystack: &str, token: &str) -> bool {
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|part| part.eq_ignore_ascii_case(token))
}

fn contains_clean_phrase(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "no issues found",
        "no issues were found",
        "no blocking issues",
        "no findings",
        "\u{672a}\u{53d1}\u{73b0}\u{95ee}\u{9898}",
        "\u{6ca1}\u{6709}\u{53d1}\u{73b0}\u{95ee}\u{9898}",
        "\u{65e0}\u{963b}\u{585e}\u{95ee}\u{9898}",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        || contains_positive_no_issue_clause(&lower)
}

fn contains_positive_no_issue_clause(lower: &str) -> bool {
    const NOUNS: &[&str] = &[
        "issue", "issues", "finding", "findings", "concern", "concerns",
    ];
    const TAIL_VERBS: &[&str] = &["found", "identified", "detected", "introduced"];
    const MAX_TOKENS_BEFORE_NOUN: usize = 6;
    const MAX_TOKENS_AFTER_NOUN: usize = 4;

    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();

    for (no_index, token) in tokens.iter().enumerate() {
        if *token != "no" {
            continue;
        }

        let search_end = (no_index + 1 + MAX_TOKENS_BEFORE_NOUN).min(tokens.len());
        let Some(relative_noun_index) = tokens[no_index + 1..search_end]
            .iter()
            .position(|candidate| NOUNS.contains(candidate))
        else {
            continue;
        };
        let noun_index = no_index + 1 + relative_noun_index;
        let tail_end = (noun_index + 1 + MAX_TOKENS_AFTER_NOUN).min(tokens.len());
        if tokens[noun_index + 1..tail_end]
            .iter()
            .any(|candidate| TAIL_VERBS.contains(candidate))
        {
            return true;
        }
    }

    false
}

fn is_findings_header(line: &str) -> bool {
    let trimmed = line.trim();
    let normalized = trimmed.trim_start_matches('#').trim();
    normalized.eq_ignore_ascii_case("findings")
        || normalized.eq_ignore_ascii_case("review findings")
}

#[cfg(test)]
#[path = "review_cmd_output_tests.rs"]
mod tests;
