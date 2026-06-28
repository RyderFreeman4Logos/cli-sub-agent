use super::*;
use chrono::Utc;
use csa_core::types::ReviewDecision;
use std::{collections::BTreeMap, path::Path};

fn write_review_sidecars(
    session_dir: &Path,
    session_id: &str,
    decision: ReviewDecision,
    legacy_verdict: &str,
    failure_reason: Option<&str>,
) {
    std::fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    csa_session::write_review_meta(
        session_dir,
        &csa_session::ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: "deadbeef".to_string(),
            decision: decision.as_str().to_string(),
            verdict: legacy_verdict.to_string(),
            review_mode: None,
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: failure_reason.map(str::to_string),
            tool: "codex".to_string(),
            scope: "range:main...HEAD".to_string(),
            exit_code: if decision == ReviewDecision::Pass {
                0
            } else {
                1
            },
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: Utc::now(),
            diff_fingerprint: None,
            fix_convergence: None,
        },
    )
    .expect("write review meta");
    let mut artifact = csa_session::ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        decision,
        legacy_verdict,
        &[],
        Vec::new(),
    );
    artifact.failure_reason = failure_reason.map(str::to_string);
    csa_session::write_review_verdict(session_dir, &artifact).expect("write review verdict");
}

fn failure_result(summary: &str) -> csa_session::SessionResult {
    let now = Utc::now();
    csa_session::SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        ..Default::default()
    }
}

fn success_result(summary: &str) -> csa_session::SessionResult {
    let now = Utc::now();
    csa_session::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        ..Default::default()
    }
}

#[test]
fn issue_2425_wait_summary_renders_clean_recovered_review_as_pass() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTWAIT2425CLEAN";
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nVerdict: PASS\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");
    write_review_sidecars(
        temp.path(),
        session_id,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        None,
    );

    let summary = render_wait_result_summary(
        temp.path(),
        session_id,
        &failure_result("No blocking findings in `main...HEAD`."),
    );

    assert!(summary.contains("Status: success"), "{summary}");
    assert!(summary.contains("Review verdict: PASS"), "{summary}");
    assert!(
        !summary.contains("Review verdict: UNAVAILABLE"),
        "{summary}"
    );
    assert!(!summary.contains("Review verdict: UNCERTAIN"), "{summary}");

    let meta: csa_session::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.failure_reason, None);
}

#[test]
fn issue_2425_wait_summary_recovers_live_empty_findings_fail_placeholder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTWAIT2425LIVE";
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nNo blocking findings found for `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nReviewed `main...HEAD`.\n\nFindings: none.\n\nKey evidence:\n- clean recovery has no hard failure evidence or structured findings.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist live clean review sections");
    write_review_sidecars(
        temp.path(),
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        Some("fail_verdict_empty_findings_artifact"),
    );
    let verdict_path = temp.path().join("output").join("review-verdict.json");
    let raw = std::fs::read_to_string(&verdict_path).expect("read review verdict");
    let mut artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&raw).expect("parse review verdict");
    artifact.severity_counts = BTreeMap::from([
        (csa_session::Severity::Critical, 0),
        (csa_session::Severity::High, 0),
        (csa_session::Severity::Medium, 1),
        (csa_session::Severity::Low, 0),
    ]);
    csa_session::write_review_verdict(temp.path(), &artifact).expect("rewrite review verdict");
    std::fs::write(
        temp.path().join("output").join("suggestion.toml"),
        format!(
            "[suggestion]\naction = \"confirm_then_fix_finding\"\nsession_id = {session_id:?}\nrequires_confirmation = true\n"
        ),
    )
    .expect("write synthetic fix suggestion");

    let summary = render_wait_result_summary(
        temp.path(),
        session_id,
        &success_result("No blocking findings found for `main...HEAD`."),
    );

    assert!(summary.contains("Status: success"), "{summary}");
    assert!(summary.contains("Review verdict: PASS"), "{summary}");
    assert!(!summary.contains("Review verdict: FAIL"), "{summary}");
    assert!(
        summary.contains("Summary: No blocking findings found for `main...HEAD`."),
        "{summary}"
    );
    assert!(!summary.contains("Summary: Review FAIL"), "{summary}");

    let meta: csa_session::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.failure_reason, None);
}

#[test]
fn issue_2425_observability_refresh_flips_repaired_empty_findings_result_to_success() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTWAIT2425REFRESH";
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nNo blocking findings found for `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nReviewed `main...HEAD`.\n\nFindings: none.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist live clean review sections");
    write_review_sidecars(
        temp.path(),
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        Some("fail_verdict_empty_findings_artifact"),
    );
    let verdict_path = temp.path().join("output").join("review-verdict.json");
    let raw = std::fs::read_to_string(&verdict_path).expect("read review verdict");
    let mut artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&raw).expect("parse review verdict");
    artifact.severity_counts = BTreeMap::from([
        (csa_session::Severity::Critical, 0),
        (csa_session::Severity::High, 0),
        (csa_session::Severity::Medium, 1),
        (csa_session::Severity::Low, 0),
    ]);
    csa_session::write_review_verdict(temp.path(), &artifact).expect("rewrite review verdict");
    std::fs::write(
        temp.path().join("output").join("suggestion.toml"),
        format!(
            "[suggestion]\naction = \"confirm_then_fix_finding\"\nsession_id = {session_id:?}\nrequires_confirmation = true\n"
        ),
    )
    .expect("write synthetic fix suggestion");
    let result = failure_result(
        r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":10}}"#,
    );
    std::fs::write(
        temp.path().join(csa_session::result::RESULT_FILE_NAME),
        toml::to_string_pretty(&result).expect("serialize result"),
    )
    .expect("write failure result");

    let repaired = crate::session_observability::refresh_and_repair_result_from_dir(temp.path())
        .expect("refresh result")
        .expect("result exists");

    assert_eq!(repaired.status, "success");
    assert_eq!(repaired.exit_code, 0);
    let persisted: csa_session::SessionResult = toml::from_str(
        &std::fs::read_to_string(temp.path().join(csa_session::result::RESULT_FILE_NAME))
            .expect("read persisted result"),
    )
    .expect("parse persisted result");
    assert_eq!(persisted.status, "success");
    assert_eq!(persisted.exit_code, 0);
}

#[test]
fn issue_2425_wait_summary_keeps_mixed_uncertain_no_blocker_review_uncertain() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTWAIT2425MIXED";
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nuncertain: no blocking findings, but insufficient context\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking findings were identified, but the review cannot conclude PASS.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist mixed uncertain review sections");
    write_review_sidecars(
        temp.path(),
        session_id,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        None,
    );

    let summary = render_wait_result_summary(
        temp.path(),
        session_id,
        &failure_result("uncertain: no blocking findings, but insufficient context"),
    );

    assert!(summary.contains("Review verdict: UNCERTAIN"), "{summary}");
    assert!(!summary.contains("Review verdict: PASS"), "{summary}");
    assert!(
        summary.contains("Summary: uncertain: no blocking findings, but insufficient context"),
        "{summary}"
    );

    let meta: csa_session::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert_eq!(meta.decision, ReviewDecision::Uncertain.as_str());
    assert_eq!(meta.verdict, "UNCERTAIN");
}

#[test]
fn issue_2425_wait_summary_replaces_no_blocker_summary_with_uncertain_reason() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTWAIT2425CRASH";
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist misleading clean review section");
    write_review_sidecars(
        temp.path(),
        session_id,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        Some("reviewer process crashed before artifact finalization"),
    );

    let summary = render_wait_result_summary(
        temp.path(),
        session_id,
        &failure_result("No blocking findings in `main...HEAD`."),
    );

    assert!(
        summary.contains(
            "Review verdict: UNCERTAIN (reviewer process crashed before artifact finalization)"
        ),
        "{summary}"
    );
    assert!(
        summary.contains(
            "Summary: Review UNCERTAIN: reviewer process crashed before artifact finalization"
        ),
        "{summary}"
    );
    assert!(
        !summary.contains("Summary: No blocking findings"),
        "uncertain infrastructure reason must replace contradictory clean prose: {summary}"
    );
}

#[test]
fn issue_2425_wait_summary_exposes_unavailable_quota_reason() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTWAIT2425QUOTA";
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist misleading clean review section");
    write_review_sidecars(
        temp.path(),
        session_id,
        ReviewDecision::Unavailable,
        "UNAVAILABLE",
        Some("provider quota exceeded before review completed"),
    );

    let summary = render_wait_result_summary(
        temp.path(),
        session_id,
        &failure_result("No blocking findings in `main...HEAD`."),
    );

    assert!(
        summary.contains(
            "Review verdict: UNAVAILABLE (provider quota exceeded before review completed)"
        ),
        "{summary}"
    );
    assert!(
        summary.contains(
            "Summary: Review UNAVAILABLE: provider quota exceeded before review completed"
        ),
        "{summary}"
    );
    assert!(
        !summary.contains("Summary: No blocking findings"),
        "{summary}"
    );
}

#[test]
fn issue_2425_wait_summary_keeps_blocking_counts_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTWAIT2425COUNTS";
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist misleading clean review section");
    write_review_sidecars(
        temp.path(),
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        None,
    );
    let verdict_path = temp.path().join("output").join("review-verdict.json");
    let raw = std::fs::read_to_string(&verdict_path).expect("read review verdict");
    let mut artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&raw).expect("parse review verdict");
    artifact.severity_counts = BTreeMap::from([
        (csa_session::Severity::Critical, 0),
        (csa_session::Severity::High, 1),
        (csa_session::Severity::Medium, 0),
        (csa_session::Severity::Low, 0),
    ]);
    csa_session::write_review_verdict(temp.path(), &artifact).expect("rewrite review verdict");

    let summary = render_wait_result_summary(
        temp.path(),
        session_id,
        &failure_result("No blocking findings in `main...HEAD`."),
    );

    assert!(summary.contains("Review verdict: FAIL"), "{summary}");
    assert!(!summary.contains("Review verdict: PASS"), "{summary}");
}
