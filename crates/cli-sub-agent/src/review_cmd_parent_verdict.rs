use std::path::Path;

use anyhow::{Context, Result};
use csa_core::types::ReviewDecision;
use csa_session::review_artifact::Finding;
use csa_session::{ReviewVerdictArtifact, write_review_verdict};

use super::super::diff_size::{ReviewDiffReport, apply_large_diff_warning};
use super::super::output::ReviewerOutcome;
use crate::review_consensus::UNAVAILABLE;

pub(super) fn write_parent_review_verdict(
    session_dir: &Path,
    session_id: &str,
    severity_count_findings: &[Finding],
    decision: ReviewDecision,
    verdict_legacy: &str,
    diff_report: ReviewDiffReport<'_>,
    review_mode: Option<&str>,
) -> Result<()> {
    let mut verdict = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        decision,
        verdict_legacy.to_string(),
        severity_count_findings,
        Vec::new(),
    );
    verdict.review_mode = review_mode.map(str::to_string);
    verdict.diff_size = diff_report.diff_size.cloned();
    apply_large_diff_warning(&mut verdict, diff_report.large_diff_warning);
    write_review_verdict(session_dir, &verdict)
        .context("failed to write parent output/review-verdict.json")
}

/// Aggregate distinct UNAVAILABLE reviewer reasons for parent meta/verdict (#2911).
/// Bounded + redacted; secrets/paths stay out of the compact carrier.
pub(super) fn aggregate_unavailable_reviewer_reasons(
    outcomes: &[ReviewerOutcome],
) -> (Option<String>, Option<String>) {
    let mut unique: Vec<String> = Vec::new();
    for outcome in outcomes {
        if outcome.verdict != UNAVAILABLE {
            continue;
        }
        let Some(reason) = unavailable_outcome_reason(outcome) else {
            continue;
        };
        let entry = format!("{}={reason}", outcome.tool);
        if !unique.iter().any(|existing| existing == &entry) {
            unique.push(entry);
        }
    }
    if unique.is_empty() {
        return (None, None);
    }
    let primary = unique
        .first()
        .and_then(|entry| entry.split_once('='))
        .map(|(_, reason)| reason.to_string());
    (primary, Some(unique.join("; ")))
}

pub(in crate::review_cmd) fn unavailable_outcome_reason(
    outcome: &ReviewerOutcome,
) -> Option<String> {
    let raw = outcome
        .diagnostic
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            outcome
                .output
                .lines()
                .find_map(|line| line.strip_prefix("Review unavailable: "))
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })?;
    let redacted = crate::review_failure_context::sanitize_review_surface_text(raw);
    let compact = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return None;
    }
    const MAX_CHARS: usize = 360;
    if compact.chars().count() <= MAX_CHARS {
        return Some(compact);
    }
    let mut out = String::new();
    for ch in compact.chars().take(MAX_CHARS.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    Some(out)
}

pub(super) fn patch_parent_review_verdict_unavailable_reasons(
    session_dir: &Path,
    primary_failure: Option<&str>,
    failure_reason: Option<&str>,
) -> Result<()> {
    if primary_failure.is_none() && failure_reason.is_none() {
        return Ok(());
    }
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&verdict_path)
        .with_context(|| format!("failed to read {}", verdict_path.display()))?;
    let mut verdict: ReviewVerdictArtifact = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", verdict_path.display()))?;
    verdict.primary_failure = primary_failure.map(str::to_string);
    verdict.failure_reason = failure_reason.map(str::to_string);
    write_review_verdict(session_dir, &verdict)
        .context("failed to rewrite parent output/review-verdict.json with unavailable reasons")
}

pub(super) fn write_parent_review_summary(
    session_dir: &Path,
    outcomes: &[ReviewerOutcome],
    final_verdict: &str,
    diff_size: Option<&csa_session::ReviewDiffSize>,
) -> Result<()> {
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let mut summary = format!("Final verdict: {final_verdict}\n\nReviewer outcomes:\n");
    if let Some(diff_size) = diff_size {
        summary = format!(
            "{}\n{summary}",
            super::super::diff_size::format_review_diff_size_line(diff_size)
        );
    }
    for outcome in outcomes {
        summary.push_str(&format!(
            "- reviewer {} ({}) => {}",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict
        ));
        if let Some(reason) = unavailable_outcome_reason(outcome) {
            summary.push_str(&format!("; reason: {reason}"));
        } else if let Some(diagnostic) = &outcome.diagnostic {
            summary.push_str(&format!("; diagnostic: {diagnostic}"));
        }
        summary.push('\n');
    }
    if let (_, Some(reason)) = aggregate_unavailable_reviewer_reasons(outcomes) {
        summary.push_str(&format!("\nUnavailable reasons: {reason}\n"));
    }
    std::fs::write(output_dir.join("summary.md"), summary)
        .context("failed to write parent output/summary.md")
}

pub(super) fn write_parent_review_details(
    session_dir: &Path,
    outcomes: &[ReviewerOutcome],
    diff_size: Option<&csa_session::ReviewDiffSize>,
) -> Result<()> {
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let mut details = String::new();
    if let Some(diff_size) = diff_size {
        details.push_str(&super::super::diff_size::format_review_diff_size_line(
            diff_size,
        ));
        details.push_str("\n\n");
    }
    for outcome in outcomes {
        details.push_str(&format!(
            "## Reviewer {} ({})\n\nVerdict: {}\nExit code: {}\n",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict,
            outcome.exit_code
        ));
        if let Some(diagnostic) = &outcome.diagnostic {
            details.push_str(&format!("Diagnostic: {diagnostic}\n"));
        }
        details.push('\n');
        details.push_str(&outcome.output);
        if !details.ends_with('\n') {
            details.push('\n');
        }
        details.push('\n');
    }
    std::fs::write(output_dir.join("details.md"), details)
        .context("failed to write parent output/details.md")
}

#[cfg(test)]
#[path = "review_cmd_parent_unavailable_tests.rs"]
mod tests;
