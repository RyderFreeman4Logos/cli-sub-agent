use std::path::Path;

use csa_core::types::ReviewDecision;
use csa_session::{ReviewSessionMeta, ReviewVerdictArtifact};

pub(super) fn read_review_verdict_label(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<String> {
    let _ =
        crate::review_cmd::output::consistency::repair_clean_empty_fail_review_verdict(session_dir);
    let summary_requires_failed_gate =
        crate::session_observability::human_review_summary_requires_failed_gate(
            session_dir,
            &result.summary,
        );
    if let Some(artifact) = read_review_verdict_artifact(session_dir) {
        let meta = read_review_meta_for_label(session_dir);
        if let Some(label) = meta
            .as_ref()
            .and_then(|meta| format_fix_loop_noop_label(meta.failure_reason.as_deref()))
            .or_else(|| format_fix_loop_noop_label(artifact.failure_reason.as_deref()))
        {
            return Some(label);
        }
        if summary_requires_failed_gate {
            return Some("FAIL".to_string());
        }
        if artifact.decision == ReviewDecision::Pass {
            if meta
                .as_ref()
                .is_some_and(|meta| meta.accepts_clean_review_verdict(artifact.decision))
            {
                return Some("PASS".to_string());
            }
            if !wait_result_allows_pass_verdict(result) {
                return Some("UNAVAILABLE".to_string());
            }
            if meta.as_ref().is_some_and(|meta| {
                meta.requires_fail_closed_verdict() || !meta.fix_clean_converged()
            }) {
                return Some("UNAVAILABLE".to_string());
            }
            return Some("PASS".to_string());
        }
        if artifact.decision == ReviewDecision::Unavailable
            && let Some(primary_failure) = artifact.primary_failure.as_deref()
            && !primary_failure.trim().is_empty()
        {
            let redacted = csa_session::redact_text_content(primary_failure.trim());
            let compacted = super::compact_wait_summary_text(&redacted);
            let label = compacted.unwrap_or_else(|| redacted.clone());
            return Some(format!("UNAVAILABLE ({label})"));
        }
        let normalized = normalize_review_verdict_label(artifact.decision.as_str(), result);
        if matches!(
            artifact.decision,
            ReviewDecision::Fail | ReviewDecision::Uncertain | ReviewDecision::Unavailable
        ) && let Some(reason) = review_failure_reason_label(meta.as_ref(), &artifact)
        {
            return Some(format!("{normalized} ({reason})"));
        }
        return Some(normalized);
    }

    let meta_path = session_dir.join("review_meta.json");
    if meta_path.is_file()
        && let Ok(raw) = std::fs::read_to_string(&meta_path)
        && let Ok(meta) = serde_json::from_str::<ReviewSessionMeta>(&raw)
    {
        if let Some(label) = format_fix_loop_noop_label(meta.failure_reason.as_deref()) {
            return Some(label);
        }
        if summary_requires_failed_gate {
            return Some("FAIL".to_string());
        }
        if meta.fix_attempted && !meta.fix_clean_converged() {
            return Some("UNAVAILABLE".to_string());
        }
        let normalized = normalize_review_verdict_label(&meta.decision, result);
        if matches!(
            meta.decision.parse::<ReviewDecision>(),
            Ok(ReviewDecision::Fail | ReviewDecision::Uncertain | ReviewDecision::Unavailable)
        ) && let Some(reason) = review_meta_failure_reason_label(&meta)
        {
            return Some(format!("{normalized} ({reason})"));
        }
        return Some(normalized);
    }

    if summary_requires_failed_gate {
        return Some("FAIL".to_string());
    }

    None
}

pub(super) fn review_failure_summary_override(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<String> {
    let _ =
        crate::review_cmd::output::consistency::repair_clean_empty_fail_review_verdict(session_dir);
    if review_verdict_artifact_is_pass(session_dir) {
        return None;
    }
    let human_summary =
        crate::session_summary_text::human_session_summary(session_dir, &result.summary)
            .and_then(|text| super::compact_wait_summary_text(&text));
    if !human_summary
        .as_deref()
        .is_some_and(summary_looks_clean_without_blockers)
    {
        return None;
    }
    let artifact = read_review_verdict_artifact(session_dir)?;
    if artifact.decision == ReviewDecision::Pass {
        return None;
    }
    let meta = read_review_meta_for_label(session_dir);
    let reason = review_failure_reason_label(meta.as_ref(), &artifact)?;
    let label = normalize_review_verdict_label(
        artifact.decision.as_str(),
        &csa_session::SessionResult::default(),
    );
    Some(format!("Review {label}: {reason}"))
}

pub(super) fn review_pass_summary_override(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<String> {
    let _ =
        crate::review_cmd::output::consistency::repair_clean_empty_fail_review_verdict(session_dir);
    if !review_verdict_artifact_is_pass(session_dir) {
        return None;
    }
    let human_summary =
        crate::session_summary_text::human_session_summary(session_dir, &result.summary)
            .and_then(|text| super::compact_wait_summary_text(&text));
    if human_summary
        .as_deref()
        .is_some_and(summary_looks_clean_without_blockers)
    {
        return human_summary;
    }
    super::compact_wait_summary_text(&result.summary)
        .filter(|summary| summary_looks_clean_without_blockers(summary))
}

pub(super) fn read_review_verdict_artifact(session_dir: &Path) -> Option<ReviewVerdictArtifact> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&verdict_path).ok()?;
    serde_json::from_str::<ReviewVerdictArtifact>(&raw).ok()
}

fn review_verdict_artifact_is_pass(session_dir: &Path) -> bool {
    read_review_verdict_artifact(session_dir)
        .is_some_and(|artifact| artifact.decision == ReviewDecision::Pass)
}

fn format_fix_loop_noop_label(reason: Option<&str>) -> Option<String> {
    let reason = reason?.strip_prefix("fix_loop_noop:")?.trim();
    if reason.is_empty() {
        return None;
    }
    Some(format!("FIX-LOOP-NO-OP ({reason})"))
}

fn read_review_meta_for_label(session_dir: &Path) -> Option<ReviewSessionMeta> {
    let meta_path = session_dir.join("review_meta.json");
    if !meta_path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str::<ReviewSessionMeta>(&raw).ok()
}

fn review_failure_reason_label(
    meta: Option<&ReviewSessionMeta>,
    artifact: &ReviewVerdictArtifact,
) -> Option<String> {
    let candidates = if artifact.decision == ReviewDecision::Unavailable {
        [
            meta.and_then(|meta| meta.primary_failure.as_deref()),
            artifact.primary_failure.as_deref(),
            meta.and_then(|meta| meta.status_reason.as_deref()),
            meta.and_then(|meta| meta.failure_reason.as_deref()),
            artifact.failure_reason.as_deref(),
        ]
    } else {
        [
            meta.and_then(|meta| meta.status_reason.as_deref()),
            meta.and_then(|meta| meta.failure_reason.as_deref()),
            artifact.failure_reason.as_deref(),
            meta.and_then(|meta| meta.primary_failure.as_deref()),
            artifact.primary_failure.as_deref(),
        ]
    };
    candidates
        .into_iter()
        .flatten()
        .find_map(compact_review_failure_reason)
}

fn review_meta_failure_reason_label(meta: &ReviewSessionMeta) -> Option<String> {
    [
        meta.status_reason.as_deref(),
        meta.failure_reason.as_deref(),
        meta.primary_failure.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(compact_review_failure_reason)
}

fn compact_review_failure_reason(reason: &str) -> Option<String> {
    super::compact_wait_summary_text(&csa_session::redact_text_content(reason))
}

fn summary_looks_clean_without_blockers(summary: &str) -> bool {
    let lower = summary.to_ascii_lowercase();
    [
        "no blocking",
        "no blockers",
        "no actionable findings",
        "no issues found",
        "no issues were found",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        || crate::review_cmd::detect_bounded_clean_verdict_token(summary)
}

fn wait_result_allows_pass_verdict(result: &csa_session::SessionResult) -> bool {
    result.exit_code == 0 && result.status.trim().eq_ignore_ascii_case("success")
}

fn normalize_review_verdict_label(value: &str, result: &csa_session::SessionResult) -> String {
    match value.trim().to_ascii_uppercase().as_str() {
        "PASS" | "CLEAN" if !wait_result_allows_pass_verdict(result) => "UNAVAILABLE".to_string(),
        "PASS" | "CLEAN" => "PASS".to_string(),
        "FAIL" | "FAILED" | "HAS_ISSUES" => "FAIL".to_string(),
        other => other.to_string(),
    }
}
