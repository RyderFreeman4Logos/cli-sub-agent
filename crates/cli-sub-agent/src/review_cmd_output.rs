use std::path::Path;
use std::{fs, str::FromStr};

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{Finding, FindingsFile, ReviewVerdictArtifact, Severity, write_review_verdict};
use tracing::{debug, warn};

#[path = "review_cmd_output_artifacts.rs"]
mod artifacts;
#[path = "review_cmd_output_clean.rs"]
pub(super) mod clean_detection;
#[path = "review_cmd_output_consistency.rs"]
mod consistency;
#[path = "review_cmd_output_diagnostics.rs"]
mod diagnostics;
#[path = "review_cmd_output_exit.rs"]
mod exit_code;
#[path = "review_cmd_output_fail_closed.rs"]
mod fail_closed;
#[path = "review_cmd_output_no_provider.rs"]
mod no_provider;
#[path = "review_cmd_output_prose_signals.rs"]
mod prose_signals;
#[path = "review_cmd_output_sections.rs"]
mod sections;
#[path = "review_cmd_output_summary.rs"]
mod summary_artifact;
#[path = "review_cmd_output_terminal_error.rs"]
mod terminal_error;
#[path = "review_cmd_output_text.rs"]
mod text;
#[path = "review_cmd_output_tool_failure.rs"]
mod tool_failure;
#[path = "review_cmd_output_worktree.rs"]
mod worktree;
use artifacts::{
    has_blocking_severity, json_severity_counts_if_present, load_findings_toml_from_output,
    load_review_artifact_from_output, severity_counts_are_zero, severity_counts_for_artifact,
    severity_counts_for_findings_toml,
};
#[cfg(test)]
use clean_detection::detect_prose_fail_conclusion;
use clean_detection::{
    review_contains_prose_clean_conclusion, review_contains_prose_fail_conclusion,
};
use consistency::enforce_final_verdict_consistency;
pub(crate) use diagnostics::detect_tool_diagnostic;
pub(super) use diagnostics::{ReviewerOutcome, print_reviewer_outcomes};
pub(super) use exit_code::{persist_review_result_exit_code, persisted_review_verdict_exit_code};
pub(super) use fail_closed::fail_closed_review_meta;
use fail_closed::fail_closed_review_verdict_artifact;
use no_provider::attach_no_provider_launch_diagnostic;
use prose_signals::{reconcile_counts_with_prose, review_prose_signals};
pub(super) use sections::{
    derive_review_result_summary, has_structured_review_content, sanitize_review_output,
};
pub(super) use summary_artifact::{
    ensure_review_summary_artifact, is_edit_restriction_summary, truncate_review_result_summary,
};
use terminal_error::terminal_error_artifact_from_full_output;
use text::{
    derive_decision_from_text, parse_overall_risk_from_text, severity_counts_from_text,
    zero_severity_counts,
};
pub(super) use text::{
    extract_review_text, stream_started_without_terminal_event, terminal_tool_error_reason,
};
pub(super) use tool_failure::{ToolReviewFailureKind, detect_tool_review_failure};
pub(super) use worktree::is_worktree_submodule;

const REVIEW_RESULT_SUMMARY_MAX_CHARS: usize = 200;
const EDIT_RESTRICTION_SUMMARY_PREFIX: &str = "Edit restriction violated:";
pub(super) const GEMINI_AUTH_PROMPT_STATUS_REASON: &str = "gemini_auth_prompt";

/// Persist a [`ReviewVerdictArtifact`] to `{session_dir}/output/review-verdict.json`.
///
/// Best-effort: failures are logged as warnings but do not fail the review.
#[cfg(test)]
pub(super) fn persist_review_verdict(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
    prior_round_refs: Vec<String>,
) {
    let _ = persist_review_verdict_artifact(project_root, meta, findings, prior_round_refs);
}

pub(super) fn persist_review_verdict_artifact(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
    prior_round_refs: Vec<String>,
) -> Option<ReviewVerdictArtifact> {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            let mut artifact = if meta.requires_fail_closed_verdict() {
                fail_closed_review_verdict_artifact(meta, findings, prior_round_refs.clone())
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
            artifact.failure_reason = meta.failure_reason.clone().or(artifact.failure_reason);
            artifact.review_mode = meta.review_mode.clone();
            attach_no_provider_launch_diagnostic(&session_dir, meta, &mut artifact);
            if let Err(error) = enforce_final_verdict_consistency(&session_dir, &mut artifact) {
                warn!(
                    session_id = %meta.session_id,
                    error = %error,
                    "Review verdict consistency check failed; writing fail-closed uncertain verdict"
                );
                artifact.decision = ReviewDecision::Uncertain;
                artifact.verdict_legacy = "UNCERTAIN".to_string();
            }
            if let Err(e) = write_review_verdict(&session_dir, &artifact) {
                warn!(
                    session_id = %meta.session_id,
                    error = %e,
                    "Failed to write output/review-verdict.json"
                );
                None
            } else {
                debug!(session_id = %meta.session_id, "Wrote output/review-verdict.json");
                Some(artifact)
            }
        }
        Err(e) => {
            warn!(
                session_id = %meta.session_id,
                error = %e,
                "Cannot resolve session dir for review verdict"
            );
            None
        }
    }
}

pub(super) fn review_meta_for_verdict_artifact(
    meta: &ReviewSessionMeta,
    artifact: &ReviewVerdictArtifact,
) -> ReviewSessionMeta {
    let mut final_meta = meta.clone();
    final_meta.decision = artifact.decision.as_str().to_string();
    final_meta.verdict = artifact.verdict_legacy.clone();
    final_meta.exit_code =
        crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision);
    final_meta
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
    blocking_summary: bool,
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
        || Ok(blocking_summary),
        || review_contains_prose_clean_conclusion(session_dir),
        || review_contains_prose_fail_conclusion(session_dir),
    )?;
    Ok(Some(verdict_from_meta(meta, decision, json_counts)))
}

fn derive_review_verdict_artifact(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
) -> Result<ReviewVerdictArtifact, anyhow::Error> {
    if let Some(artifact) = terminal_error_artifact_from_full_output(session_dir, meta, findings)? {
        return Ok(artifact);
    }

    let prose_signals = review_prose_signals(session_dir)?;
    let mut synthetic_empty_findings_counts = None;
    if let Some(findings_file) = load_findings_toml_from_output(session_dir)? {
        let raw_severity_counts =
            severity_counts_for_findings_toml(&findings_file, zero_severity_counts);

        let synthetic_marker = session_dir
            .join("output")
            .join(super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
        let is_synthetic = synthetic_marker.exists();

        // Synthetic-empty check uses RAW counts (before prose reconciliation).
        // Prose severity extraction can produce phantom counts from descriptive
        // text, which would prevent this fast path from firing (#2002).
        if is_synthetic
            && findings_file.findings.is_empty()
            && severity_counts_are_zero(&raw_severity_counts)
        {
            if let Some(artifact) =
                cross_check_json_for_blocking(session_dir, meta, prose_signals.blocking_summary)?
            {
                return Ok(artifact);
            }
            synthetic_empty_findings_counts = Some(raw_severity_counts.clone());
            // Synthetic-empty + no blocking JSON → fall through to full.md chain.
            debug!(
                session_id = %meta.session_id,
                "Synthetic-empty findings.toml detected; falling through to full.md fallback chain"
            );
        } else {
            let severity_counts =
                reconcile_counts_with_prose(raw_severity_counts, &prose_signals.severity_counts);
            let extraction_confirmed_empty =
                findings_extraction_confirmed_empty(session_dir, &findings_file);

            if findings_file.findings.is_empty()
                && (severity_counts_are_zero(&severity_counts) || extraction_confirmed_empty)
            {
                if let Some(artifact) = cross_check_json_for_blocking(
                    session_dir,
                    meta,
                    prose_signals.blocking_summary,
                )? {
                    return Ok(artifact);
                }
                if let Some(json_counts) =
                    json_severity_counts_if_present(session_dir, zero_severity_counts)?
                {
                    let decision = derive_decision_from_severity_counts(
                        &json_counts,
                        false,
                        None,
                        ReviewDecision::from_str(&meta.decision).ok(),
                        || Ok(prose_signals.blocking_summary),
                        || review_contains_prose_clean_conclusion(session_dir),
                        || review_contains_prose_fail_conclusion(session_dir),
                    )?;
                    return Ok(verdict_from_meta(meta, decision, json_counts));
                }
                if extraction_confirmed_empty {
                    let structured_counts =
                        severity_counts_for_findings_toml(&findings_file, zero_severity_counts);
                    return Ok(verdict_from_meta(
                        meta,
                        ReviewDecision::Pass,
                        structured_counts,
                    ));
                }
            }

            let decision = derive_decision_from_severity_counts(
                &severity_counts,
                findings_file.findings.is_empty(),
                None,
                ReviewDecision::from_str(&meta.decision).ok(),
                || Ok(prose_signals.blocking_summary),
                || review_contains_prose_clean_conclusion(session_dir),
                || review_contains_prose_fail_conclusion(session_dir),
            )?;
            return Ok(verdict_from_meta(meta, decision, severity_counts));
        }
    }

    if let Some(artifact) = load_review_artifact_from_output(session_dir)? {
        let severity_counts = reconcile_counts_with_prose(
            severity_counts_for_artifact(&artifact, zero_severity_counts),
            &prose_signals.severity_counts,
        );
        let decision = derive_decision_from_severity_counts(
            &severity_counts,
            artifact.findings.is_empty(),
            artifact.overall_risk.as_deref(),
            ReviewDecision::from_str(&meta.decision).ok(),
            || Ok(prose_signals.blocking_summary),
            || review_contains_prose_clean_conclusion(session_dir),
            || review_contains_prose_fail_conclusion(session_dir),
        )?;
        return Ok(verdict_from_meta(meta, decision, severity_counts));
    }

    if findings.is_empty()
        && review_contains_prose_clean_conclusion(session_dir)?
        && !prose_signals.has_failure_evidence()
    {
        return Ok(verdict_from_meta(
            meta,
            ReviewDecision::Pass,
            zero_severity_counts(),
        ));
    }

    if let Some(artifact) = infer_review_verdict_from_full_output(session_dir, meta)? {
        return Ok(artifact);
    }

    if let Some(severity_counts) = synthetic_empty_findings_counts {
        // Synthetic-empty findings.toml exhausted every artifact fallback (no
        // blocking JSON, no JSON artifact, no full.md verdict). The pre-#1675
        // behavior returned Pass unconditionally here — the SAME zero-evidence
        // false-PASS hole #1675 closed on the non-synthetic path, just reached
        // via the #1045-r3 synthetic fall-through. Route through the shared gate
        // so a Fail/Uncertain meta whose prose affirmatively concludes FAIL
        // fails closed even when the structured findings were unparseable. The
        // counts are guaranteed zero here (set only inside the
        // severity_counts_are_zero branch above), so the #1675 gate applies and
        // neutral-prose synthetic-empty results still resolve to Pass (#1349).
        let decision = derive_decision_from_severity_counts(
            &severity_counts,
            true,
            None,
            ReviewDecision::from_str(&meta.decision).ok(),
            || Ok(prose_signals.blocking_summary),
            || review_contains_prose_clean_conclusion(session_dir),
            || review_contains_prose_fail_conclusion(session_dir),
        )?;
        return Ok(verdict_from_meta(meta, decision, severity_counts));
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

fn findings_extraction_confirmed_empty(session_dir: &Path, findings_file: &FindingsFile) -> bool {
    findings_file.findings.is_empty()
        && session_dir
            .join("output")
            .join(super::findings_toml::FINDINGS_TOML_EXTRACTED_MARKER)
            .exists()
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
    prose_blocking_check: impl FnOnce() -> Result<bool, anyhow::Error>,
    prose_clean_check: impl FnOnce() -> Result<bool, anyhow::Error>,
    prose_fail_check: impl FnOnce() -> Result<bool, anyhow::Error>,
) -> Result<ReviewDecision, anyhow::Error> {
    // Blocking findings (critical/high/medium) always fail.
    if has_blocking_severity(severity_counts) {
        return Ok(ReviewDecision::Fail);
    }

    if meta_decision == Some(ReviewDecision::Skip) && !severity_counts_are_zero(severity_counts) {
        return Ok(ReviewDecision::Skip);
    }

    // Non-blocking findings (low only) → pass. Explicit low-only severity
    // counts beat summary wording; the summary heuristic only elevates
    // otherwise ambiguous zero-count output.
    if !severity_counts_are_zero(severity_counts) {
        // Only low-severity findings present — non-blocking.
        return Ok(ReviewDecision::Pass);
    }
    if prose_blocking_check()? {
        return Ok(ReviewDecision::Fail);
    }

    // Zero severity counts but non-empty findings list → fail-closed (parsing anomaly).
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
        // #1675: a Fail/Uncertain meta whose prose AFFIRMATIVELY concludes FAIL, but whose
        // structured findings failed to emit, is a real failure with lost evidence — fail
        // closed. Gated on affirmative prose FAIL (NOT "prose not clean") so #1349's
        // neutral-prose noise still resolves to the zero-evidence Pass below.
        if matches!(
            meta_decision,
            Some(ReviewDecision::Fail | ReviewDecision::Uncertain)
        ) && prose_fail_check()?
        {
            return Ok(ReviewDecision::Fail);
        }
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
        review_mode: None,
        prior_round_refs,
        diff_size: None,
        large_diff_warning: false,
        large_diff_warning_threshold: None,
        large_diff_warning_changed_lines: None,
        no_provider_launch: None,
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

pub(in crate::review_cmd) use clean_detection::is_review_output_empty;

#[cfg(test)]
#[path = "review_cmd_output_fix_reuse_tests.rs"]
mod fix_reuse_tests;
#[cfg(test)]
#[path = "review_cmd_output_terminal_error_reason_tests.rs"]
mod terminal_error_reason_tests;
#[cfg(test)]
#[path = "review_cmd_output_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "review_cmd_output_verdict_tail_tests.rs"]
mod verdict_tail_tests;
