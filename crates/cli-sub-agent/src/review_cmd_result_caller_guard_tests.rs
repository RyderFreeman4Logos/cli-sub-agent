use super::tests::outcome;
use super::*;
use std::path::Path;

#[test]
fn resolve_single_review_result_rejects_caller_guard_as_review_result() {
    let mut result = outcome("</csa-caller-sa-guard>", 1);
    result.execution.execution.summary = "</csa-caller-sa-guard>".to_string();
    result.failure_reason = Some("reviewer failed to start: executable not found".to_string());

    let resolved =
        resolve_single_review_result(&result, ToolName::Codex, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert_eq!(resolved.effective_exit_code, 1);
    assert!(!resolved.sanitized.contains("csa-caller-sa-guard"));
    assert!(resolved.sanitized.contains("reviewer failed to start"));
}

#[test]
fn build_reviewer_outcome_rejects_caller_guard_as_review_result() {
    let mut result = outcome("<csa-caller-sa-guard>\n</csa-caller-sa-guard>", 1);
    result.primary_failure =
        Some("reviewer startup failed: process exited before provider".to_string());

    let reviewer = build_reviewer_outcome(0, ToolName::Codex, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
    assert!(!reviewer.output.contains("csa-caller-sa-guard"));
    assert!(
        reviewer
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("reviewer startup failed"))
    );
}

#[test]
fn resolve_single_review_result_fails_closed_for_truncated_caller_guard() {
    let result = outcome("<csa-caller-sa-guard:compact tier=codex", 1);
    let resolved =
        resolve_single_review_result(&result, ToolName::Codex, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert!(!resolved.sanitized.contains("csa-caller-sa-guard"));
    assert!(
        resolved
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("caller guard"))
    );
}

#[test]
fn literal_caller_guard_text_in_provider_output_remains_data() {
    let output = "<csa-caller-sa-guard>\n<!-- CSA:SECTION:summary -->\nPASS — documented literal </csa-caller-sa-guard>\n<!-- CSA:SECTION:summary:END -->";
    let resolved = resolve_single_review_result(
        &outcome(output, 0),
        ToolName::Codex,
        "uncommitted",
        Path::new("."),
    );

    assert_eq!(resolved.decision, ReviewDecision::Pass);
    assert!(resolved.sanitized.contains("</csa-caller-sa-guard>"));
}
