use super::{
    MultiReviewerConsensusArtifacts, parent_artifact_for_decision, parent_consensus_review_meta,
    parent_review_decision, write_multi_reviewer_consensus_artifacts,
    write_multi_reviewer_parent_artifacts, write_standalone_consensus_review_artifacts,
};
use crate::review_consensus::UNAVAILABLE;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use csa_core::env::CSA_SESSION_DIR_ENV_KEY;
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::review_artifact::{
    Finding, FindingsFile, ReviewArtifact, ReviewVerdictArtifact, Severity, SeveritySummary,
};
use proptest::prelude::*;
use std::fs;
use tempfile::tempdir;

#[derive(Clone, Copy, Debug)]
enum ReviewerState {
    Pass,
    Fail,
    Unavailable,
}

fn blocking_finding(fid: impl Into<String>) -> Finding {
    let fid = fid.into();
    Finding {
        severity: Severity::High,
        fid: fid.clone(),
        file: "src/lib.rs".to_string(),
        line: Some(7),
        rule_id: format!("rule.review.{fid}"),
        summary: "blocking finding must propagate".to_string(),
        engine: "reviewer".to_string(),
    }
}

fn write_parent_reviewer_artifact(
    session_dir: &std::path::Path,
    reviewer_index: usize,
    finding: Finding,
) {
    let reviewer_dir = session_dir.join(format!("reviewer-{}", reviewer_index + 1));
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![finding];
    let artifact = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: format!("01TESTREVIEWER{reviewer_index:012}"),
        timestamp: chrono::Utc::now(),
    };
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("artifact should serialize"),
    )
    .expect("review artifact should be written");
}

#[test]
fn write_multi_reviewer_parent_artifacts_writes_output_sidecars() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![Finding {
        severity: Severity::High,
        fid: "FID-1".to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(7),
        rule_id: "rule.review.parent-sidecars".to_string(),
        summary: "parent sidecar finding".to_string(),
        engine: "reviewer".to_string(),
    }];
    let artifact = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: "01CHILDSESSION0000000000000".to_string(),
        timestamp: chrono::Utc::now(),
    };
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    let outcomes = vec![
        super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: "01CHILDSESSION0000000000000".to_string(),
            output: "Reviewer details".to_string(),
            exit_code: 1,
            verdict: crate::review_consensus::HAS_ISSUES,
            diagnostic: None,
        },
        super::super::output::ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::GeminiCli,
            session_id: "reviewer-2-unavailable".to_string(),
            output: "Review unavailable: reviewer timed out after 1800s\n".to_string(),
            exit_code: 1,
            verdict: UNAVAILABLE,
            diagnostic: Some("reviewer timed out after 1800s".to_string()),
        },
    ];

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        2,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        None,
    )
    .expect("parent artifacts should be produced");

    let output_dir = temp.path().join("output");
    let findings_toml: FindingsFile = toml::from_str(
        &fs::read_to_string(output_dir.join("findings.toml")).expect("findings.toml should exist"),
    )
    .expect("findings.toml should parse");
    assert_eq!(findings_toml.findings.len(), 1);
    assert_eq!(findings_toml.findings[0].id, "FID-1");

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(output_dir.join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts[&Severity::High], 1);
    assert!(
        fs::read_to_string(output_dir.join("summary.md"))
            .expect("summary should exist")
            .contains("reviewer 2 (gemini-cli) => UNAVAILABLE")
    );
    assert!(
        fs::read_to_string(output_dir.join("details.md"))
            .expect("details should exist")
            .contains("Review unavailable: reviewer timed out")
    );
}

#[test]
fn write_multi_reviewer_parent_artifacts_preserves_blocking_findings_on_clean_consensus() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![Finding {
        severity: Severity::High,
        fid: "CLEAN-BLOCKING-FID".to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(13),
        rule_id: "rule.review.clean-consensus-blocking".to_string(),
        summary: "blocking finding must not disappear".to_string(),
        engine: "reviewer".to_string(),
    }];
    let artifact = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: "01CHILDSESSION0000000000000".to_string(),
        timestamp: chrono::Utc::now(),
    };
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: "01CHILDSESSION0000000000000".to_string(),
        output: "Reviewer reported a blocking artifact.".to_string(),
        exit_code: 0,
        verdict: crate::review_consensus::CLEAN,
        diagnostic: None,
    }];

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        1,
        &outcomes,
        crate::review_consensus::CLEAN,
        false,
        None,
    )
    .expect("parent artifacts should be produced");

    let output_dir = temp.path().join("output");
    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(output_dir.join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::HAS_ISSUES);
    assert_eq!(verdict.severity_counts[&Severity::High], 1);

    let findings_toml: FindingsFile = toml::from_str(
        &fs::read_to_string(output_dir.join("findings.toml")).expect("findings.toml should exist"),
    )
    .expect("findings.toml should parse");
    assert_eq!(findings_toml.findings.len(), 1);
    assert_eq!(findings_toml.findings[0].id, "CLEAN-BLOCKING-FID");
}

#[test]
fn write_multi_reviewer_parent_artifacts_accepts_reviewer_contract_artifact() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![Finding {
        severity: Severity::High,
        fid: "FID-1".to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(7),
        rule_id: "rule.review.parent-sidecars".to_string(),
        summary: "parent sidecar finding".to_string(),
        engine: "reviewer".to_string(),
    }];
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "verdict": "FAIL",
            "findings": findings,
            "summary": "High-severity finding present"
        }))
        .expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: "01CHILDSESSION0000000000000".to_string(),
        output: "Reviewer details".to_string(),
        exit_code: 1,
        verdict: crate::review_consensus::HAS_ISSUES,
        diagnostic: None,
    }];

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        1,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        None,
    )
    .expect("parent artifacts should be produced");

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts[&Severity::High], 1);
}

#[test]
fn write_multi_reviewer_parent_artifacts_reads_child_session_reviewer_artifacts() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project dir should be created");
    let parent_dir = temp.path().join("parent-session");
    fs::create_dir_all(&parent_dir).expect("parent session dir should be created");
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &parent_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let child = csa_session::create_session_fresh(
        &project,
        Some("review[1]: range:main...HEAD"),
        None,
        None,
    )
    .expect("child reviewer session should be created");
    let child_id = child.meta_session_id.clone();
    let child_dir = csa_session::get_session_dir(&project, &child_id).unwrap();
    let reviewer_dir = child_dir.join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![Finding {
        severity: Severity::High,
        fid: "CHILD-FID".to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(11),
        rule_id: "rule.child-artifact".to_string(),
        summary: "child artifact finding".to_string(),
        engine: "reviewer".to_string(),
    }];
    let artifact = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: child_id.clone(),
        timestamp: chrono::Utc::now(),
    };
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: child_id,
        output: "Reviewer found an issue.".to_string(),
        exit_code: 1,
        verdict: crate::review_consensus::HAS_ISSUES,
        diagnostic: None,
    }];

    write_multi_reviewer_parent_artifacts(
        &project,
        1,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        None,
    )
    .expect("parent artifacts should be produced");

    let parent_findings: FindingsFile = toml::from_str(
        &fs::read_to_string(parent_dir.join("output").join("findings.toml"))
            .expect("findings.toml should exist"),
    )
    .expect("findings.toml should parse");
    assert_eq!(parent_findings.findings.len(), 1);
    assert_eq!(parent_findings.findings[0].id, "CHILD-FID");
}

#[test]
fn consensus_artifacts_copy_child_only_findings_into_parent_session_outputs() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project dir should be created");
    let parent_dir = temp.path().join("parent-session");
    fs::create_dir_all(&parent_dir).expect("parent session dir should be created");
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &parent_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let child = csa_session::create_session_fresh(
        &project,
        Some("review[1]: range:main...HEAD"),
        None,
        None,
    )
    .expect("child reviewer session should be created");
    let child_id = child.meta_session_id.clone();
    let child_dir = csa_session::get_session_dir(&project, &child_id).unwrap();
    write_parent_reviewer_artifact(&child_dir, 0, blocking_finding("CHILD-ONLY-FID"));

    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: child_id.clone(),
        output: "Reviewer found an issue.".to_string(),
        exit_code: 1,
        verdict: crate::review_consensus::HAS_ISSUES,
        diagnostic: None,
    }];
    let ctx = MultiReviewerConsensusArtifacts {
        project_root: &project,
        reviewers: 1,
        outcomes: &outcomes,
        final_verdict: crate::review_consensus::HAS_ISSUES,
        all_reviewers_unavailable: false,
        head_sha: "abcdef1234567890",
        scope: "range:main...HEAD",
        review_iterations: 1,
        diff_fingerprint: Some("sha256:test".to_string()),
    };

    write_multi_reviewer_consensus_artifacts(ctx)
        .expect("parent consensus artifacts should be produced");

    let parent_findings: FindingsFile = toml::from_str(
        &fs::read_to_string(parent_dir.join("output").join("findings.toml"))
            .expect("parent findings.toml should exist"),
    )
    .expect("parent findings.toml should parse");
    assert_eq!(parent_findings.findings.len(), 1);
    assert_eq!(parent_findings.findings[0].id, "CHILD-ONLY-FID");
    assert!(
        parent_dir
            .join("output")
            .join("review-verdict.json")
            .exists()
    );
    assert!(
        parent_dir
            .join(crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE)
            .exists()
    );
    assert!(
        !child_dir
            .join("output")
            .join("review-verdict.json")
            .exists(),
        "consensus artifacts must be parent-session artifacts, not child-only outputs"
    );
}

#[test]
fn write_multi_reviewer_parent_artifacts_promotes_empty_findings_to_pass() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "verdict": "PASS",
            "summary": "No blocking findings in main...HEAD.",
            "findings": []
        }))
        .expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: "01CHILDSESSION0000000000000".to_string(),
        output: "Reviewer wrote a clean findings artifact.".to_string(),
        exit_code: 1,
        verdict: crate::review_consensus::HAS_ISSUES,
        diagnostic: None,
    }];
    let parent_meta = csa_session::state::ReviewSessionMeta {
        session_id: "01PARENTSESSION000000000000".to_string(),
        head_sha: "abcdef1234567890".to_string(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: crate::review_consensus::HAS_ISSUES.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "consensus".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: Some("sha256:test".to_string()),
    };

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        1,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        Some(&parent_meta),
    )
    .expect("parent artifacts should be produced");

    let output_dir = temp.path().join("output");
    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(output_dir.join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::CLEAN);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));

    let findings_toml: FindingsFile = toml::from_str(
        &fs::read_to_string(output_dir.join("findings.toml")).expect("findings.toml should exist"),
    )
    .expect("findings.toml should parse");
    assert!(findings_toml.findings.is_empty());
    assert!(
        fs::read_to_string(output_dir.join("summary.md"))
            .expect("summary should exist")
            .starts_with("Final verdict: CLEAN")
    );

    let written_meta: csa_session::state::ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(temp.path().join("review_meta.json"))
            .expect("review_meta.json should exist"),
    )
    .expect("review meta should parse");
    assert_eq!(written_meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(written_meta.verdict, crate::review_consensus::CLEAN);
    assert_eq!(written_meta.exit_code, 0);
}

#[test]
fn parent_review_decision_fails_closed_on_unbacked_has_issues_consensus() {
    // #1659: a HAS_ISSUES consensus with an empty consolidated artifact is trustworthy as
    // PASS only when EVERY dissenting (HAS_ISSUES-voting) reviewer persisted its structured
    // findings. The 4th arg is `dissent_findings_persisted`: when false, at least one
    // dissenter's findings never reached disk (quota/auth failure), so the empty artifact
    // must FAIL-CLOSED, never promote to a synthetic PASS.
    let empty = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&[]),
        findings: Vec::new(),
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: "01PARENTSESSION000000000000".to_string(),
        timestamp: chrono::Utc::now(),
    };
    assert_eq!(
        parent_review_decision(&empty, crate::review_consensus::HAS_ISSUES, false, false),
        ReviewDecision::Fail,
        "#1659: unbacked HAS_ISSUES consensus must fail-closed"
    );
    assert_eq!(
        parent_review_decision(&empty, crate::review_consensus::HAS_ISSUES, false, true),
        ReviewDecision::Pass,
        "every dissenting reviewer persisting an explicit empty artifact remains a trusted PASS"
    );
    assert_eq!(
        parent_review_decision(&empty, crate::review_consensus::CLEAN, false, false),
        ReviewDecision::Pass,
        "a clean consensus with no artifacts must not fail-closed"
    );
}

#[test]
fn write_multi_reviewer_parent_artifacts_fails_closed_without_persisted_reviewer_artifact() {
    // #1659 end-to-end: reviewers reached HAS_ISSUES but none persisted a
    // review-findings.json (quota/auth failure). The parent gate verdict MUST be fail,
    // not a synthetic-empty PASS, so the merge gate cannot be silently bypassed.
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    // Deliberately persist NO reviewer-N/review-findings.json (the #1659 condition).
    let outcomes = vec![
        super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: "01CHILDSESSION0000000000000".to_string(),
            output: "Reviewer flagged issues in prose but persisted no structured findings."
                .to_string(),
            exit_code: 1,
            verdict: crate::review_consensus::HAS_ISSUES,
            diagnostic: None,
        },
        super::super::output::ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::GeminiCli,
            session_id: "reviewer-2-unavailable".to_string(),
            output: "Review unavailable: quota exhausted\n".to_string(),
            exit_code: 1,
            verdict: UNAVAILABLE,
            diagnostic: Some("quota exhausted".to_string()),
        },
    ];

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        2,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        None,
    )
    .expect("parent artifacts should be produced");

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(
        verdict.decision,
        ReviewDecision::Fail,
        "#1659: HAS_ISSUES consensus with no persisted reviewer findings must fail-closed"
    );
}

#[test]
fn write_multi_reviewer_parent_artifacts_fails_closed_when_empty_artifact_masks_unpersisted_dissent()
 {
    // #1659 round-2 (codex): a single reviewer persisting an EMPTY artifact must NOT mask
    // another reviewer that voted HAS_ISSUES but never persisted its findings. The earlier
    // any-reviewer-loaded discriminator wrongly trusted the empty artifact and promoted to
    // PASS; the per-dissenter check must fail-closed because the HAS_ISSUES voter (reviewer 2)
    // left no structured findings on disk.
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    // Reviewer 1 (CLEAN) persists an EXPLICIT empty artifact.
    let reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "verdict": "PASS",
            "summary": "No blocking findings.",
            "findings": []
        }))
        .expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    // Reviewer 2 (HAS_ISSUES) persists NOTHING -- the masked dissenter (no reviewer-2 dir).
    let outcomes = vec![
        super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::GeminiCli,
            session_id: "01CLEANREVIEWER00000000000".to_string(),
            output: "Reviewer 1 was clean.".to_string(),
            exit_code: 0,
            verdict: crate::review_consensus::CLEAN,
            diagnostic: None,
        },
        super::super::output::ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::Codex,
            session_id: "01DISSENTREVIEWER0000000000".to_string(),
            output: "Reviewer 2 flagged blocking issues in prose but persisted no findings."
                .to_string(),
            exit_code: 1,
            verdict: crate::review_consensus::HAS_ISSUES,
            diagnostic: None,
        },
    ];

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        2,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        None,
    )
    .expect("parent artifacts should be produced");

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(
        verdict.decision,
        ReviewDecision::Fail,
        "#1659 round-2: an empty artifact must not mask an unpersisted HAS_ISSUES dissenter"
    );
}

#[test]
fn write_multi_reviewer_parent_artifacts_marks_all_unavailable() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");
    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: "reviewer-1-unavailable".to_string(),
        output: "Review unavailable: reviewer timed out after 1800s\n".to_string(),
        exit_code: 1,
        verdict: UNAVAILABLE,
        diagnostic: Some("reviewer timed out after 1800s".to_string()),
    }];

    write_multi_reviewer_parent_artifacts(temp.path(), 1, &outcomes, UNAVAILABLE, true, None)
        .expect("parent artifacts should be produced");

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Unavailable);
    assert_eq!(verdict.verdict_legacy, UNAVAILABLE);
}

#[test]
fn write_multi_reviewer_consensus_artifacts_preserves_blocking_findings_on_clean_consensus() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![Finding {
        severity: Severity::High,
        fid: "FID-1".to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(7),
        rule_id: "rule.review.minority-finding".to_string(),
        summary: "minority reviewer finding".to_string(),
        engine: "reviewer".to_string(),
    }];
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&ReviewArtifact {
            severity_summary: SeveritySummary::from_findings(&findings),
            findings,
            review_mode: Some("diff".to_string()),
            schema_version: "1.0".to_string(),
            session_id: "01CHILDSESSION0000000000000".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    let outcomes = vec![
        super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: "01CHILDSESSION0000000000000".to_string(),
            output: "Reviewer found an issue.".to_string(),
            exit_code: 1,
            verdict: crate::review_consensus::HAS_ISSUES,
            diagnostic: None,
        },
        super::super::output::ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::GeminiCli,
            session_id: "01CLEANREVIEWER00000000000".to_string(),
            output: "Reviewer was clean.".to_string(),
            exit_code: 0,
            verdict: crate::review_consensus::CLEAN,
            diagnostic: None,
        },
    ];
    let ctx = MultiReviewerConsensusArtifacts {
        project_root: temp.path(),
        reviewers: 2,
        outcomes: &outcomes,
        final_verdict: crate::review_consensus::CLEAN,
        all_reviewers_unavailable: false,
        head_sha: "abcdef1234567890",
        scope: "range:main...HEAD",
        review_iterations: 2,
        diff_fingerprint: Some("sha256:test".to_string()),
    };

    write_multi_reviewer_consensus_artifacts(ctx)
        .expect("parent consensus artifacts should be produced");

    let written_meta: csa_session::state::ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(temp.path().join("review_meta.json")).unwrap())
            .expect("review meta should parse");
    assert_eq!(written_meta.decision, ReviewDecision::Fail.as_str());
    assert_eq!(written_meta.verdict, crate::review_consensus::HAS_ISSUES);

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::HAS_ISSUES);
    assert_eq!(verdict.severity_counts[&Severity::High], 1);

    let parent_findings: FindingsFile = toml::from_str(
        &fs::read_to_string(temp.path().join("output").join("findings.toml"))
            .expect("findings.toml should exist"),
    )
    .expect("findings.toml should parse");
    assert_eq!(parent_findings.findings.len(), 1);
    assert_eq!(parent_findings.findings[0].id, "FID-1");

    let parent_artifact: ReviewArtifact = serde_json::from_str(
        &fs::read_to_string(
            temp.path()
                .join(crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        )
        .expect("review-findings-consolidated.json should exist"),
    )
    .expect("review-findings-consolidated.json should parse");
    assert_eq!(parent_artifact.findings.len(), 1);
    assert_eq!(parent_artifact.findings[0].fid, "FID-1");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn parent_aggregation_never_drops_blocking_findings_when_consensus_text_is_clean(
        states in prop::collection::vec(reviewer_state_strategy(), 2..=4),
    ) {
        let reviewer_artifacts: Vec<ReviewArtifact> = states
            .iter()
            .enumerate()
            .filter(|(_, state)| matches!(state, ReviewerState::Fail))
            .map(|(idx, _)| {
                let findings = vec![blocking_finding(format!("PROP-BLOCKING-{idx}"))];
                ReviewArtifact {
                    severity_summary: SeveritySummary::from_findings(&findings),
                    findings,
                    review_mode: Some("diff".to_string()),
                    schema_version: "1.0".to_string(),
                    session_id: format!("01TESTREVIEWER{idx:012}"),
                    timestamp: chrono::Utc::now(),
                }
            })
            .collect();
        let consolidated = crate::review_consensus::build_consolidated_artifact(
            reviewer_artifacts,
            "01PARENTSESSION000000000000",
        );
        // final_verdict is always CLEAN here, so the #1659 dissent guard never fires; pass
        // `true` (consensus treated as backed) -- this proptest exercises the
        // blocking-findings-survive-clean-consensus path, not the fail-closed guard.
        let decision = parent_review_decision(
            &consolidated,
            crate::review_consensus::CLEAN,
            states.iter().all(|state| matches!(state, ReviewerState::Unavailable)),
            true,
        );
        let parent_artifact = parent_artifact_for_decision(&consolidated, decision);

        let expected_blocking_ids: Vec<String> = states
            .iter()
            .enumerate()
            .filter(|(_, state)| matches!(state, ReviewerState::Fail))
            .map(|(idx, _)| format!("PROP-BLOCKING-{idx}"))
            .collect();
        for expected_id in &expected_blocking_ids {
            prop_assert!(
                parent_artifact.findings.iter().any(|finding| finding.fid == *expected_id),
                "blocking finding {expected_id} must survive clean consensus text"
            );
        }
        if expected_blocking_ids.is_empty() {
            prop_assert!(
                decision != ReviewDecision::Fail,
                "clean consensus without blocking artifacts should not be promoted to failure"
            );
        } else {
            prop_assert_eq!(decision, ReviewDecision::Fail);
        }
    }
}

fn reviewer_state_strategy() -> impl Strategy<Value = ReviewerState> {
    prop_oneof![
        Just(ReviewerState::Pass),
        Just(ReviewerState::Fail),
        Just(ReviewerState::Unavailable),
    ]
}

#[test]
fn write_standalone_consensus_review_artifacts_updates_carrier_session() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project dir should be created");
    let carrier = csa_session::create_session_fresh(
        &project,
        Some("review[1]: range:main...HEAD"),
        None,
        None,
    )
    .expect("carrier session should be created");
    let carrier_id = carrier.meta_session_id.clone();
    let outcomes = vec![
        super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: carrier_id.clone(),
            output: "Reviewer 1 found issues.".to_string(),
            exit_code: 1,
            verdict: crate::review_consensus::HAS_ISSUES,
            diagnostic: None,
        },
        super::super::output::ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::GeminiCli,
            session_id: "01OTHERREVIEWER00000000000".to_string(),
            output: "Reviewer 2 was clean.".to_string(),
            exit_code: 0,
            verdict: crate::review_consensus::CLEAN,
            diagnostic: None,
        },
    ];

    let ctx = MultiReviewerConsensusArtifacts {
        project_root: &project,
        reviewers: 2,
        outcomes: &outcomes,
        final_verdict: crate::review_consensus::CLEAN,
        all_reviewers_unavailable: false,
        head_sha: "abcdef1234567890",
        scope: "range:main...HEAD",
        review_iterations: 2,
        diff_fingerprint: Some("sha256:test".to_string()),
    };

    let written = write_standalone_consensus_review_artifacts(&ctx)
        .expect("standalone consensus artifacts should be written");

    assert_eq!(written.as_deref(), Some(carrier_id.as_str()));
    let session_dir = csa_session::get_session_dir(&project, &carrier_id).unwrap();
    let meta: csa_session::state::ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(session_dir.join("review_meta.json")).unwrap())
            .expect("review meta should parse");
    assert_eq!(meta.tool, "consensus");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.verdict, crate::review_consensus::CLEAN);
    assert_eq!(meta.scope, "range:main...HEAD");
    assert_eq!(meta.head_sha, "abcdef1234567890");

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::CLEAN);
    assert!(
        fs::read_to_string(session_dir.join("output").join("summary.md"))
            .expect("summary should exist")
            .contains("Final verdict: CLEAN")
    );
}

#[test]
fn standalone_consensus_preserves_blocking_child_findings_on_clean_consensus() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project dir should be created");
    let carrier = csa_session::create_session_fresh(
        &project,
        Some("review[1]: range:main...HEAD"),
        None,
        None,
    )
    .expect("carrier session should be created");
    let carrier_id = carrier.meta_session_id.clone();
    let carrier_dir = csa_session::get_session_dir(&project, &carrier_id).unwrap();
    let reviewer_dir = carrier_dir.join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![Finding {
        severity: Severity::High,
        fid: "STANDALONE-FID".to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(13),
        rule_id: "rule.standalone-artifact".to_string(),
        summary: "standalone artifact finding".to_string(),
        engine: "reviewer".to_string(),
    }];
    let artifact = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: carrier_id.clone(),
        timestamp: chrono::Utc::now(),
    };
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&artifact).expect("artifact should serialize"),
    )
    .expect("review artifact should be written");
    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: carrier_id.clone(),
        output: "Reviewer text was clean, but artifact contains a blocking finding.".to_string(),
        exit_code: 0,
        verdict: crate::review_consensus::CLEAN,
        diagnostic: None,
    }];
    let ctx = MultiReviewerConsensusArtifacts {
        project_root: &project,
        reviewers: 1,
        outcomes: &outcomes,
        final_verdict: crate::review_consensus::CLEAN,
        all_reviewers_unavailable: false,
        head_sha: "abcdef1234567890",
        scope: "range:main...HEAD",
        review_iterations: 1,
        diff_fingerprint: Some("sha256:test".to_string()),
    };

    let written = write_standalone_consensus_review_artifacts(&ctx)
        .expect("standalone consensus artifacts should be written");

    assert_eq!(written.as_deref(), Some(carrier_id.as_str()));
    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(carrier_dir.join("output").join("review-verdict.json"))
            .expect("review verdict should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));

    let findings_toml: FindingsFile = toml::from_str(
        &fs::read_to_string(carrier_dir.join("output").join("findings.toml"))
            .expect("findings.toml should exist"),
    )
    .expect("findings.toml should parse");
    assert_eq!(findings_toml.findings.len(), 1);
    assert_eq!(findings_toml.findings[0].id, "STANDALONE-FID");
}

#[test]
fn write_standalone_consensus_review_artifacts_skips_synthetic_unavailable_carrier() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project dir should be created");
    let carrier = csa_session::create_session_fresh(
        &project,
        Some("review[2]: range:main...HEAD"),
        None,
        None,
    )
    .expect("carrier session should be created");
    let carrier_id = carrier.meta_session_id.clone();
    let outcomes = vec![
        super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: "reviewer-1-unavailable".to_string(),
            output: "Review unavailable: reviewer timed out after 1800s\n".to_string(),
            exit_code: 1,
            verdict: UNAVAILABLE,
            diagnostic: Some("reviewer timed out after 1800s".to_string()),
        },
        super::super::output::ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::GeminiCli,
            session_id: carrier_id.clone(),
            output: "Reviewer 2 was clean.".to_string(),
            exit_code: 0,
            verdict: crate::review_consensus::CLEAN,
            diagnostic: None,
        },
    ];
    let synthetic_dir = csa_session::get_session_dir(&project, "reviewer-1-unavailable").unwrap();
    fs::create_dir_all(synthetic_dir.join("output"))
        .expect("synthetic sidecar output dir should be created");
    fs::write(
        synthetic_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("synthetic sidecar findings should be written");

    let ctx = MultiReviewerConsensusArtifacts {
        project_root: &project,
        reviewers: 2,
        outcomes: &outcomes,
        final_verdict: crate::review_consensus::CLEAN,
        all_reviewers_unavailable: false,
        head_sha: "abcdef1234567890",
        scope: "range:main...HEAD",
        review_iterations: 2,
        diff_fingerprint: Some("sha256:test".to_string()),
    };

    let written = write_standalone_consensus_review_artifacts(&ctx)
        .expect("standalone consensus artifacts should be written");

    assert_eq!(written.as_deref(), Some(carrier_id.as_str()));
    let session_dir = csa_session::get_session_dir(&project, &carrier_id).unwrap();
    let meta: csa_session::state::ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(session_dir.join("review_meta.json")).unwrap())
            .expect("review meta should parse");
    assert_eq!(meta.session_id, carrier_id);
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.verdict, crate::review_consensus::CLEAN);

    assert!(!synthetic_dir.join("review_meta.json").exists());
}

#[test]
fn write_multi_reviewer_parent_artifacts_writes_daemon_review_meta() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _daemon_session_dir_guard =
        ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_DIR", &session_dir);
    let _daemon_session_id_guard =
        ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", "01PARENTSESSION000000000000");
    let _session_dir_guard =
        ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, "/unrelated/session");
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01UNRELATEDSESSION0000000000");

    let outcomes = vec![super::super::output::ReviewerOutcome {
        reviewer_index: 0,
        tool: ToolName::Codex,
        session_id: "01CHILDSESSION0000000000000".to_string(),
        output: "Reviewer details".to_string(),
        exit_code: 0,
        verdict: crate::review_consensus::CLEAN,
        diagnostic: None,
    }];
    let parent_meta = csa_session::state::ReviewSessionMeta {
        session_id: "01PARENTSESSION000000000000".to_string(),
        head_sha: "abcdef1234567890".to_string(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: crate::review_consensus::CLEAN.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "consensus".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: Some("sha256:test".to_string()),
    };

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        1,
        &outcomes,
        crate::review_consensus::CLEAN,
        false,
        Some(&parent_meta),
    )
    .expect("parent artifacts should be produced");

    let written_meta: csa_session::state::ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(temp.path().join("review_meta.json")).unwrap())
            .expect("review meta should parse");
    assert_eq!(written_meta.session_id, "01PARENTSESSION000000000000");
    assert_eq!(written_meta.tool, "consensus");
    assert_eq!(written_meta.decision, ReviewDecision::Pass.as_str());
}

#[test]
fn parent_consensus_review_meta_falls_back_to_session_env() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");
    let _session_dir_guard =
        ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, "/tmp/parent-session");
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");

    let meta = parent_consensus_review_meta(
        "abcdef1234567890",
        "range:main...HEAD",
        crate::review_consensus::CLEAN,
        2,
        Some("sha256:test".to_string()),
    )
    .expect("session env should synthesize parent review meta");

    assert_eq!(meta.session_id, "01PARENTSESSION000000000000");
    assert_eq!(meta.tool, "consensus");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.scope, "range:main...HEAD");
    assert_eq!(meta.review_iterations, 2);
}
