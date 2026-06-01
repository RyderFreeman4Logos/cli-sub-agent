use super::{CLEAN, persist_fix_final_artifacts};
use crate::test_env_lock::ScopedTestEnvVar;
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, Severity, write_findings_toml,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn make_clean_review_meta(session_id: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: CLEAN.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "diff".to_string(),
        exit_code: 0,
        fix_attempted: true,
        fix_rounds: 1,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        fix_convergence: None,
    }
}

fn temp_project_root(test_name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("csa-{test_name}-{suffix}"));
    fs::create_dir_all(&path).expect("create temp project root");
    path
}

fn unique_session_id(prefix: &str) -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    format!("{prefix}-{suffix}")
}

fn create_session_dir(project_root: &Path, session_id: &str) -> PathBuf {
    let session_dir =
        csa_session::get_session_dir(project_root, session_id).expect("resolve session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    session_dir
}

fn sample_stale_finding() -> ReviewFinding {
    ReviewFinding {
        id: "stale-medium".to_string(),
        severity: Severity::Medium,
        file_ranges: vec![ReviewFindingFileRange {
            path: "src/lib.rs".to_string(),
            start: 42,
            end: Some(42),
        }],
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: "Stale finding from a previous fix round.".to_string(),
    }
}

#[test]
fn persist_fix_final_artifacts_rewrites_stale_findings_toml_to_empty_on_clean() {
    let project_root = temp_project_root("persist-fix-final-artifacts");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXFINALARTIFACTS");
    let session_dir = create_session_dir(&project_root, &session_id);

    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![sample_stale_finding()],
        },
    )
    .expect("write stale findings.toml");

    persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

    let findings_path = session_dir.join("output").join("findings.toml");
    assert!(
        findings_path.exists(),
        "findings.toml should remain present"
    );

    let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse findings.toml");
    assert_eq!(parsed, FindingsFile::default());
}

#[test]
fn persist_fix_final_artifacts_refreshes_verdict_after_findings_normalized() {
    let project_root = temp_project_root("persist-fix-final-artifacts-verdict-refresh");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXFINALVERDICT");
    let session_dir = create_session_dir(&project_root, &session_id);

    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![ReviewFinding {
                id: "stale-high".to_string(),
                severity: Severity::High,
                file_ranges: vec![ReviewFindingFileRange {
                    path: "src/lib.rs".to_string(),
                    start: 7,
                    end: Some(7),
                }],
                is_regression_of_commit: None,
                suggested_test_scenario: None,
                description: "Stale high finding from a previous fix round.".to_string(),
            }],
        },
    )
    .expect("write stale findings.toml");

    fs::write(
        session_dir.join("output").join("full.md"),
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking issues found in this scope.\nOverall risk: low\n<!-- CSA:SECTION:details:END -->",
    )
    .expect("write full output transcript");

    persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
}

/// Round 4 repro (#1045): stale review-findings.json with a HIGH finding
/// left over from the pre-fix review round. Fix loop converges clean →
/// both findings.toml and review-findings.json must be cleared, and the
/// final verdict must report decision=pass / severity_counts.high=0.
#[test]
fn persist_fix_final_artifacts_clears_stale_review_findings_json_on_clean() {
    let project_root = temp_project_root("persist-fix-stale-json");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXSTALEJSON");
    let session_dir = create_session_dir(&project_root, &session_id);

    // Seed stale review-findings.json with a HIGH finding (from pre-fix round).
    let stale_json = serde_json::json!({
        "findings": [{
            "severity": "high",
            "fid": "stale-high",
            "file": "src/lib.rs",
            "line": 42,
            "rule_id": "rule.stale",
            "summary": "Stale high finding from pre-fix review",
            "engine": "reviewer"
        }],
        "severity_summary": { "critical": 0, "high": 1, "medium": 0, "low": 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
    )
    .expect("write stale review-findings.json");

    // No stale findings.toml — persist_fix_final_artifacts will create a clean one.
    persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

    // findings.toml must be empty.
    let findings_path = session_dir.join("output").join("findings.toml");
    let parsed: FindingsFile =
        toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
            .expect("parse findings.toml");
    assert_eq!(parsed, FindingsFile::default());

    // review-findings.json must be removed.
    assert!(
        !session_dir.join("review-findings.json").exists(),
        "review-findings.json should be removed after clean convergence"
    );

    // Verdict must report pass.
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
}

/// Stale consolidated review artifact with a HIGH finding must be cleared
/// on clean convergence, otherwise it takes precedence over the clean
/// findings.toml and forces a false fail.
#[test]
fn persist_fix_final_artifacts_clears_stale_consolidated_findings_json_on_clean() {
    let project_root = temp_project_root("persist-fix-stale-consolidated-json");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXSTALECONSOLIDATED");
    let session_dir = create_session_dir(&project_root, &session_id);

    let stale_consolidated_json = serde_json::json!({
        "findings": [{
            "severity": "high",
            "fid": "stale-consolidated-high",
            "file": "src/lib.rs",
            "line": 42,
            "rule_id": "rule.stale-consolidated",
            "summary": "Stale high finding from consolidated artifact",
            "engine": "review-consensus"
        }],
        "severity_summary": { "critical": 0, "high": 1, "medium": 0, "low": 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join(crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        serde_json::to_vec_pretty(&stale_consolidated_json)
            .expect("serialize stale consolidated json"),
    )
    .expect("write stale consolidated review-findings.json");

    persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

    let findings_path = session_dir.join("output").join("findings.toml");
    let parsed: FindingsFile =
        toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
            .expect("parse findings.toml");
    assert_eq!(parsed, FindingsFile::default());

    assert!(
        !session_dir
            .join(crate::bug_class::CONSOLIDATED_REVIEW_ARTIFACT_FILE)
            .exists(),
        "review-findings-consolidated.json should be removed after clean convergence"
    );

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
}

/// Mixed staleness (#1045 round 4): stale findings.toml with a MEDIUM finding
/// + stale review-findings.json with a HIGH finding. Converge clean → both
///   must be cleared, verdict must be pass.
#[test]
fn persist_fix_final_artifacts_clears_both_stale_artifacts_on_clean() {
    let project_root = temp_project_root("persist-fix-both-stale");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXBOTHSTALE");
    let session_dir = create_session_dir(&project_root, &session_id);

    // Stale findings.toml with a MEDIUM finding.
    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![sample_stale_finding()],
        },
    )
    .expect("write stale findings.toml");

    // Stale review-findings.json with a HIGH finding.
    let stale_json = serde_json::json!({
        "findings": [{
            "severity": "high",
            "fid": "stale-high-json",
            "file": "src/main.rs",
            "line": 10,
            "rule_id": "rule.stale-json",
            "summary": "Stale high from JSON",
            "engine": "reviewer"
        }],
        "severity_summary": { "critical": 0, "high": 1, "medium": 0, "low": 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
    )
    .expect("write stale review-findings.json");

    persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

    // findings.toml must be empty.
    let findings_path = session_dir.join("output").join("findings.toml");
    let parsed: FindingsFile =
        toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
            .expect("parse findings.toml");
    assert_eq!(parsed, FindingsFile::default());

    // review-findings.json must be removed.
    assert!(
        !session_dir.join("review-findings.json").exists(),
        "review-findings.json should be removed after clean convergence"
    );

    // Verdict must report pass with zero blocking counts.
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&0));
}

/// Non-converged fix (exhausted rounds): stale artifacts must be PRESERVED
/// — this is the existing contract per decision #820 Option A.
#[test]
fn fix_loop_exhausted_preserves_stale_review_findings_json() {
    let project_root = temp_project_root("persist-fix-exhausted-json");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXEXHAUSTEDJSON");
    let session_dir = create_session_dir(&project_root, &session_id);

    // Stale review-findings.json with a HIGH finding.
    let stale_json = serde_json::json!({
        "findings": [{
            "severity": "high",
            "fid": "stale-high-exhausted",
            "file": "src/lib.rs",
            "line": 99,
            "rule_id": "rule.stale-exhausted",
            "summary": "Stale high finding persisted on exhaustion",
            "engine": "reviewer"
        }],
        "severity_summary": { "critical": 0, "high": 1, "medium": 0, "low": 0 },
        "overall_risk": "high"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
    )
    .expect("write stale review-findings.json");

    // Stale findings.toml too.
    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![sample_stale_finding()],
        },
    )
    .expect("write stale findings.toml");

    let mut exhausted_meta = make_clean_review_meta(&session_id);
    exhausted_meta.decision = ReviewDecision::Fail.as_str().to_string();
    exhausted_meta.verdict = "HAS_ISSUES".to_string();
    exhausted_meta.exit_code = 1;
    exhausted_meta.fix_rounds = 3;

    persist_fix_final_artifacts(&project_root, &exhausted_meta, false);

    // review-findings.json must still exist (not cleaned on non-convergence).
    assert!(
        session_dir.join("review-findings.json").exists(),
        "review-findings.json should be preserved when fix loop is exhausted"
    );

    // findings.toml must still have the stale content.
    let findings_path = session_dir.join("output").join("findings.toml");
    let parsed: FindingsFile =
        toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
            .expect("parse findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_stale_finding()],
        }
    );
}

/// #1048 M3: --fix session that started from synthetic-empty initial
/// review, converges clean → synthetic marker must be removed alongside
/// findings.toml and review-findings.json.
///
/// Bug: persist_fix_final_artifacts(converged_clean=true) cleared
/// findings.toml + review-findings.json but left the synthetic sidecar
/// marker in place, causing derive_review_verdict_artifact to fall
/// through to full.md on subsequent reads.
#[test]
fn persist_fix_final_artifacts_clears_synthetic_marker_on_clean_convergence() {
    let project_root = temp_project_root("persist-fix-synthetic-marker-clean");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXSYNTHETICMARKERCLEAN");
    let session_dir = create_session_dir(&project_root, &session_id);

    // Create the synthetic marker (simulates a fix session that started
    // from a synthetic-empty initial review).
    let marker_path = session_dir
        .join("output")
        .join(super::super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
    fs::write(&marker_path, b"").expect("write synthetic marker");

    // Seed stale review-findings.json to verify it's also removed.
    let stale_json = serde_json::json!({
        "findings": [{
            "severity": "medium",
            "fid": "stale-medium-synth",
            "file": "src/lib.rs",
            "line": 10,
            "rule_id": "rule.stale",
            "summary": "Stale finding from pre-fix review",
            "engine": "reviewer"
        }],
        "severity_summary": { "critical": 0, "high": 0, "medium": 1, "low": 0 },
        "overall_risk": "medium"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
    )
    .expect("write stale review-findings.json");

    persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

    // findings.toml must be empty.
    let findings_path = session_dir.join("output").join("findings.toml");
    let parsed: FindingsFile =
        toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
            .expect("parse findings.toml");
    assert_eq!(parsed, FindingsFile::default());

    // review-findings.json must be removed.
    assert!(
        !session_dir.join("review-findings.json").exists(),
        "review-findings.json should be removed after clean convergence"
    );

    // Synthetic marker must be removed (#1048 M3).
    assert!(
        !marker_path.exists(),
        "#1048 M3: synthetic marker must be removed after clean convergence"
    );

    // Verdict must report pass.
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
}

#[test]
fn fix_loop_exhausted_preserves_open_findings_in_findings_toml() {
    let project_root = temp_project_root("persist-fix-final-artifacts-exhausted");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXEXHAUSTEDARTIFACTS");
    let session_dir = create_session_dir(&project_root, &session_id);
    let existing = FindingsFile {
        findings: vec![sample_stale_finding()],
    };

    write_findings_toml(&session_dir, &existing).expect("write last-round findings.toml");

    let mut exhausted_meta = make_clean_review_meta(&session_id);
    exhausted_meta.decision = ReviewDecision::Fail.as_str().to_string();
    exhausted_meta.verdict = "HAS_ISSUES".to_string();
    exhausted_meta.exit_code = 1;
    exhausted_meta.fix_rounds = 3;

    persist_fix_final_artifacts(&project_root, &exhausted_meta, false);

    let findings_path = session_dir.join("output").join("findings.toml");
    assert!(
        findings_path.exists(),
        "findings.toml should remain present after exhausted fix loop"
    );

    let actual = fs::read_to_string(&findings_path).expect("read preserved findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse preserved findings.toml");
    assert_eq!(parsed, existing);
}
