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
        let decision = if artifact.severity_counts.values().any(|count| *count > 0) {
            ReviewDecision::Fail
        } else {
            artifact.decision
        };
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
        if decision == ReviewDecision::Pass {
            if crate::session_observability::review_sidecars_allow_clean_pass(session_dir)
                .unwrap_or(false)
            {
                return Some("PASS".to_string());
            }
            return Some("UNAVAILABLE".to_string());
        }
        if decision == ReviewDecision::Unavailable
            && let Some(primary_failure) = artifact.primary_failure.as_deref()
            && !primary_failure.trim().is_empty()
        {
            let redacted =
                crate::review_failure_context::sanitize_review_surface_text(primary_failure.trim());
            let compacted = super::compact_wait_summary_text(&redacted);
            let label = compacted.unwrap_or_else(|| redacted.clone());
            return Some(format!("UNAVAILABLE ({label})"));
        }
        let normalized = normalize_review_verdict_label(decision.as_str(), result);
        if matches!(
            decision,
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
        if matches!(meta.decision.parse(), Ok(ReviewDecision::Pass)) {
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
    if artifact.decision == ReviewDecision::Pass
        && !artifact.severity_counts.values().any(|count| *count > 0)
    {
        return None;
    }
    let meta = read_review_meta_for_label(session_dir);
    let reason = review_failure_reason_label(meta.as_ref(), &artifact)?;
    let decision = if artifact.severity_counts.values().any(|count| *count > 0) {
        ReviewDecision::Fail
    } else {
        artifact.decision
    };
    let label =
        normalize_review_verdict_label(decision.as_str(), &csa_session::SessionResult::default());
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
    crate::session_observability::review_sidecars_allow_clean_pass(session_dir).unwrap_or(false)
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
    super::compact_wait_summary_text(
        &crate::review_failure_context::sanitize_review_surface_text(reason),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn terminal_result(status: &str, exit_code: i32, summary: &str) -> csa_session::SessionResult {
        let now = Utc::now();
        csa_session::SessionResult {
            status: status.to_string(),
            exit_code,
            summary: summary.to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            ..Default::default()
        }
    }

    #[test]
    fn issue_2825_review_label_keeps_structured_failure_over_exact_pass_prose() {
        let temp = tempfile::tempdir().expect("tempdir");
        csa_session::persist_structured_output(
            temp.path(),
            "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n",
        )
        .expect("persist pass summary");
        let mut artifact = ReviewVerdictArtifact::from_parts(
            "01TEST2601LABELPASS".to_string(),
            ReviewDecision::Fail,
            "HAS_ISSUES",
            &[],
            Vec::new(),
        );
        artifact
            .severity_counts
            .insert(csa_session::Severity::High, 1);
        csa_session::write_review_verdict(temp.path(), &artifact).expect("write verdict");
        csa_session::write_findings_toml(
            temp.path(),
            &csa_session::FindingsFile {
                findings: vec![csa_session::ReviewFinding {
                    id: "prose-001".to_string(),
                    severity: csa_session::Severity::High,
                    file_ranges: Vec::new(),
                    is_regression_of_commit: None,
                    suggested_test_scenario: None,
                    description: "P1 positive evidence was misclassified".to_string(),
                }],
            },
        )
        .expect("write findings");

        let label = read_review_verdict_label(temp.path(), &terminal_result("failure", 1, "PASS"));

        assert_eq!(label.as_deref(), Some("FAIL"));
    }

    #[test]
    fn issue_2601_review_label_exact_pass_summary_does_not_hide_hard_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        csa_session::persist_structured_output(
            temp.path(),
            "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n",
        )
        .expect("persist pass summary");
        let mut artifact = ReviewVerdictArtifact::from_parts(
            "01TEST2601LABELFAIL".to_string(),
            ReviewDecision::Fail,
            "HAS_ISSUES",
            &[],
            Vec::new(),
        );
        artifact.failure_reason = Some("provider crashed before final review".to_string());
        csa_session::write_review_verdict(temp.path(), &artifact).expect("write verdict");

        let label = read_review_verdict_label(temp.path(), &terminal_result("failure", 1, "PASS"));

        assert!(
            label
                .as_deref()
                .is_some_and(|label| label.starts_with("FAIL"))
        );
    }
}
