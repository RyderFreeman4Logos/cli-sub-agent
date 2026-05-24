use std::path::Path;
use std::{fs, str::FromStr};

use anyhow::Result;
use csa_core::gemini::RATE_LIMIT_PATTERNS;
use csa_core::types::{ReviewDecision, ToolName};
use csa_executor::{
    contains_gemini_oauth_prompt, normalize_gemini_prompt_text, strip_ansi_escape_sequences,
};
use csa_session::output_parser::parse_sections;
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{Finding, OutputSection, ReviewVerdictArtifact, Severity, write_review_verdict};
use tracing::{debug, warn};

#[path = "review_cmd_output_artifacts.rs"]
mod artifacts;
#[path = "review_cmd_output_clean.rs"]
mod clean_detection;
#[path = "review_cmd_output_diagnostics.rs"]
mod diagnostics;
#[path = "review_cmd_output_exit.rs"]
mod exit_code;
#[path = "review_cmd_output_summary.rs"]
mod summary_artifact;
#[path = "review_cmd_output_text.rs"]
mod text;
use artifacts::{
    has_blocking_severity, json_severity_counts_if_present, load_findings_toml_from_output,
    load_review_artifact_from_output, severity_counts_are_zero, severity_counts_for_artifact,
    severity_counts_for_findings_toml,
};
use clean_detection::{review_contains_prose_clean_conclusion, strip_prompt_guards};
pub(crate) use diagnostics::detect_tool_diagnostic;
pub(super) use diagnostics::{ReviewerOutcome, print_reviewer_outcomes};
pub(super) use exit_code::{persist_review_result_exit_code, persisted_review_verdict_exit_code};
pub(super) use summary_artifact::{
    ensure_review_summary_artifact, is_edit_restriction_summary, truncate_review_result_summary,
};
pub(super) use text::extract_review_text;
use text::{
    derive_decision_from_text, parse_overall_risk_from_text, severity_counts_from_text,
    zero_severity_counts,
};

const REVIEW_RESULT_SUMMARY_MAX_CHARS: usize = 200;
const EDIT_RESTRICTION_SUMMARY_PREFIX: &str = "Edit restriction violated:";
pub(super) const GEMINI_AUTH_PROMPT_STATUS_REASON: &str = "gemini_auth_prompt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolReviewFailureKind {
    GeminiAuthPromptDetected,
}

impl ToolReviewFailureKind {
    pub(super) fn status_reason(self) -> &'static str {
        match self {
            Self::GeminiAuthPromptDetected => GEMINI_AUTH_PROMPT_STATUS_REASON,
        }
    }

    pub(super) fn summary_note(self) -> &'static str {
        match self {
            Self::GeminiAuthPromptDetected => {
                "gemini-cli auth failure: OAuth browser prompt detected; no review verdict produced"
            }
        }
    }
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
            let mut artifact = if meta.status_reason.is_some() {
                let mut artifact = ReviewVerdictArtifact::from_parts(
                    meta.session_id.clone(),
                    ReviewDecision::from_str(&meta.decision).unwrap_or(ReviewDecision::Uncertain),
                    meta.verdict.clone(),
                    findings,
                    prior_round_refs.clone(),
                );
                artifact.routed_to = meta.routed_to.clone();
                artifact.primary_failure = meta.primary_failure.clone();
                artifact.failure_reason = meta.failure_reason.clone();
                artifact
            } else {
                match derive_review_verdict_artifact(&session_dir, meta, findings) {
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
                }
            };
            artifact.routed_to = meta.routed_to.clone();
            artifact.primary_failure = meta.primary_failure.clone();
            artifact.failure_reason = meta.failure_reason.clone();
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
#[cfg(test)]
pub(crate) fn persist_review_verdict_for_tests(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
    prior_round_refs: Vec<String>,
) {
    persist_review_verdict(project_root, meta, findings, prior_round_refs);
}

/// Build a [`ReviewVerdictArtifact`] from meta fields + computed decision/counts.
fn verdict_from_meta(
    meta: &ReviewSessionMeta,
    decision: ReviewDecision,
    severity_counts: std::collections::BTreeMap<Severity, u32>,
) -> ReviewVerdictArtifact {
    build_review_verdict_artifact(
        meta.session_id.clone(),
        decision,
        legacy_verdict_for_decision(decision, &meta.verdict),
        severity_counts,
        meta.routed_to.clone(),
        meta.primary_failure.clone(),
        meta.failure_reason.clone(),
        Vec::new(),
    )
}

/// Cross-check review-findings.json when findings.toml shows zero counts.
/// Returns `Some(artifact)` if JSON has blocking findings; `None` otherwise.
fn cross_check_json_for_blocking(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
) -> Result<Option<ReviewVerdictArtifact>, anyhow::Error> {
    let Some(json_artifact) = load_review_artifact_from_output(session_dir)? else {
        return Ok(None);
    };
    let json_counts = severity_counts_for_artifact(&json_artifact, zero_severity_counts);
    if !has_blocking_severity(&json_counts) {
        return Ok(None);
    }
    let decision = derive_decision_from_severity_counts(
        &json_counts,
        json_artifact.findings.is_empty(),
        json_artifact.overall_risk.as_deref(),
        ReviewDecision::from_str(&meta.decision).ok(),
        || review_contains_prose_clean_conclusion(session_dir),
    )?;
    Ok(Some(verdict_from_meta(meta, decision, json_counts)))
}

fn derive_review_verdict_artifact(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
) -> Result<ReviewVerdictArtifact, anyhow::Error> {
    let mut synthetic_empty_findings_counts = None;
    if let Some(findings_file) = load_findings_toml_from_output(session_dir)? {
        let severity_counts =
            severity_counts_for_findings_toml(&findings_file, zero_severity_counts);

        // Detect synthetic-empty findings.toml: the sidecar marker is written by
        // persist_review_findings_toml when TOML extraction failed (#1045 round 3).
        let synthetic_marker = session_dir
            .join("output")
            .join(super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
        let is_synthetic = synthetic_marker.exists();

        // Synthetic-empty + zero counts → fall through to full.md chain (#1045 r3).
        if is_synthetic
            && findings_file.findings.is_empty()
            && severity_counts_are_zero(&severity_counts)
        {
            if let Some(artifact) = cross_check_json_for_blocking(session_dir, meta)? {
                return Ok(artifact);
            }
            synthetic_empty_findings_counts = Some(severity_counts.clone());
            // Synthetic-empty + no blocking JSON → fall through to full.md chain.
            debug!(
                session_id = %meta.session_id,
                "Synthetic-empty findings.toml detected; falling through to full.md fallback chain"
            );
        } else {
            // Non-synthetic (trusted) or non-empty findings.toml: cross-check
            // review-findings.json for the empty case (round 2 logic), then return.
            if findings_file.findings.is_empty() && severity_counts_are_zero(&severity_counts) {
                if let Some(artifact) = cross_check_json_for_blocking(session_dir, meta)? {
                    return Ok(artifact);
                }
                // No blocking JSON findings, but JSON may have low-only counts.
                // Preserve them so downstream telemetry sees the low count (#1048 M1).
                if let Some(json_counts) =
                    json_severity_counts_if_present(session_dir, zero_severity_counts)?
                {
                    let decision = derive_decision_from_severity_counts(
                        &json_counts,
                        false, // JSON has findings (low-only)
                        None,
                        ReviewDecision::from_str(&meta.decision).ok(),
                        || review_contains_prose_clean_conclusion(session_dir),
                    )?;
                    return Ok(verdict_from_meta(meta, decision, json_counts));
                }
            }

            let decision = derive_decision_from_severity_counts(
                &severity_counts,
                findings_file.findings.is_empty(),
                None,
                ReviewDecision::from_str(&meta.decision).ok(),
                || review_contains_prose_clean_conclusion(session_dir),
            )?;
            return Ok(verdict_from_meta(meta, decision, severity_counts));
        }
    }

    if let Some(artifact) = load_review_artifact_from_output(session_dir)? {
        let severity_counts = severity_counts_for_artifact(&artifact, zero_severity_counts);
        let decision = derive_decision_from_severity_counts(
            &severity_counts,
            artifact.findings.is_empty(),
            artifact.overall_risk.as_deref(),
            ReviewDecision::from_str(&meta.decision).ok(),
            || review_contains_prose_clean_conclusion(session_dir),
        )?;
        return Ok(verdict_from_meta(meta, decision, severity_counts));
    }

    if let Some(artifact) = infer_review_verdict_from_full_output(session_dir, meta)? {
        return Ok(artifact);
    }

    if let Some(severity_counts) = synthetic_empty_findings_counts {
        return Ok(verdict_from_meta(
            meta,
            ReviewDecision::Pass,
            severity_counts,
        ));
    }

    if !full_output_is_effectively_empty(session_dir)? {
        let decision = if findings.is_empty() {
            ReviewDecision::Pass
        } else {
            ReviewDecision::from_str(&meta.decision).unwrap_or(ReviewDecision::Fail)
        };
        return Ok(ReviewVerdictArtifact::from_parts(
            meta.session_id.clone(),
            decision,
            legacy_verdict_for_decision(decision, &meta.verdict),
            findings,
            Vec::new(),
        ));
    }

    let decision = ReviewDecision::from_str(&meta.decision).unwrap_or(ReviewDecision::Uncertain);
    Ok(ReviewVerdictArtifact::from_parts(
        meta.session_id.clone(),
        decision,
        legacy_verdict_for_decision(decision, &meta.verdict),
        findings,
        Vec::new(),
    ))
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
    Ok(Some(verdict_from_meta(meta, decision, counts)))
}

fn full_output_is_effectively_empty(session_dir: &Path) -> Result<bool, anyhow::Error> {
    let full_output_path = session_dir.join("output").join("full.md");
    if !full_output_path.exists() {
        return Ok(true);
    }

    let raw_output = fs::read_to_string(&full_output_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", full_output_path.display()))?;
    Ok(raw_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .all(|line| line.starts_with('{')))
}

/// Derive the review decision from structured severity counts, not summary-text
/// keywords or stale `meta.decision` values (#1045).
/// Blocking severities fail; low-only findings pass; zero findings and zero
/// severity counts defer to the explicit tie-break rules below.
fn derive_decision_from_severity_counts(
    severity_counts: &std::collections::BTreeMap<Severity, u32>,
    findings_empty: bool,
    overall_risk: Option<&str>,
    meta_decision: Option<ReviewDecision>,
    prose_clean_check: impl FnOnce() -> Result<bool, anyhow::Error>,
) -> Result<ReviewDecision, anyhow::Error> {
    // Blocking findings (critical/high/medium) always fail.
    if has_blocking_severity(severity_counts) {
        return Ok(ReviewDecision::Fail);
    }

    // Non-blocking findings (low only) → pass.
    // Zero severity counts but non-empty findings list → fail-closed (parsing anomaly).
    if !findings_empty && !severity_counts_are_zero(severity_counts) {
        // Only low-severity findings present — non-blocking.
        return Ok(ReviewDecision::Pass);
    }
    if !findings_empty && severity_counts_are_zero(severity_counts) {
        // Findings exist but severity counts are zero (unrecognized severities).
        // Fail-closed.
        return Ok(ReviewDecision::Fail);
    }

    // Honour overall_risk as a fail-closed signal when it's severe.
    if overall_risk.is_some_and(|risk| {
        risk.eq_ignore_ascii_case("high") || risk.eq_ignore_ascii_case("critical")
    }) {
        return Ok(ReviewDecision::Fail);
    }

    // Unavailable NOT propagated here: genuine failures use the status_reason fast-path.
    // Unavailable in meta_decision is prompt-injection noise; zero findings → Pass. (#1340)

    // #1349: Empty findings + zero counts is conclusive Pass; meta_decision from
    // text-parse noise or exit-code fallback must not block on zero-evidence records.
    // #1480: This also covers Skip from meta_decision: when the reviewer text happened
    // to contain "SKIP" but the structured artifact shows zero findings, the zero-evidence
    // Pass conclusion must win over the text-parse Skip noise.
    if findings_empty && severity_counts_are_zero(severity_counts) {
        return Ok(ReviewDecision::Pass);
    }

    // Skip: deliberate "no diff to review" signal — only propagate when there is
    // non-zero evidence (severity counts), i.e. the zero-evidence Pass above did not fire.
    if let Some(ReviewDecision::Skip) = meta_decision {
        return Ok(ReviewDecision::Skip);
    }

    // #1140/#1144: Uncertain/Fail meta + zero counts (findings non-empty) → prose tiebreak.
    if matches!(
        meta_decision,
        Some(ReviewDecision::Uncertain | ReviewDecision::Fail)
    ) && severity_counts_are_zero(severity_counts)
    {
        return Ok(if prose_clean_check()? {
            ReviewDecision::Pass
        } else {
            meta_decision.unwrap_or(ReviewDecision::Uncertain)
        });
    }

    if meta_decision == Some(ReviewDecision::Pass) || prose_clean_check()? {
        return Ok(ReviewDecision::Pass);
    }

    Ok(ReviewDecision::Pass)
}

#[allow(clippy::too_many_arguments)]
fn build_review_verdict_artifact(
    session_id: String,
    decision: ReviewDecision,
    verdict_legacy: String,
    severity_counts: std::collections::BTreeMap<Severity, u32>,
    routed_to: Option<String>,
    primary_failure: Option<String>,
    failure_reason: Option<String>,
    prior_round_refs: Vec<String>,
) -> ReviewVerdictArtifact {
    ReviewVerdictArtifact {
        schema_version: csa_session::review_artifact::REVIEW_VERDICT_SCHEMA_VERSION,
        session_id,
        timestamp: chrono::Utc::now(),
        decision,
        verdict_legacy,
        severity_counts,
        routed_to,
        primary_failure,
        failure_reason,
        prior_round_refs,
    }
}

fn legacy_verdict_for_decision(decision: ReviewDecision, fallback: &str) -> String {
    match decision {
        ReviewDecision::Pass => "CLEAN".to_string(),
        ReviewDecision::Fail => "HAS_ISSUES".to_string(),
        ReviewDecision::Skip | ReviewDecision::Uncertain | ReviewDecision::Unavailable => {
            fallback.to_string()
        }
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

pub(super) fn detect_tool_review_failure(
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

pub(in crate::review_cmd) use clean_detection::is_review_output_empty;

#[cfg(test)]
#[path = "review_cmd_output_fix_reuse_tests.rs"]
mod fix_reuse_tests;
#[cfg(test)]
#[path = "review_cmd_output_tests.rs"]
mod tests;
