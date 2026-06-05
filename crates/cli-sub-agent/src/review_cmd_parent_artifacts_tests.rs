use super::{
    MultiReviewerConsensusArtifacts, ReviewDiffReport, clear_multi_reviewer_artifact_dirs,
    parent_artifact_for_decision, parent_consensus_review_meta, parent_review_decision,
    parse_reviewer_artifact, write_multi_reviewer_consensus_artifacts,
    write_multi_reviewer_parent_artifacts, write_parent_review_verdict,
    write_standalone_consensus_review_artifacts,
};
use crate::review_consensus::UNAVAILABLE;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use csa_core::env::{CSA_SESSION_DIR_ENV_KEY, CSA_SESSION_ID_ENV_KEY};
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::review_artifact::{
    Finding, FindingsFile, ReviewArtifact, ReviewVerdictArtifact, Severity, SeveritySummary,
};
use proptest::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[derive(Clone, Copy, Debug)]
enum ReviewerState {
    Pass,
    Fail,
    Unavailable,
}

impl ReviewerState {
    fn verdict(self) -> &'static str {
        match self {
            Self::Pass => crate::review_consensus::CLEAN,
            Self::Fail => crate::review_consensus::HAS_ISSUES,
            Self::Unavailable => UNAVAILABLE,
        }
    }
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

fn finding_with_severity(severity: Severity, fid: impl Into<String>) -> Finding {
    let fid = fid.into();
    Finding {
        severity,
        fid: fid.clone(),
        file: "src/lib.rs".to_string(),
        line: Some(7),
        rule_id: format!("rule.review.{fid}"),
        summary: "review finding".to_string(),
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

fn startup_env_for_parent_session(
    session_dir: &Path,
    session_id: &str,
) -> crate::startup_env::StartupSubtreeEnv {
    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_SESSION_DIR_ENV_KEY, session_dir.display().to_string()),
        (CSA_SESSION_ID_ENV_KEY, session_id.to_string()),
    ]))
}

fn reviewer_outcome(
    reviewer_index: usize,
    tool: ToolName,
    verdict: &'static str,
    diagnostic: Option<&str>,
) -> super::super::output::ReviewerOutcome {
    super::super::output::ReviewerOutcome {
        reviewer_index,
        tool,
        session_id: format!("01TESTREVIEWER{reviewer_index:012}"),
        output: verdict.to_string(),
        exit_code: if verdict == crate::review_consensus::CLEAN {
            0
        } else {
            1
        },
        verdict,
        diagnostic: diagnostic.map(str::to_string),
    }
}

fn parent_verdict_for_outcomes(
    outcomes: &[super::super::output::ReviewerOutcome],
    final_verdict: &str,
    all_reviewers_unavailable: bool,
) -> ReviewVerdictArtifact {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        outcomes.len(),
        outcomes,
        final_verdict,
        all_reviewers_unavailable,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
        None,
    )
    .expect("parent artifacts should be produced");

    serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse")
}

#[test]
fn issue_1657_unavailable_reviewer_plus_clean_reviewer_passes() {
    let outcomes = vec![
        reviewer_outcome(0, ToolName::GeminiCli, UNAVAILABLE, Some("quota exhausted")),
        reviewer_outcome(
            1,
            ToolName::ClaudeCode,
            crate::review_consensus::CLEAN,
            None,
        ),
    ];

    let verdict =
        parent_verdict_for_outcomes(&outcomes, crate::review_consensus::HAS_ISSUES, false);

    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::CLEAN);
}

#[test]
fn issue_1815_unavailable_auto_escalated_co_reviewer_plus_clean_reviewer_passes() {
    let outcomes = vec![
        reviewer_outcome(0, ToolName::Codex, crate::review_consensus::CLEAN, None),
        reviewer_outcome(1, ToolName::GeminiCli, UNAVAILABLE, Some("quota exhausted")),
    ];

    let verdict =
        parent_verdict_for_outcomes(&outcomes, crate::review_consensus::HAS_ISSUES, false);

    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::CLEAN);
}

#[test]
fn issue_1657_unavailable_reviewer_plus_blocking_reviewer_fails() {
    let outcomes = vec![
        reviewer_outcome(0, ToolName::GeminiCli, UNAVAILABLE, Some("quota exhausted")),
        reviewer_outcome(
            1,
            ToolName::ClaudeCode,
            crate::review_consensus::HAS_ISSUES,
            None,
        ),
    ];

    let verdict = parent_verdict_for_outcomes(&outcomes, crate::review_consensus::CLEAN, false);

    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::HAS_ISSUES);
}

#[test]
fn issue_1657_all_unavailable_reviewers_are_unavailable_and_fail_closed() {
    let outcomes = vec![
        reviewer_outcome(0, ToolName::GeminiCli, UNAVAILABLE, Some("quota exhausted")),
        reviewer_outcome(1, ToolName::Codex, UNAVAILABLE, Some("tool unavailable")),
    ];

    let verdict = parent_verdict_for_outcomes(&outcomes, UNAVAILABLE, true);

    assert_eq!(verdict.decision, ReviewDecision::Unavailable);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_ne!(verdict.decision, ReviewDecision::Uncertain);
    assert_ne!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(
        crate::verdict_exit_code::exit_code_from_review_decision(verdict.decision),
        1
    );
    assert_eq!(verdict.verdict_legacy, UNAVAILABLE);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
}

#[test]
fn issue_1657_single_crashed_reviewer_without_verdict_fails_closed() {
    let outcomes = vec![reviewer_outcome(
        0,
        ToolName::Codex,
        crate::review_consensus::UNCERTAIN,
        Some("reviewer crashed before producing a verdict"),
    )];

    let verdict = parent_verdict_for_outcomes(&outcomes, crate::review_consensus::UNCERTAIN, false);

    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::HAS_ISSUES);
}

#[test]
fn issue_1657_two_clean_reviewers_pass() {
    let outcomes = vec![
        reviewer_outcome(0, ToolName::Codex, crate::review_consensus::CLEAN, None),
        reviewer_outcome(
            1,
            ToolName::ClaudeCode,
            crate::review_consensus::CLEAN,
            None,
        ),
    ];

    let verdict = parent_verdict_for_outcomes(&outcomes, crate::review_consensus::CLEAN, false);

    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, crate::review_consensus::CLEAN);
}

#[test]
fn issue_1696_parent_verdict_counts_all_reviewer_findings_when_decision_passes() {
    let temp = tempdir().expect("tempdir should be created");
    let findings = vec![
        finding_with_severity(Severity::Medium, "FID-MEDIUM"),
        finding_with_severity(Severity::Low, "FID-LOW-1"),
        finding_with_severity(Severity::Low, "FID-LOW-2"),
        finding_with_severity(Severity::Low, "FID-LOW-3"),
    ];
    let consolidated = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: "01PARENTSESSION000000000000".to_string(),
        timestamp: chrono::Utc::now(),
    };
    let decision = ReviewDecision::Pass;
    let parent_artifact = parent_artifact_for_decision(&consolidated, decision);

    write_parent_review_verdict(
        temp.path(),
        "01PARENTSESSION000000000000",
        &consolidated.findings,
        decision,
        crate::review_consensus::CLEAN,
        ReviewDiffReport {
            diff_size: None,
            large_diff_warning: None,
        },
        None,
    )
    .expect("parent review verdict should be written");

    let verdict_path = temp.path().join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_slice(&fs::read(verdict_path).expect("read review verdict"))
            .expect("review verdict should parse");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, crate::review_consensus::CLEAN);
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&1));
    assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&3));
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Critical), Some(&0));
    assert_eq!(
        parent_artifact.findings.len(),
        3,
        "the parent artifact may stay filtered while verdict counts remain descriptive"
    );
}

#[test]
fn issue_1817_consensus_parent_verdict_carries_review_mode() {
    // Multi-reviewer consensus: the parent verdict must record the review mode so
    // the merge gate can audit that a red-team consensus actually ran (#1817).
    let temp = tempdir().expect("tempdir should be created");
    let consolidated = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&[]),
        findings: Vec::new(),
        review_mode: Some("red-team".to_string()),
        schema_version: "1.0".to_string(),
        session_id: "01PARENTSESSION000000000000".to_string(),
        timestamp: chrono::Utc::now(),
    };
    let decision = ReviewDecision::Pass;
    let parent_artifact = parent_artifact_for_decision(&consolidated, decision);

    write_parent_review_verdict(
        temp.path(),
        "01PARENTSESSION000000000000",
        &consolidated.findings,
        decision,
        crate::review_consensus::CLEAN,
        ReviewDiffReport {
            diff_size: None,
            large_diff_warning: None,
        },
        parent_artifact.review_mode.as_deref(),
    )
    .expect("parent review verdict should be written");

    let verdict_path = temp.path().join("output").join("review-verdict.json");
    let artifact: ReviewVerdictArtifact =
        serde_json::from_slice(&fs::read(verdict_path).expect("read review verdict"))
            .expect("review verdict should parse");
    assert_eq!(
        artifact.review_mode.as_deref(),
        Some("red-team"),
        "consensus parent verdict must propagate the reviewers' red-team mode"
    );
}

fn init_git_repo_for_marker(project: &Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .status()
            .expect("git command should execute");
        assert!(status.success(), "git {args:?} should succeed");
    };
    run(&["init"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test User"]);
    fs::write(project.join("tracked.txt"), "baseline\n").expect("write baseline file");
    run(&["add", "tracked.txt"]);
    run(&["commit", "-m", "initial"]);
}

fn current_git_branch(project: &Path) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("git rev-parse should execute");
    assert!(output.status.success(), "git rev-parse should succeed");
    String::from_utf8(output.stdout)
        .expect("branch name should be utf-8")
        .trim()
        .to_string()
}

#[test]
fn issue_1817_mode_less_reviewer_on_red_team_run_tags_parent_pass() {
    // #1817 regression: when the per-reviewer findings artifact carries NO review_mode
    // (legacy / contract-format), the parent consensus must still record the RUN-level
    // mode on review_meta.json, review-verdict.json, AND the review-gate marker -- not
    // `None`. Otherwise `csa review --check-verdict --red-team` skips the mode-less
    // parent verdict and the red-team merge-gate audit cannot be proven.
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let project = temp.path();
    // A git repo is needed so the clean-verdict gate marker resolves a branch.
    init_git_repo_for_marker(project);
    let branch = current_git_branch(project);

    let session_dir = project.display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    // Contract-format reviewer artifact => parse_reviewer_artifact yields review_mode = None.
    let reviewer_dir = project.join("reviewer-1");
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

    let outcomes = vec![reviewer_outcome(
        0,
        ToolName::Codex,
        crate::review_consensus::CLEAN,
        None,
    )];
    let ctx = MultiReviewerConsensusArtifacts {
        project_root: project,
        reviewers: 1,
        outcomes: &outcomes,
        final_verdict: crate::review_consensus::CLEAN,
        all_reviewers_unavailable: false,
        head_sha: "abcdef1234567890",
        scope: "range:main...HEAD",
        run_review_mode: Some("red-team"),
        review_iterations: 1,
        diff_fingerprint: Some("sha256:test".to_string()),
        diff_size: None,
        large_diff_warning: None,
    };

    write_multi_reviewer_consensus_artifacts(
        ctx,
        &startup_env_for_parent_session(project, "01PARENTSESSION000000000000"),
    )
    .expect("parent consensus artifacts should be produced");

    let meta: csa_session::state::ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(project.join("review_meta.json"))
            .expect("review_meta.json should exist"),
    )
    .expect("review meta should parse");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(
        meta.review_mode.as_deref(),
        Some("red-team"),
        "a mode-less reviewer on a red-team run must still tag the parent meta red-team"
    );

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(project.join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(
        verdict.review_mode.as_deref(),
        Some("red-team"),
        "a mode-less reviewer on a red-team run must still tag the parent verdict red-team"
    );

    let marker = crate::review_gate::read_review_gate_marker(project, &branch, "abcdef1234567890")
        .expect("a clean red-team consensus must write a review-gate marker");
    assert_eq!(
        marker.review_mode.as_deref(),
        Some("red-team"),
        "a mode-less reviewer on a red-team run must still tag the review-gate marker red-team"
    );
}

#[test]
fn issue_1817_failing_parent_consensus_on_red_team_run_is_mode_tagged() {
    // #1817 regression (FAIL path): a REJECTING parent consensus on a red-team run must
    // record the run mode on review_meta.json + review-verdict.json so the red-team gate
    // matches and honors the parent FAIL instead of skipping it on a mode mismatch. (A
    // non-clean verdict writes no gate marker, by design, so the FAIL is honored via the
    // persisted session verdict.)
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    // Mode-less reviewer artifact carrying a blocking finding => consensus FAIL.
    let reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let findings = vec![blocking_finding("FAIL-FID")];
    fs::write(
        reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "verdict": "HAS_ISSUES",
            "summary": "Blocking finding in main...HEAD.",
            "findings": findings,
        }))
        .expect("artifact should serialize"),
    )
    .expect("review artifact should be written");

    let outcomes = vec![reviewer_outcome(
        0,
        ToolName::Codex,
        crate::review_consensus::HAS_ISSUES,
        None,
    )];
    let ctx = MultiReviewerConsensusArtifacts {
        project_root: temp.path(),
        reviewers: 1,
        outcomes: &outcomes,
        final_verdict: crate::review_consensus::HAS_ISSUES,
        all_reviewers_unavailable: false,
        head_sha: "abcdef1234567890",
        scope: "range:main...HEAD",
        run_review_mode: Some("red-team"),
        review_iterations: 1,
        diff_fingerprint: Some("sha256:test".to_string()),
        diff_size: None,
        large_diff_warning: None,
    };

    write_multi_reviewer_consensus_artifacts(
        ctx,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
    )
    .expect("parent consensus artifacts should be produced");

    let meta: csa_session::state::ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(temp.path().join("review_meta.json"))
            .expect("review_meta.json should exist"),
    )
    .expect("review meta should parse");
    assert_eq!(meta.decision, ReviewDecision::Fail.as_str());
    assert_eq!(
        meta.review_mode.as_deref(),
        Some("red-team"),
        "a failing red-team parent consensus must record the run mode so the gate honors the FAIL"
    );

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(
        verdict.review_mode.as_deref(),
        Some("red-team"),
        "a failing red-team parent verdict must be mode-tagged"
    );
}

#[test]
fn issue_1696_reviewer_artifact_ignores_info_without_dropping_countable_findings() {
    let temp = tempdir().expect("tempdir should be created");
    let reviewer_dir = temp.path().join("reviewer-3");
    fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
    let artifact_path = reviewer_dir.join("review-findings.json");
    let content = r#"{
        "findings": [
            {
                "severity": "medium",
                "fid": "FID-MEDIUM",
                "file": "src/lib.rs",
                "line": 7,
                "rule_id": "rule.review.medium",
                "summary": "medium finding",
                "engine": "reviewer"
            },
            {
                "severity": "low",
                "fid": "FID-LOW-1",
                "file": "src/lib.rs",
                "line": 8,
                "rule_id": "rule.review.low1",
                "summary": "low finding",
                "engine": "reviewer"
            },
            {
                "severity": "low",
                "fid": "FID-LOW-2",
                "file": "src/lib.rs",
                "line": 9,
                "rule_id": "rule.review.low2",
                "summary": "low finding",
                "engine": "reviewer"
            },
            {
                "severity": "low",
                "fid": "FID-LOW-3",
                "file": "src/lib.rs",
                "line": 10,
                "rule_id": "rule.review.low3",
                "summary": "low finding",
                "engine": "reviewer"
            },
            {
                "severity": "info",
                "fid": "FID-INFO",
                "file": "src/lib.rs",
                "line": 11,
                "rule_id": "rule.review.info",
                "summary": "informational note",
                "engine": "reviewer"
            }
        ],
        "severity_summary": {
            "critical": 0,
            "high": 0,
            "medium": 1,
            "low": 3
        }
    }"#;

    let artifact =
        parse_reviewer_artifact(&artifact_path, content).expect("reviewer artifact should parse");
    assert_eq!(artifact.findings.len(), 4);
    assert_eq!(artifact.severity_summary.medium, 1);
    assert_eq!(artifact.severity_summary.low, 3);
    assert_eq!(artifact.severity_summary.high, 0);
    assert_eq!(artifact.severity_summary.critical, 0);
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
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
        &startup_env_for_parent_session(&parent_dir, "01PARENTSESSION000000000000"),
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
        run_review_mode: None,
        review_iterations: 1,
        diff_fingerprint: Some("sha256:test".to_string()),
        diff_size: None,
        large_diff_warning: None,
    };

    write_multi_reviewer_consensus_artifacts(
        ctx,
        &startup_env_for_parent_session(&parent_dir, "01PARENTSESSION000000000000"),
    )
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
        exit_code: 0,
        verdict: crate::review_consensus::CLEAN,
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
        review_mode: None,
        fix_convergence: None,
    };

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        1,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
    // #1659: a produced HAS_ISSUES verdict is a real blocking vote even when
    // the reviewer failed to persist structured findings.
    let empty = ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&[]),
        findings: Vec::new(),
        review_mode: Some("diff".to_string()),
        schema_version: "1.0".to_string(),
        session_id: "01PARENTSESSION000000000000".to_string(),
        timestamp: chrono::Utc::now(),
    };
    let fail_outcomes = [reviewer_outcome(
        0,
        ToolName::Codex,
        crate::review_consensus::HAS_ISSUES,
        None,
    )];
    let clean_outcomes = [reviewer_outcome(
        0,
        ToolName::ClaudeCode,
        crate::review_consensus::CLEAN,
        None,
    )];
    assert_eq!(
        parent_review_decision(
            &empty,
            crate::review_consensus::HAS_ISSUES,
            &fail_outcomes,
            false,
            false
        ),
        ReviewDecision::Fail,
        "#1659: produced HAS_ISSUES verdict must fail"
    );
    assert_eq!(
        parent_review_decision(
            &empty,
            crate::review_consensus::CLEAN,
            &clean_outcomes,
            false,
            false
        ),
        ReviewDecision::Pass,
        "a clean consensus with no artifacts must not fail-closed"
    );
    assert_eq!(
        parent_review_decision(
            &empty,
            crate::review_consensus::HAS_ISSUES,
            &[],
            false,
            true
        ),
        ReviewDecision::Fail,
        "empty produced set must fail-closed"
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
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
fn clear_multi_reviewer_artifact_dirs_removes_stale_empty_artifact_so_dissenter_fails_closed() {
    // #1681: the parent/daemon session dir can survive across review invocations in a
    // plan. If reviewer-1 left an empty artifact in round 1, round 2 must clear it before
    // accepting reviewer-1 as a persisted HAS_ISSUES dissenter.
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
    let _session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");
    let _daemon_session_dir_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let stale_reviewer_dir = temp.path().join("reviewer-1");
    fs::create_dir_all(&stale_reviewer_dir).expect("stale reviewer dir should be created");
    fs::write(
        stale_reviewer_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "verdict": "PASS",
            "summary": "Round 1 had no findings.",
            "findings": []
        }))
        .expect("stale artifact should serialize"),
    )
    .expect("stale artifact should be written");

    let sibling_output_dir = temp.path().join("output");
    fs::create_dir_all(&sibling_output_dir).expect("output dir should be created");
    fs::write(
        sibling_output_dir.join("keep.txt"),
        "preserve sibling output",
    )
    .expect("sibling output marker should be written");
    let out_of_scope_reviewer_dir = temp.path().join("reviewer-3");
    fs::create_dir_all(&out_of_scope_reviewer_dir)
        .expect("out-of-scope reviewer dir should be created");

    clear_multi_reviewer_artifact_dirs(
        2,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
    )
    .expect("stale reviewer dirs should be cleared");

    assert!(
        !temp.path().join("reviewer-1").exists(),
        "reviewer-1 must be cleared before the new round writes"
    );
    assert!(
        !temp.path().join("reviewer-2").exists(),
        "missing reviewer-2 is treated as already clear"
    );
    assert!(
        sibling_output_dir.join("keep.txt").exists(),
        "clearing must preserve non-reviewer-N sibling artifacts"
    );
    assert!(
        out_of_scope_reviewer_dir.exists(),
        "clearing reviewers=2 must not remove reviewer-3"
    );

    let outcomes = vec![
        super::super::output::ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: "01DISSENTREVIEWER0000000000".to_string(),
            output: "Reviewer 1 flagged blocking issues but persisted no fresh artifact."
                .to_string(),
            exit_code: 1,
            verdict: crate::review_consensus::HAS_ISSUES,
            diagnostic: None,
        },
        super::super::output::ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::GeminiCli,
            session_id: "01CLEANREVIEWER00000000000".to_string(),
            output: "Reviewer 2 was clean.".to_string(),
            exit_code: 0,
            verdict: crate::review_consensus::CLEAN,
            diagnostic: None,
        },
    ];

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        2,
        &outcomes,
        crate::review_consensus::HAS_ISSUES,
        false,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
        "#1681: stale empty reviewer-N artifacts must not satisfy persisted dissent"
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

    let parent_meta = csa_session::state::ReviewSessionMeta {
        session_id: "01PARENTSESSION000000000000".to_string(),
        head_sha: "abcdef1234567890".to_string(),
        decision: ReviewDecision::Uncertain.as_str().to_string(),
        verdict: UNAVAILABLE.to_string(),
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
        review_mode: None,
        fix_convergence: None,
    };

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        1,
        &outcomes,
        UNAVAILABLE,
        true,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
        Some(&parent_meta),
    )
    .expect("parent artifacts should be produced");

    let verdict: ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Unavailable);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_ne!(verdict.decision, ReviewDecision::Uncertain);
    assert_ne!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(
        crate::verdict_exit_code::exit_code_from_review_decision(verdict.decision),
        1
    );
    assert_eq!(verdict.verdict_legacy, UNAVAILABLE);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));

    let written_meta: csa_session::state::ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(temp.path().join("review_meta.json")).unwrap())
            .expect("review meta should parse");
    assert_eq!(written_meta.decision, ReviewDecision::Unavailable.as_str());
    assert_eq!(written_meta.verdict, UNAVAILABLE);
    assert_eq!(written_meta.exit_code, 1);
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
        run_review_mode: None,
        review_iterations: 2,
        diff_fingerprint: Some("sha256:test".to_string()),
        diff_size: None,
        large_diff_warning: None,
    };

    write_multi_reviewer_consensus_artifacts(
        ctx,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
    )
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
        let outcomes: Vec<super::super::output::ReviewerOutcome> = states
            .iter()
            .enumerate()
            .map(|(idx, state)| reviewer_outcome(idx, ToolName::Codex, state.verdict(), None))
            .collect();
        let decision = parent_review_decision(
            &consolidated,
            crate::review_consensus::CLEAN,
            &outcomes,
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
        if states.iter().all(|state| matches!(state, ReviewerState::Unavailable)) {
            prop_assert_eq!(decision, ReviewDecision::Unavailable);
        } else if expected_blocking_ids.is_empty() {
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
            output: "Reviewer 1 was clean.".to_string(),
            exit_code: 0,
            verdict: crate::review_consensus::CLEAN,
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
        run_review_mode: None,
        review_iterations: 2,
        diff_fingerprint: Some("sha256:test".to_string()),
        diff_size: None,
        large_diff_warning: None,
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
        run_review_mode: None,
        review_iterations: 1,
        diff_fingerprint: Some("sha256:test".to_string()),
        diff_size: None,
        large_diff_warning: None,
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
        run_review_mode: None,
        review_iterations: 2,
        diff_fingerprint: Some("sha256:test".to_string()),
        diff_size: None,
        large_diff_warning: None,
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
        review_mode: None,
        fix_convergence: None,
    };

    write_multi_reviewer_parent_artifacts(
        temp.path(),
        1,
        &outcomes,
        crate::review_consensus::CLEAN,
        false,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
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
fn parent_consensus_review_meta_reads_startup_env() {
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
        &startup_env_for_parent_session(
            Path::new("/tmp/parent-session"),
            "01PARENTSESSION000000000000",
        ),
    )
    .expect("startup env should synthesize parent review meta");

    assert_eq!(meta.session_id, "01PARENTSESSION000000000000");
    assert_eq!(meta.tool, "consensus");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.scope, "range:main...HEAD");
    assert_eq!(meta.review_iterations, 2);
}
