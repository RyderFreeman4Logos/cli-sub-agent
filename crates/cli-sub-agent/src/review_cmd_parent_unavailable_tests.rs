use super::{
    aggregate_unavailable_reviewer_reasons, patch_parent_review_verdict_unavailable_reasons,
    write_parent_review_verdict,
};
use crate::review_cmd::diff_size::ReviewDiffReport;
use crate::review_cmd::output::ReviewerOutcome;
use crate::review_consensus::UNAVAILABLE;
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::ReviewVerdictArtifact;
use std::fs;
use tempfile::tempdir;

fn unavailable_outcome(index: usize, reason: &str) -> ReviewerOutcome {
    ReviewerOutcome {
        reviewer_index: index,
        tool: ToolName::Codex,
        session_id: format!("01TESTREVIEWER{index:012}"),
        output: format!("Review unavailable: {reason}\n"),
        exit_code: 1,
        verdict: UNAVAILABLE,
        diagnostic: Some(reason.to_string()),
    }
}

#[test]
fn issue_2911_all_unavailable_parent_carries_actionable_reason() {
    let reason = "host_memory_admission: reviewer provider did not launch; no alternate configured reviewer candidate was available";
    let outcomes = vec![
        unavailable_outcome(0, reason),
        unavailable_outcome(1, reason),
        unavailable_outcome(2, reason),
        unavailable_outcome(3, reason),
    ];

    let (primary, failure_reason) = aggregate_unavailable_reviewer_reasons(&outcomes);
    assert_eq!(primary.as_deref(), Some(reason));
    assert_eq!(
        failure_reason.as_deref(),
        Some(
            "codex=host_memory_admission: reviewer provider did not launch; no alternate configured reviewer candidate was available"
        ),
        "identical reviewer reasons must de-dupe rather than repeat four opaque labels"
    );

    let temp = tempdir().expect("tempdir");
    write_parent_review_verdict(
        temp.path(),
        "01PARENTSESSION000000000000",
        &[],
        ReviewDecision::Unavailable,
        UNAVAILABLE,
        ReviewDiffReport {
            diff_size: None,
            large_diff_warning: None,
        },
        None,
    )
    .expect("write parent verdict");
    patch_parent_review_verdict_unavailable_reasons(
        temp.path(),
        primary.as_deref(),
        failure_reason.as_deref(),
    )
    .expect("patch unavailable reasons");

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("read verdict"),
    )
    .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert_eq!(artifact.verdict_legacy, UNAVAILABLE);
    assert_eq!(artifact.primary_failure.as_deref(), Some(reason));
    assert!(
        artifact
            .failure_reason
            .as_deref()
            .is_some_and(|text| text.contains("host_memory_admission")),
        "parent verdict must carry actionable unavailable reason: {:?}",
        artifact.failure_reason
    );
}

#[test]
fn issue_2911_unavailable_reason_relativizes_absolute_paths() {
    let absolute = "/home/alice/private-project/config.toml";
    let reason = format!("provider failed reading {absolute} while launching reviewer");
    let outcomes = vec![unavailable_outcome(0, &reason)];

    let (primary, failure_reason) = aggregate_unavailable_reviewer_reasons(&outcomes);
    let primary = primary.expect("primary reason");
    let failure_reason = failure_reason.expect("failure reason");

    assert!(
        !primary.contains("/home/alice"),
        "primary must not leak absolute home path: {primary}"
    );
    assert!(
        !failure_reason.contains("/home/alice"),
        "failure_reason must not leak absolute home path: {failure_reason}"
    );
    assert!(
        primary.contains("private-project/config.toml"),
        "primary should keep a relative path for actionability: {primary}"
    );
    assert!(
        primary.contains("provider failed reading"),
        "primary should keep the actionable diagnostic: {primary}"
    );
}
